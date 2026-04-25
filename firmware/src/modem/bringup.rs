//! Bring-up state machine: raw AT init + CMUX entry + PPP data-mode.
//!
//! Tries to be paranoid about the things that bite first:
//! * SIM800L sometimes auto-bauds — sends `AT` 3× until OK before any other
//!   command.
//! * Drains URCs (`+CFUN: 1`, `+CPIN: READY`, `Call Ready`, `SMS Ready`)
//!   instead of treating them as command responses.
//! * After `OK` to `AT+CMUX=0,...`, drains the UART for 50 ms before
//!   yielding it — anything stale that isn't a 27.010 frame breaks mux.

use embassy_time::{with_timeout, Duration, Timer};
use embedded_io_async::{Read, Write};
use heapless::String;

use crate::config;

const LINE_BUF: usize = 256;
const COMMAND_TIMEOUT: Duration = Duration::from_millis(2_000);
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(60);

// String fields are read via the `Debug` impl (used in log::error! calls).
// Rustc can't see through Debug, so it warns spuriously — silence locally.
#[allow(dead_code)]
#[derive(Debug)]
pub enum Error {
    Timeout(&'static str),
    Cmd(&'static str),
    Io,
    NotRegistered,
    SimLocked,
    LineTooLong,
}

/// Run the full pre-CMUX sequence and finish with `AT+CMUX=0`.
///
/// Returns once the modem has acknowledged the CMUX command. Caller must
/// then drain the UART for ≈50 ms and split it before spawning the CMUX
/// dispatcher / TX tasks.
pub async fn raw_at_init<U>(uart: &mut U) -> Result<(), Error>
where
    U: Read + Write + Unpin,
{
    let mut io = AtIo::new(uart);

    // 1) Echo / baud sanity. Modems often reply to the first AT with garbage
    //    if they auto-baud. Try a few times.
    for _ in 0..5 {
        if io.cmd("AT").await.is_ok() {
            break;
        }
        Timer::after(Duration::from_millis(100)).await;
    }
    io.cmd("AT").await.map_err(|_| Error::Timeout("AT echo"))?;

    io.cmd("ATE0").await.map_err(|_| Error::Cmd("ATE0"))?;
    io.cmd("AT+CMEE=2").await.map_err(|_| Error::Cmd("CMEE"))?;

    // 2) SIM PIN if configured.
    if !config::SIM_PIN.is_empty() {
        let mut s: String<32> = String::new();
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("AT+CPIN={}", config::SIM_PIN));
        io.cmd(&s).await.map_err(|_| Error::SimLocked)?;
    }

    // 3) Wait for SIM ready. Poll `AT+CPIN?` until +CPIN: READY.
    wait_for_response(&mut io, "AT+CPIN?", "+CPIN: READY", REGISTRATION_TIMEOUT)
        .await
        .map_err(|_| Error::SimLocked)?;

    // 4) Network registration.
    io.cmd("AT+CREG=2").await.ok();
    wait_for_registration(&mut io).await?;

    // 5) PDP context + GPRS attach.
    let mut s: String<128> = String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut s,
        format_args!("AT+CGDCONT=1,\"IP\",\"{}\"", config::APN),
    );
    io.cmd(&s).await.map_err(|_| Error::Cmd("CGDCONT"))?;
    io.cmd("AT+CGATT=1").await.map_err(|_| Error::Cmd("CGATT"))?;

    // 6) Switch to CMUX. Parameters: basic mode, no convergence, k=2, N1=127,
    //    T1=10×10ms, N2=3 retries, T2=30×10ms, T3=10s, k=2.
    io.cmd("AT+CMUX=0,0,5,127,10,3,30,10,2")
        .await
        .map_err(|_| Error::Cmd("CMUX"))?;

    log::info!("modem entered CMUX mode");
    Ok(())
}

/// After CMUX is up and DLC2 SABM has been acknowledged, push DLC2 into PPP
/// data mode and consume the `CONNECT` response. The DLC2 channel is then
/// safe to hand to `embassy_net_ppp::Runner`.
pub async fn start_ppp<U>(dlc2: &mut U) -> Result<(), Error>
where
    U: Read + Write + Unpin,
{
    let mut io = AtIo::new(dlc2);
    // CGDATA does not echo `OK`; success is indicated by `CONNECT`.
    io.write_line("AT+CGDATA=\"PPP\",1")
        .await
        .map_err(|_| Error::Io)?;
    let mut buf = [0u8; LINE_BUF];
    loop {
        let line = with_timeout(COMMAND_TIMEOUT, io.read_line(&mut buf))
            .await
            .map_err(|_| Error::Timeout("CGDATA CONNECT"))?
            .map_err(|_| Error::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("CONNECT") {
            log::info!("PPP CONNECT received on DLC2");
            return Ok(());
        }
        if trimmed.starts_with("ERROR") || trimmed.starts_with("+CME ERROR") {
            return Err(Error::Cmd("CGDATA"));
        }
        log::debug!("ignoring line on DLC2 pre-PPP: {trimmed:?}");
    }
}

// --- AT line I/O helper ------------------------------------------------------

struct AtIo<'a, U> {
    uart: &'a mut U,
    leftover: heapless::Vec<u8, LINE_BUF>,
}

