//! `embedded_io_async`-shaped handle for a single CMUX virtual channel.
//!
//! Atat (DLC1) and embassy-net-ppp (DLC2) both consume this trait — the
//! handle hides the per-DLC RX `Pipe` and the shared TX `Channel`.

use core::convert::Infallible;

use cmux_core::MAX_INFO_LEN;
use embassy_sync::pipe::Pipe;
use embedded_io_async::{ErrorType, Read, Write};

use crate::cmux::tx::{TxChan, TxReq};

pub struct DlcChannel<M: embassy_sync::blocking_mutex::raw::RawMutex + 'static, const N: usize> {
    dlci: u8,
    rx: &'static Pipe<M, N>,
    tx: &'static TxChan,
}

impl<M: embassy_sync::blocking_mutex::raw::RawMutex + 'static, const N: usize> DlcChannel<M, N> {
    pub const fn new(dlci: u8, rx: &'static Pipe<M, N>, tx: &'static TxChan) -> Self {
        Self { dlci, rx, tx }
    }

    pub fn dlci(&self) -> u8 {
        self.dlci
    }
}

impl<M: embassy_sync::blocking_mutex::raw::RawMutex + 'static, const N: usize> ErrorType for DlcChannel<M, N> {
    type Error = Infallible;
}

impl<M: embassy_sync::blocking_mutex::raw::RawMutex + 'static, const N: usize> Read for DlcChannel<M, N> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        Ok(self.rx.read(buf).await)
    }
}

impl<M: embassy_sync::blocking_mutex::raw::RawMutex + 'static, const N: usize> Write for DlcChannel<M, N> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        // Chunk into 27.010 basic-mode MTU sized pieces. `chunks` guarantees
        // every slice is ≤ MAX_INFO_LEN, so the Vec push below is safe.
        let mut written = 0;
        for chunk in buf.chunks(MAX_INFO_LEN) {
            let mut info = heapless::Vec::new();
            // SAFETY of unwrap: chunk.len() ≤ MAX_INFO_LEN by construction.
            info.extend_from_slice(chunk).unwrap();
            self.tx.send(TxReq::Data { dlci: self.dlci, info }).await;
            written += chunk.len();
        }
        Ok(written)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        // Fire-and-forget queue model: write() returns once the request is
        // queued. No explicit drain primitive in v1; PPP/TLS tolerate this.
        Ok(())
    }
}
