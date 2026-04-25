//! CMUX runtime: dispatcher + TX serializer + per-DLC handles.
//!
//! Public entry point: [`start`]. Call this once after the modem has been
//! switched into mux mode (`AT+CMUX=0,...` returned OK and the UART has
//! been drained). It spawns the dispatcher and TX tasks, opens DLC0/1/2,
//! and returns the user-facing `embedded_io_async` handles.

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pipe::Pipe;
use embassy_time::{with_timeout, Duration};
use esp_hal::{Async, uart::{UartRx, UartTx}};
use static_cell::StaticCell;

pub mod channel;
pub mod dispatcher;
pub mod tx;

use channel::DlcChannel;
use dispatcher::{ControlEvt, CtrlChan, Dlc1Pipe, Dlc2Pipe, Sinks, DLC1_PIPE_BYTES, DLC2_PIPE_BYTES};
use tx::{TxChan, TxReq};

/// User-facing handles after CMUX is up.
pub struct Handles {
    pub dlc1: DlcChannel<CriticalSectionRawMutex, DLC1_PIPE_BYTES>,
    pub dlc2: DlcChannel<CriticalSectionRawMutex, DLC2_PIPE_BYTES>,
}

#[derive(Debug)]
pub enum StartError {
    /// Peer didn't acknowledge SABM within the timeout for this DLCI.
    SabmTimeout(u8),
    /// Peer rejected the channel (DM in response to SABM).
    Rejected(u8),
}

const SABM_TIMEOUT: Duration = Duration::from_millis(2_000);
const SABM_RETRIES: u8 = 3;

/// Spin up the CMUX runtime and open DLC0 (control), DLC1 (AT) and DLC2 (PPP).
pub async fn start(
    spawner: Spawner,
    uart_rx: UartRx<'static, Async>,
    uart_tx: UartTx<'static, Async>,
) -> Result<Handles, StartError> {
    static TX_REQS: StaticCell<TxChan> = StaticCell::new();
    static DLC1_RX: StaticCell<Dlc1Pipe> = StaticCell::new();
    static DLC2_RX: StaticCell<Dlc2Pipe> = StaticCell::new();
    static SINKS: StaticCell<Sinks> = StaticCell::new();
    static CTRL: StaticCell<CtrlChan> = StaticCell::new();

    let tx_reqs: &'static TxChan = TX_REQS.init(Channel::new());
    let dlc1_rx: &'static Dlc1Pipe = DLC1_RX.init(Pipe::new());
    let dlc2_rx: &'static Dlc2Pipe = DLC2_RX.init(Pipe::new());
    let sinks: &'static Sinks = SINKS.init(Sinks { dlc1: dlc1_rx, dlc2: dlc2_rx });
    let ctrl: &'static CtrlChan = CTRL.init(Channel::new());

    spawner
        .spawn(dispatcher::dispatcher_task(uart_rx, sinks, ctrl).unwrap());
    spawner.spawn(tx::tx_task(uart_tx, tx_reqs).unwrap());

    for dlci in [0u8, 1, 2] {
        open_dlc(dlci, tx_reqs, ctrl).await?;
        log::info!("CMUX DLC{dlci} open");
    }

    Ok(Handles {
        dlc1: DlcChannel::new(1, dlc1_rx, tx_reqs),
        dlc2: DlcChannel::new(2, dlc2_rx, tx_reqs),
    })
}

async fn open_dlc(dlci: u8, tx: &'static TxChan, ctrl: &'static CtrlChan) -> Result<(), StartError> {
    for attempt in 0..SABM_RETRIES {
        // Drain any stale UA event from a prior aborted attempt.
        while ctrl.try_receive().is_ok() {}
        tx.send(TxReq::Sabm(dlci)).await;
        match with_timeout(SABM_TIMEOUT, ctrl.receive()).await {
            Ok(ControlEvt::UaReceived(d)) if d == dlci => return Ok(()),
            Ok(ControlEvt::DmReceived(d)) if d == dlci => return Err(StartError::Rejected(dlci)),
            Ok(other) => {
                log::debug!("DLC{dlci} open: ignoring {other:?} on attempt {attempt}");
            }
            Err(_) => {
                log::warn!("DLC{dlci} SABM attempt {attempt} timed out, retrying");
            }
        }
    }
    Err(StartError::SabmTimeout(dlci))
}
