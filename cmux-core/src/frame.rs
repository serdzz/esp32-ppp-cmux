//! Frame encoder and streaming byte-oriented decoder for the basic-mode
//! 27.010 wire format.
//!
//! Wire layout:
//!
//! ```text
//! Flag(0xF9) | Address | Control | Length | Info... | FCS | Flag(0xF9)
//!     1B         1B        1B      1 or 2B   0..=127B  1B    1B
//! ```
//!
//! Two-byte length frames are accepted on encode in spec terms (EA=0), but
//! this implementation deliberately rejects them on decode. The chosen MTU
//! (`MAX_INFO_LEN` = 127) keeps things to a single length octet.

use heapless::Vec;

use crate::address::{Address, Role};
use crate::control::{self, Control, FrameKind};
use crate::{fcs, FLAG, MAX_INFO_LEN};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    InfoTooLong,
    OutputTooSmall,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    BadAddress,
    BadControl,
    BadFcs,
    /// Two-byte length form (EA=0) is not supported in this build.
    TwoByteLengthUnsupported,
    /// Closing flag missing where one was expected.
    MissingEndFlag,
}

/// What the encoder takes as input.
#[derive(Copy, Clone, Debug)]
pub struct Frame<'a> {
    pub dlci: u8,
    pub role: Role,
    pub kind: FrameKind,
    pub pf: bool,
    pub info: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Convenience: SABM with PF=1 and no info, the standard channel-open
    /// command from the host.
    pub const fn sabm(dlci: u8) -> Self {
        Self {
            dlci,
            role: Role::Command,
            kind: FrameKind::Sabm,
            pf: true,
            info: &[],
        }
    }

    /// DISC with PF=1, the standard channel-close command from the host.
    pub const fn disc(dlci: u8) -> Self {
        Self {
            dlci,
            role: Role::Command,
            kind: FrameKind::Disc,
            pf: true,
            info: &[],
        }
    }

    /// UIH (data) frame from the host. C/R = command per §5.4.3.1.
    pub const fn uih(dlci: u8, info: &'a [u8]) -> Self {
        Self {
            dlci,
            role: Role::Command,
            kind: FrameKind::Uih,
            pf: false,
            info,
        }
    }
}

/// Worst-case wire size of one frame for the supplied payload length.
pub const fn wire_len(info_len: usize) -> usize {
    // flag + addr + ctrl + len + info + fcs + flag
    1 + 1 + 1 + 1 + info_len + 1 + 1
}

/// Encode `frame` into `out`. Returns the number of bytes written.
pub fn encode(frame: &Frame<'_>, out: &mut [u8]) -> Result<usize, EncodeError> {
    if frame.info.len() > MAX_INFO_LEN {
        return Err(EncodeError::InfoTooLong);
    }
    let needed = wire_len(frame.info.len());
    if out.len() < needed {
        return Err(EncodeError::OutputTooSmall);
    }
    let addr = Address::outgoing(frame.dlci, frame.role).to_byte();
    let ctrl = Control::new(frame.kind, frame.pf).to_byte();
    let len = ((frame.info.len() as u8) << 1) | 1;

    out[0] = FLAG;
    out[1] = addr;
    out[2] = ctrl;
    out[3] = len;
    let info_end = 4 + frame.info.len();
    out[4..info_end].copy_from_slice(frame.info);
    out[info_end] = fcs::fcs(&[addr, ctrl, len]);
    out[info_end + 1] = FLAG;

    Ok(needed)
}

/// Borrowed view of a successfully decoded frame. Lifetime is tied to the
/// `&mut FrameDecoder` borrow that produced it — caller must consume the
/// view before feeding more bytes.
#[derive(Debug)]
pub struct FrameView<'a> {
    pub address: Address,
    pub control: Control,
    pub info: &'a [u8],
}

impl<'a> FrameView<'a> {
    pub fn dlci(&self) -> u8 {
        self.address.dlci
    }

    pub fn kind(&self) -> FrameKind {
        self.control.kind
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    /// Looking for the first opening flag.
    Hunt,
    /// Last byte was a flag; next non-flag byte is the address.
    Addr,
    Ctrl,
    Len,
    Info,
    Fcs,
    End,
}

pub struct FrameDecoder {
    state: State,
    addr: u8,
    ctrl: u8,
    len_byte: u8,
    info_len: u8,
    info: Vec<u8, MAX_INFO_LEN>,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub const fn new() -> Self {
        Self {
            state: State::Hunt,
            addr: 0,
            ctrl: 0,
            len_byte: 0,
            info_len: 0,
            info: Vec::new(),
        }
    }

