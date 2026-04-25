//! Periodic AT status task on DLC1.
//!
//! Sends `AT+CSQ` and `AT+CREG?` once per 30 s. Results are logged for now;
//! v2 should publish them through a `PubSubChannel` for the MQTT task to
//! forward.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Timer};
use embedded_io_async::{Read, Write};

use crate::cmux::channel::DlcChannel;
use crate::cmux::dispatcher::DLC1_PIPE_BYTES;

pub type AtDlc = DlcChannel<CriticalSectionRawMutex, DLC1_PIPE_BYTES>;

#[embassy_executor::task]
pub async fn status_task(mut dlc1: AtDlc) {
    // Settle: bring-up may have left URCs in the pipe. Burn them.
    drain(&mut dlc1).await;

    loop {
        if let Err(e) = poll(&mut dlc1, "AT+CSQ").await {
            log::warn!("status: CSQ failed: {e:?}");
        }
        Timer::after(Duration::from_secs(2)).await;
        if let Err(e) = poll(&mut dlc1, "AT+CREG?").await {
            log::warn!("status: CREG failed: {e:?}");
        }
        Timer::after(Duration::from_secs(30)).await;
    }
}

#[derive(Debug)]
enum StatusError {
    Io,
    Timeout,
}

async fn poll(dlc1: &mut AtDlc, cmd: &str) -> Result<(), StatusError> {
    dlc1.write_all(cmd.as_bytes())
        .await
        .map_err(|_| StatusError::Io)?;
    dlc1.write_all(b"\r").await.map_err(|_| StatusError::Io)?;

    let deadline = embassy_time::Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 128];
    let mut acc: heapless::Vec<u8, 256> = heapless::Vec::new();

    loop {
        if embassy_time::Instant::now() > deadline {
            return Err(StatusError::Timeout);
        }
        let n = match embassy_time::with_timeout(Duration::from_millis(500), dlc1.read(&mut buf))
            .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(_)) => return Err(StatusError::Io),
            Err(_) => continue,
        };
        let _ = acc.extend_from_slice(&buf[..n]);

        // Look for any +CSQ:/+CREG: line, then OK terminator.
        if let Ok(s) = core::str::from_utf8(&acc) {
            for line in s.lines().map(str::trim) {
                if line.starts_with('+') {
                    log::info!("status [{cmd}]: {line}");
                }
            }
            if s.contains("\nOK") || s.starts_with("OK") {
                return Ok(());
            }
            if s.contains("\nERROR") || s.starts_with("ERROR") {
                return Err(StatusError::Io);
            }
        }
    }
}

async fn drain(dlc1: &mut AtDlc) {
    let mut buf = [0u8; 64];
    while let Ok(Ok(_n)) =
        embassy_time::with_timeout(Duration::from_millis(50), dlc1.read(&mut buf)).await
    {}
}
