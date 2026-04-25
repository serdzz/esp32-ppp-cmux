//! Tiny `BufRead` adapter over our `DlcChannel`.
//!
//! `embassy_sync::pipe::Pipe` only implements `BufRead` via its split
//! `Reader` (which requires `&mut Pipe` to construct), and we hold an
//! immutable `&'static Pipe` from a `StaticCell`. Adding our own buffer is
//! the simplest way to satisfy `embassy_net_ppp::Runner::run`'s
//! `BufRead + Write` bound.

use core::convert::Infallible;

use embedded_io_async::{BufRead, ErrorType, Read, Write};

use crate::net::ppp::PppDlc;

const BUF: usize = 256;

pub struct BufferedDlc {
    inner: PppDlc,
    buf: [u8; BUF],
    valid: usize,
    pos: usize,
}

impl BufferedDlc {
    pub fn new(inner: PppDlc) -> Self {
        Self {
            inner,
            buf: [0; BUF],
            valid: 0,
            pos: 0,
        }
    }
}

impl ErrorType for BufferedDlc {
    type Error = Infallible;
}

impl Read for BufferedDlc {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if self.pos < self.valid {
            let avail = self.valid - self.pos;
            let n = avail.min(buf.len());
            buf[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(buf).await
    }
}

impl BufRead for BufferedDlc {
    async fn fill_buf(&mut self) -> Result<&[u8], Self::Error> {
        if self.pos >= self.valid {
            self.valid = self.inner.read(&mut self.buf).await?;
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..self.valid])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = (self.pos + amt).min(self.valid);
    }
}

impl Write for BufferedDlc {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.inner.write(buf).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush().await
    }
}
