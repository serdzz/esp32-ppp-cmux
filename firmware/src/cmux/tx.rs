//! TX side of the CMUX runtime.
//!
//! Multiple producers (atat on DLC1, embassy-net-ppp on DLC2, the bring-up
//! state machine for SABM/DISC) push `TxReq` messages into a single
//! `Channel`; one task drains it, owns the UART TX half, and serialises
//! every request into a 27.010 frame on the wire.

use cmux_core::{frame, Frame, MAX_INFO_LEN};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embedded_io_async::Write as _;
use esp_hal::{uart::UartTx, Async};
use heapless::Vec;

/// Maximum queued outbound frames. Tuned for AT command bursts plus PPP
/// MRU bursts (~3 frames per IP packet at MTU=127 if HDLC escaping is
/// expensive). Bump if back-pressure on `DlcChannel::write` becomes a real
/// problem.
pub const TX_QUEUE_DEPTH: usize = 16;

/// Message accepted by the TX task. Owned payload — the writer copies its
/// slice into the `Vec` before queueing, which keeps the writer's stack
/// frame free to be dropped while the TX task serialises.
///
/// `Disc` is not currently constructed — kept here so the TX serializer
/// supports orderly channel teardown when the v2 supervisor lands.
#[allow(dead_code)]
pub enum TxReq {
    /// Data on a DLC. Always sent as a UIH-command frame from the host.
    Data {
        dlci: u8,
        info: Vec<u8, MAX_INFO_LEN>,
    },
    /// SABM channel-open command. Caller awaits the matching UA via
    /// [`crate::cmux::dispatcher::ControlEvt`].
    Sabm(u8),
    /// DISC channel-close command.
    Disc(u8),
}

pub type TxChan = Channel<CriticalSectionRawMutex, TxReq, TX_QUEUE_DEPTH>;

#[embassy_executor::task]
pub async fn tx_task(mut uart_tx: UartTx<'static, Async>, requests: &'static TxChan) {
    let mut wire = [0u8; frame::wire_len(MAX_INFO_LEN)];
    loop {
        let req = requests.receive().await;
        let frame = match &req {
            TxReq::Data { dlci, info } => Frame::uih(*dlci, info),
            TxReq::Sabm(dlci) => Frame::sabm(*dlci),
            TxReq::Disc(dlci) => Frame::disc(*dlci),
        };
        let n = match frame::encode(&frame, &mut wire) {
            Ok(n) => n,
            Err(e) => {
                log::error!("CMUX encode error: {e:?}");
                continue;
            }
        };
        if let Err(e) = uart_tx.write_all(&wire[..n]).await {
            log::error!("UART TX error: {e:?}");
        }
    }
}