    /// Feed one byte from the wire. Returns:
    /// * `Ok(None)` — byte consumed, frame still in progress.
    /// * `Ok(Some(frame))` — a complete frame is available.
    /// * `Err(_)` — framing error; decoder resyncs to Hunt and the next flag.
    pub fn feed(&mut self, b: u8) -> Result<Option<FrameView<'_>>, DecodeError> {
        match self.state {
            State::Hunt => {
                if b == FLAG {
                    self.state = State::Addr;
                }
                Ok(None)
            }
            State::Addr => {
                if b == FLAG {
                    // Repeated flag (inter-frame fill) — consume and stay.
                    return Ok(None);
                }
                if b & 0x01 == 0 {
                    self.reset_to_hunt();
                    return Err(DecodeError::BadAddress);
                }
                self.addr = b;
                self.state = State::Ctrl;
                Ok(None)
            }
            State::Ctrl => {
                if FrameKind::from_ctrl(b).is_none() {
                    self.reset_to_hunt();
                    return Err(DecodeError::BadControl);
                }
                self.ctrl = b;
                self.state = State::Len;
                Ok(None)
            }
            State::Len => {
                if b & 0x01 == 0 {
                    self.reset_to_hunt();
                    return Err(DecodeError::TwoByteLengthUnsupported);
                }
                self.len_byte = b;
                self.info_len = b >> 1;
                self.info.clear();
                self.state = if self.info_len == 0 {
                    State::Fcs
                } else {
                    State::Info
                };
                Ok(None)
            }
            State::Info => {
                // info_len is ≤ 127 = MAX_INFO_LEN, so this never overflows.
                let _ = self.info.push(b);
                if self.info.len() == self.info_len as usize {
                    self.state = State::Fcs;
                }
                Ok(None)
            }
            State::Fcs => {
                let header = [self.addr, self.ctrl, self.len_byte];
                if !fcs::check(&header, b) {
                    self.reset_to_hunt();
                    return Err(DecodeError::BadFcs);
                }
                self.state = State::End;
                Ok(None)
            }
            State::End => {
                if b != FLAG {
                    self.reset_to_hunt();
                    return Err(DecodeError::MissingEndFlag);
                }
                // Stay in sync for the next frame; closing flag may also be
                // the next frame's opening flag.
                self.state = State::Addr;
                Ok(Some(self.view()))
            }
        }
    }

    fn reset_to_hunt(&mut self) {
        self.state = State::Hunt;
        self.info.clear();
    }

    fn view(&self) -> FrameView<'_> {
        // SAFETY of unwraps: validated in Addr/Ctrl states above.
        let address = Address::from_byte(self.addr).expect("validated address");
        let kind = FrameKind::from_ctrl(self.ctrl).expect("validated control");
        let pf = control::pf_set(self.ctrl);
        FrameView {
            address,
            control: Control { kind, pf },
            info: &self.info,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame: Frame<'_>) -> (Vec<u8, 256>, FrameKind, u8, heapless::Vec<u8, 127>) {
        let mut buf = [0u8; 256];
        let n = encode(&frame, &mut buf).expect("encode");
        let mut wire = Vec::<u8, 256>::new();
        wire.extend_from_slice(&buf[..n]).unwrap();

        let mut dec = FrameDecoder::new();
        let mut info = heapless::Vec::<u8, 127>::new();
        let mut kind = None;
        let mut dlci = 0;
        for &b in &wire {
            if let Some(f) = dec.feed(b).expect("feed") {
                kind = Some(f.kind());
                dlci = f.dlci();
                info.extend_from_slice(f.info).unwrap();
            }
        }
        (wire, kind.expect("frame produced"), dlci, info)
    }

    #[test]
    fn sabm_round_trip() {
        let f = Frame::sabm(0);
        let (_, kind, dlci, info) = round_trip(f);
        assert_eq!(kind, FrameKind::Sabm);
        assert_eq!(dlci, 0);
        assert!(info.is_empty());
    }

    #[test]
    fn uih_round_trip_payload() {
        let payload: heapless::Vec<u8, 127> = (0..120u8).collect();
        let f = Frame::uih(2, &payload);
        let (_, kind, dlci, info) = round_trip(f);
        assert_eq!(kind, FrameKind::Uih);
        assert_eq!(dlci, 2);
        assert_eq!(info.as_slice(), payload.as_slice());
    }

