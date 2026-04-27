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
/// First-attach to 2G after a cold start can take 60-120 s while the modem
/// scans bands and selects a cell. 180 s gives a comfortable margin.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(180);

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

    // Soft-reset radio to a known-clean state. Mirrors TinyGSM's restart
    // sequence — without this, leftover URCs / pending PDP context from a
    // previous boot interleave with our query/response loop and confuse the
    // parser. CFUN=1,1 reboots; modem will print RDY again. Allow generous
    // timeouts.
    io.cmd_with_timeout("AT+CFUN=0", Duration::from_secs(10))
        .await
        .ok(); // best-effort; some firmware revisions reject CFUN=0
    io.cmd_with_timeout("AT+CFUN=1,1", Duration::from_secs(10))
        .await
        .ok();
    Timer::after(Duration::from_secs(5)).await;
    // After reboot the modem prints RDY/+CPIN: READY/Call Ready URCs.
    // Re-issue ATE0/CMEE; the previous settings are lost on warm boot.
    for _ in 0..5 {
        if io.cmd("AT").await.is_ok() {
            break;
        }
        Timer::after(Duration::from_millis(200)).await;
    }
    io.cmd("ATE0").await.map_err(|_| Error::Cmd("ATE0 post-CFUN"))?;
    io.cmd("AT+CMEE=2").await.ok();

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
    //
    // CGDCONT/CGATT failures are non-fatal here: the modem may reject GPRS
    // attach with an empty APN (Tele2 LV needs `internet.tele2.lv`, etc.),
    // but `AT+CGDATA="PPP",1` on DLC2 in start_ppp() will run its own
    // attach attempt with the PPP-supplied PAP credentials. Logging the
    // failure tells the user to fix their APN, but doesn't kill bring-up.
    let mut s: String<128> = String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut s,
        format_args!("AT+CGDCONT=1,\"IP\",\"{}\"", config::APN),
    );
    if let Err(e) = io.cmd(&s).await {
        log::warn!("CGDCONT failed (continuing): {e:?}");
    }
    if let Err(e) = io.cmd_with_timeout("AT+CGATT=1", Duration::from_secs(15)).await {
        log::warn!("CGATT failed (continuing — PPP will retry): {e:?}");
    }

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
        self.cmd_with_timeout(command, COMMAND_TIMEOUT).await
    }

    async fn cmd_with_timeout(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<(), Error> {
        self.write_line(command).await.map_err(|_| Error::Io)?;
        let mut buf = [0u8; LINE_BUF];
        loop {
            let line = with_timeout(timeout, self.read_line(&mut buf))
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

    /// Best-effort: read+discard every byte the modem has buffered for us.
    /// Useful between phases to ensure stale URCs don't poison the next
    /// query/response loop.
    async fn drain(&mut self, window: Duration) {
        self.leftover.clear();
        let mut buf = [0u8; LINE_BUF];
        let _ = with_timeout(window, async {
            loop {
                if self.uart.read(&mut buf).await.is_err() {
                    return;
                }
            }
        })
        .await;
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
            let n = self.uart.read(&mut buf[len..]).await.map_err(|_| ())?;
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
    // Burn any URCs queued from earlier phases (RDY, +CFUN: 1, Call Ready,
    // SMS Ready, +CPIN: READY, ...). Otherwise the first query/response
    // loop reads them as command output and the parser desyncs.
    io.drain(Duration::from_millis(200)).await;
    let deadline = embassy_time::Instant::now() + REGISTRATION_TIMEOUT;
    let mut tick: u32 = 0;
    loop {
        // CREG (CS) and CGREG (GPRS): "+(C)REG: <n>,<stat>[,<lac>,<ci>]"
        // stat 1 = registered home, 5 = registered roaming.
        // Tele2 LV (and many M2M-tuned networks) sometimes only attach GPRS,
        // leaving CREG=0 while CGREG=1 — accept either.
        let creg = query_reg(io, "AT+CREG?", "+CREG:").await.unwrap_or(255);
        let cgreg = query_reg(io, "AT+CGREG?", "+CGREG:").await.unwrap_or(255);
        let csq = query_csq(io).await.ok();
        log::info!("registration tick {tick}: CSQ={csq:?} CREG={creg} CGREG={cgreg}");
        let registered = matches!(creg, 1 | 5) || matches!(cgreg, 1 | 5);
        if registered {
            if let Ok(op) = query_cops(io).await {
                log::info!("registered: operator={op:?}");
            } else {
                log::info!("registered to network");
            }
            return Ok(());
        }
        if embassy_time::Instant::now() > deadline {
            return Err(Error::NotRegistered);
        }
        Timer::after(Duration::from_secs(5)).await;
        tick = tick.wrapping_add(1);
    }
}

/// Issue a `AT+CREG?` / `AT+CGREG?` query, return the `<stat>` field.
///
/// Tolerates both query response (`+(C)REG: <n>,<stat>[,...]`, 2+ fields)
/// and the URC variant (`+(C)REG: <stat>`, 1 field) — Tele2 and many
/// other operators send the URC right after `AT+CREG=2` is enabled, and
/// it can race the query response.
async fn query_reg<U>(io: &mut AtIo<'_, U>, cmd: &str, prefix: &str) -> Result<u8, Error>
where
    U: Read + Write + Unpin,
{
    io.write_line(cmd).await.map_err(|_| Error::Io)?;
    let mut buf = [0u8; LINE_BUF];
    let mut stat: Option<u8> = None;
    loop {
        let line = match with_timeout(COMMAND_TIMEOUT, io.read_line(&mut buf)).await {
            Ok(Ok(l)) => l,
            Ok(Err(_)) => {
                log::warn!("{cmd}: read_line io error");
                return Err(Error::Io);
            }
            Err(_) => {
                log::warn!("{cmd}: timeout");
                return Err(Error::Timeout("reg query"));
            }
        };
        let trimmed = line.trim();
        if trimmed == "OK" {
            return stat.ok_or_else(|| {
                log::warn!("{cmd}: OK but no parseable {prefix} line");
                Error::Cmd("no reg line")
            });
        }
        if trimmed.starts_with("ERROR") || trimmed.starts_with("+CME ERROR") {
            log::warn!("{cmd}: {trimmed}");
            return Err(Error::Cmd("reg ERROR"));
        }
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let parts: heapless::Vec<&str, 5> = rest.split(',').map(str::trim).collect();
            // Query response: <n>,<stat>[,<lac>,<ci>] — stat is parts[1]
            // URC: <stat> — stat is parts[0]
            let stat_field = if parts.len() >= 2 { parts[1] } else { parts[0] };
            stat = stat_field.parse::<u8>().ok();
        }
    }
}

/// `AT+CSQ` → (rssi, ber). rssi 0..=31 (99 = unknown), ber 0..=7.
async fn query_csq<U>(io: &mut AtIo<'_, U>) -> Result<(u8, u8), Error>
where
    U: Read + Write + Unpin,
{
    io.write_line("AT+CSQ").await.map_err(|_| Error::Io)?;
    let mut buf = [0u8; LINE_BUF];
    let mut out: Option<(u8, u8)> = None;
    loop {
        let line = with_timeout(COMMAND_TIMEOUT, io.read_line(&mut buf))
            .await
            .map_err(|_| Error::Timeout("CSQ"))?
            .map_err(|_| Error::Io)?;
        let trimmed = line.trim();
        if trimmed == "OK" {
            return out.ok_or(Error::Cmd("no CSQ line"));
        }
        if let Some(rest) = trimmed.strip_prefix("+CSQ:") {
            let parts: heapless::Vec<&str, 2> = rest.split(',').map(str::trim).collect();
            if parts.len() == 2 {
                if let (Ok(r), Ok(b)) = (parts[0].parse(), parts[1].parse()) {
                    out = Some((r, b));
                }
            }
        }
    }
}

/// `AT+COPS?` → operator name (best-effort).
async fn query_cops<U>(io: &mut AtIo<'_, U>) -> Result<heapless::String<32>, Error>
where
    U: Read + Write + Unpin,
{
    io.write_line("AT+COPS?").await.map_err(|_| Error::Io)?;
    let mut buf = [0u8; LINE_BUF];
    let mut name: heapless::String<32> = heapless::String::new();
    loop {
        let line = with_timeout(COMMAND_TIMEOUT, io.read_line(&mut buf))
            .await
            .map_err(|_| Error::Timeout("COPS"))?
            .map_err(|_| Error::Io)?;
        let trimmed = line.trim();
        if trimmed == "OK" {
            return Ok(name);
        }
        if let Some(rest) = trimmed.strip_prefix("+COPS:") {
            // +COPS: <mode>,<format>,"<oper>"[,<AcT>]
            if let Some(start) = rest.find('"') {
                if let Some(end) = rest[start + 1..].find('"') {
                    let _ = name.push_str(&rest[start + 1..start + 1 + end]);
                }
            }
        }
    }
}
