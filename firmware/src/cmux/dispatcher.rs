//! RX side of the CMUX runtime.
//!
//! Reads bytes from the modem UART, feeds them to a [`cmux_core::FrameDecoder`],
//! and fans the decoded frames out to:
//! * per-DLC `Pipe` byte streams (consumed by atat / embassy-net-ppp), or
//! * a control `Channel` of [`ControlEvt`] events (consumed by the
//!   bring-up state machine waiting on SABM-acks etc).

use cmux_core::{FrameDecoder, FrameKind, FrameView};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pipe::Pipe;
use embedded_io_async::Read;
use esp_hal::{Async, uart::UartRx};

/// Per-DLC RX buffer sizing. PPP needs headroom for an MRU-1500 IP packet
/// times worst-case HDLC escape ≈ 3 KB; round up to 4 KB. AT traffic is
/// far smaller — 512 B is plenty.
pub const DLC1_PIPE_BYTES: usize = 512;
pub const DLC2_PIPE_BYTES: usize = 4096;

pub type Dlc1Pipe = Pipe<CriticalSectionRawMutex, DLC1_PIPE_BYTES>;
pub type Dlc2Pipe = Pipe<CriticalSectionRawMutex, DLC2_PIPE_BYTES>;

pub struct Sinks {
    pub dlc1: &'static Dlc1Pipe,
    pub dlc2: &'static Dlc2Pipe,
}

/// Events emitted by the dispatcher for the bring-up / supervisor code.
#[derive(Copy, Clone, Debug)]
pub enum ControlEvt {
    /// Peer accepted our SABM on this DLCI (UA received).
    UaReceived(u8),
    /// Peer reported the DLC as disconnected.
    DmReceived(u8),
    /// Peer initiated DISC on this DLCI.
    DiscReceived(u8),
}

pub const CTRL_QUEUE_DEPTH: usize = 8;
pub type CtrlChan = Channel<CriticalSectionRawMutex, ControlEvt, CTRL_QUEUE_DEPTH>;

#[embassy_executor::task]
pub async fn dispatcher_task(
    mut uart_rx: UartRx<'static, Async>,
    sinks: &'static Sinks,
    ctrl: &'static CtrlChan,
) {
    let mut decoder = FrameDecoder::new();
    // Small scratch — the decoder is byte-fed, so this only matters for
    // UART syscall overhead.
    let mut buf = [0u8; 64];

    loop {
        // Disambiguate from UartRx's blocking inherent `read` — we want the
        // async embedded_io trait impl.
        let n = match Read::read(&mut uart_rx, &mut buf).await {
            Ok(0) => continue,
            Ok(n) => n,
            Err(e) => {
                log::error!("modem UART RX error: {e:?}");
                continue;
            }
        };

        for &byte in &buf[..n] {
            match decoder.feed(byte) {
                Ok(None) => {}
                Ok(Some(frame)) => process(&frame, sinks, ctrl).await,
                Err(e) => log::warn!("CMUX decode error: {e:?}"),
            }
        }
    }
}

async fn process(frame: &FrameView<'_>, sinks: &Sinks, ctrl: &CtrlChan) {
    let dlci = frame.dlci();
    let kind = frame.kind();

    match (dlci, kind) {
        // --- Data path -----------------------------------------------------
        (1, FrameKind::Uih) => {
            sinks.dlc1.write_all(frame.info).await;
        }
        (2, FrameKind::Uih) => {
            sinks.dlc2.write_all(frame.info).await;
        }
        (0, FrameKind::Uih) => {
            // DLC0 control payloads (CLD, MSC, PSC) — not actively handled
            // in v1 (no software flow control). Log for now.
            log::debug!("DLC0 control UIH: {:02x?}", frame.info);
        }

        // --- Control handshake --------------------------------------------
        (_, FrameKind::Ua) => emit(ctrl, ControlEvt::UaReceived(dlci)).await,
        (_, FrameKind::Dm) => emit(ctrl, ControlEvt::DmReceived(dlci)).await,
        (_, FrameKind::Disc) => emit(ctrl, ControlEvt::DiscReceived(dlci)).await,

        // SABM from the modem would mean the peer is opening a channel to us
        // — not expected for SIM800L (we are always the initiator). Log.
        (_, FrameKind::Sabm) => {
            log::warn!("unexpected SABM from modem on DLC{dlci}");
        }

        (dlci, FrameKind::Uih) => {
            log::warn!("UIH on unconfigured DLC{dlci}, len={}", frame.info.len());
        }
    }
}

async fn emit(ctrl: &CtrlChan, evt: ControlEvt) {
    if ctrl.try_send(evt).is_err() {
        log::warn!("control event queue full, dropping {evt:?}");
    }
}