    #[test]
    fn rejects_bad_fcs() {
        let mut buf = [0u8; 32];
        let n = encode(&Frame::sabm(0), &mut buf).unwrap();
        // Flip a bit in the FCS byte (second-to-last).
        buf[n - 2] ^= 0x01;

        let mut dec = FrameDecoder::new();
        let mut err = None;
        for &b in &buf[..n] {
            if let Err(e) = dec.feed(b) {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(DecodeError::BadFcs));
    }

    #[test]
    fn resyncs_after_garbage() {
        let mut buf = [0u8; 32];
        let n = encode(&Frame::sabm(0), &mut buf).unwrap();
        let mut dec = FrameDecoder::new();
        // Prepend random non-flag noise — decoder should sit in Hunt then
        // sync on the leading flag and decode the frame.
        let noise = [0xAA, 0x55, 0x00, 0xFF, 0x12];
        let mut produced = false;
        for &b in noise.iter().chain(&buf[..n]) {
            if dec.feed(b).expect("feed").is_some() {
                produced = true;
            }
        }
        assert!(produced);
    }

    #[test]
    fn back_to_back_frames_share_flag() {
        // Two frames, second starts immediately after first's closing flag
        // (no extra flag between them — the closing flag IS the opener).
        let mut buf = [0u8; 64];
        let n1 = encode(&Frame::sabm(1), &mut buf).unwrap();
        let n2 = encode(&Frame::sabm(2), &mut buf[n1 - 1..]).unwrap();
        let total = n1 - 1 + n2;

        let mut dec = FrameDecoder::new();
        let mut count = 0;
        let mut last_dlci = 0;
        for &b in &buf[..total] {
            if let Some(f) = dec.feed(b).expect("feed") {
                count += 1;
                last_dlci = f.dlci();
            }
        }
        assert_eq!(count, 2);
        assert_eq!(last_dlci, 2);
    }

    #[test]
    fn rejects_two_byte_length() {
        // Hand-craft: flag, addr=0x03, ctrl=UIH, len byte EA=0 (=0x02), ...
        let bytes = [FLAG, 0x03, 0xEF, 0x02, 0x00, 0x00, FLAG];
        let mut dec = FrameDecoder::new();
        let mut err = None;
        for &b in &bytes {
            if let Err(e) = dec.feed(b) {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(DecodeError::TwoByteLengthUnsupported));
    }

    use proptest::prelude::*;

    fn arb_kind() -> impl Strategy<Value = FrameKind> {
        prop_oneof![
            Just(FrameKind::Sabm),
            Just(FrameKind::Ua),
            Just(FrameKind::Dm),
            Just(FrameKind::Disc),
            Just(FrameKind::Uih),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1024, .. ProptestConfig::default() })]

        #[test]
        fn proptest_uih_round_trip(
            dlci in 0u8..64,
            payload in proptest::collection::vec(any::<u8>(), 0..=127),
        ) {
            let frame = Frame::uih(dlci, &payload);
            let mut buf = [0u8; 256];
            let n = encode(&frame, &mut buf).unwrap();
            let mut dec = FrameDecoder::new();
            let mut got = None;
            for &b in &buf[..n] {
                if let Some(f) = dec.feed(b).unwrap() {
                    got = Some((f.dlci(), f.kind(), heapless::Vec::<u8, 127>::from_slice(f.info).unwrap()));
                }
            }
            let (got_dlci, got_kind, got_info) = got.unwrap();
            prop_assert_eq!(got_dlci, dlci);
            prop_assert_eq!(got_kind, FrameKind::Uih);
            prop_assert_eq!(got_info.as_slice(), payload.as_slice());
        }

        #[test]
        fn proptest_handshake_round_trip(
            dlci in 0u8..64,
            kind in arb_kind(),
            pf in any::<bool>(),
        ) {
            let frame = Frame { dlci, role: Role::Command, kind, pf, info: &[] };
            let mut buf = [0u8; 16];
            let n = encode(&frame, &mut buf).unwrap();
            let mut dec = FrameDecoder::new();
            let mut got = None;
            for &b in &buf[..n] {
                if let Some(f) = dec.feed(b).unwrap() {
                    got = Some((f.dlci(), f.kind(), f.control.pf, f.info.len()));
                }
            }
            prop_assert_eq!(got, Some((dlci, kind, pf, 0)));
        }

        #[test]
        fn proptest_random_garbage_never_panics(stream in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let mut dec = FrameDecoder::new();
            for b in stream {
                let _ = dec.feed(b);
            }
        }
    }
}