impl<'a, U> AtIo<'a, U>
where
    U: Read + Write + Unpin,
{
    fn new(uart: &'a mut U) -> Self {
        Self {
            uart,
            leftover: heapless::Vec::new(),
        }
    }

    async fn write_line(&mut self, s: &str) -> Result<(), ()> {
        self.uart.write_all(s.as_bytes()).await.map_err(|_| ())?;
        self.uart.write_all(b"\r").await.map_err(|_| ())?;
        Ok(())
    }

    /// Send a command and consume lines until `OK` (success) or `ERROR` /
    /// `+CME ERROR` (failure). Intermediate lines (echo, intermediate
    /// responses, URCs) are logged at debug level.
    async fn cmd(&mut self, command: &str) -> Result<(), Error> {
        self.write_line(command)
            .await
            .map_err(|_| Error::Io)?;
        let mut buf = [0u8; LINE_BUF];
        loop {
            let line = with_timeout(COMMAND_TIMEOUT, self.read_line(&mut buf))
                .await
                .map_err(|_| Error::Timeout("AT cmd"))?
                .map_err(|_| Error::Io)?;
            let trimmed = line.trim();
            match trimmed {
                "OK" => return Ok(()),
                "ERROR" => return Err(Error::Cmd("ERROR")),
                s if s.starts_with("+CME ERROR") => return Err(Error::Cmd("+CME ERROR")),
                "" => {}
                other => {
                    log::debug!("AT pass-through {command:?}: {other:?}");
                }
            }
        }
    }

    /// Read until `\n`, returning the line as a `&str` (without the newline).
    async fn read_line<'b>(&mut self, buf: &'b mut [u8]) -> Result<&'b str, ()> {
        // Move any leftover from a previous call into the caller's buffer.
        let mut len = self.leftover.len();
        if len > 0 {
            buf[..len].copy_from_slice(&self.leftover);
            self.leftover.clear();
        }
        loop {
            if let Some(nl_idx) = buf[..len].iter().position(|&b| b == b'\n') {
                // Extract the line (excluding LF).
                let line_end = nl_idx;
                // Save anything after the LF for the next call.
                let after = &buf[nl_idx + 1..len];
                if !after.is_empty() {
                    self.leftover.extend_from_slice(after).map_err(|_| ())?;
                }
                let line = core::str::from_utf8(&buf[..line_end]).map_err(|_| ())?;
                // Strip a trailing CR if present.
                let line = line.strip_suffix('\r').unwrap_or(line);
                return Ok(line);
            }
            if len == buf.len() {
                return Err(()); // line too long
            }
            let n = self
                .uart
                .read(&mut buf[len..])
                .await
                .map_err(|_| ())?;
            if n == 0 {
                continue;
            }
            len += n;
        }
    }
}

async fn wait_for_response<U>(
    io: &mut AtIo<'_, U>,
    cmd: &str,
    needle: &str,
    timeout: Duration,
) -> Result<(), Error>
where
    U: Read + Write + Unpin,
{
    let deadline = embassy_time::Instant::now() + timeout;
    loop {
        if io.cmd(cmd).await.is_ok() {
            // We can't peek the intermediate lines from cmd() in this minimal
            // impl. Re-issue with manual line read so we can match the needle.
            io.write_line(cmd).await.map_err(|_| Error::Io)?;
            let mut buf = [0u8; LINE_BUF];
            loop {
                let line = with_timeout(COMMAND_TIMEOUT, io.read_line(&mut buf))
                    .await
                    .map_err(|_| Error::Timeout("response"))?
                    .map_err(|_| Error::Io)?;
                let trimmed = line.trim();
                if trimmed == "OK" {
                    break;
                }
                if trimmed.contains(needle) {
                    return Ok(());
                }
            }
        }
        if embassy_time::Instant::now() > deadline {
            return Err(Error::NotRegistered);
        }
        Timer::after(Duration::from_millis(500)).await;
    }
}

async fn wait_for_registration<U>(io: &mut AtIo<'_, U>) -> Result<(), Error>
where
    U: Read + Write + Unpin,
{
    let deadline = embassy_time::Instant::now() + REGISTRATION_TIMEOUT;
    loop {
        // CREG response: "+CREG: <n>,<stat>[,<lac>,<ci>]" — stat 1 = home, 5 = roaming.
        io.write_line("AT+CREG?").await.map_err(|_| Error::Io)?;
        let mut buf = [0u8; LINE_BUF];
        let mut registered = false;
        loop {
            let line = with_timeout(COMMAND_TIMEOUT, io.read_line(&mut buf))
                .await
                .map_err(|_| Error::Timeout("CREG"))?
                .map_err(|_| Error::Io)?;
            let trimmed = line.trim();
            if trimmed == "OK" {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("+CREG:") {
                // Parse the stat field — second comma-separated value.
                let parts: heapless::Vec<&str, 5> =
                    rest.split(',').map(str::trim).collect();
                if parts.len() >= 2 {
                    if let Ok(stat) = parts[1].parse::<u8>() {
                        if stat == 1 || stat == 5 {
                            registered = true;
                        }
                    }
                }
            }
        }
        if registered {
            log::info!("registered to network");
            return Ok(());
        }
        if embassy_time::Instant::now() > deadline {
            return Err(Error::NotRegistered);
        }
        Timer::after(Duration::from_millis(1_000)).await;
    }
}
