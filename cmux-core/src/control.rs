//! Control octet (3GPP TS 27.010 §5.2.1.3) and the small set of frame
//! kinds we exchange.
//!
//! Constants below are taken straight from the spec / Linux `n_gsm.h`. The
//! P/F bit is bit 4 (mask 0x10): 0 in the base value, 1 in the `_PF` variant.

pub const SABM: u8 = 0x2F;
pub const SABM_PF: u8 = 0x3F;
pub const UA: u8 = 0x63;
pub const UA_PF: u8 = 0x73;
pub const DM: u8 = 0x0F;
pub const DM_PF: u8 = 0x1F;
pub const DISC: u8 = 0x43;
pub const DISC_PF: u8 = 0x53;
pub const UIH: u8 = 0xEF;
pub const UIH_PF: u8 = 0xFF;

/// Mask for the Poll/Final bit inside the control octet.
pub const PF_MASK: u8 = 0x10;

/// Strip the P/F bit so we can match against the canonical opcodes.
#[inline]
pub fn opcode(ctrl: u8) -> u8 {
    ctrl & !PF_MASK
}

/// Whether the P/F bit is set in this control octet.
#[inline]
pub fn pf_set(ctrl: u8) -> bool {
    ctrl & PF_MASK != 0
}

/// High-level classification of a frame.
///
/// Information frames are split between SABM/UA/DM/DISC handshake control
/// frames and UIH data frames. UIH_PF is treated identically to UIH in basic
/// mode — the P/F bit there is informational only.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Sabm,
    Ua,
    Dm,
    Disc,
    Uih,
}

impl FrameKind {
    /// Map a (raw) control byte to a kind, ignoring the P/F bit.
    pub fn from_ctrl(ctrl: u8) -> Option<Self> {
        Some(match opcode(ctrl) {
            SABM => Self::Sabm,
            UA => Self::Ua,
            DM => Self::Dm,
            DISC => Self::Disc,
            UIH => Self::Uih,
            _ => return None,
        })
    }

    /// Build a control octet of this kind, with the supplied P/F bit.
    pub fn to_ctrl(self, pf: bool) -> u8 {
        let base = match self {
            Self::Sabm => SABM,
            Self::Ua => UA,
            Self::Dm => DM,
            Self::Disc => DISC,
            Self::Uih => UIH,
        };
        if pf { base | PF_MASK } else { base }
    }
}

/// Borrowed wrapper used by the higher-level encode path.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub kind: FrameKind,
    pub pf: bool,
}

impl Control {
    pub const fn new(kind: FrameKind, pf: bool) -> Self {
        Self { kind, pf }
    }

    pub fn to_byte(self) -> u8 {
        self.kind.to_ctrl(self.pf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_constants() {
        assert_eq!(SABM_PF, SABM | PF_MASK);
        assert_eq!(UA_PF, UA | PF_MASK);
        assert_eq!(DM_PF, DM | PF_MASK);
        assert_eq!(DISC_PF, DISC | PF_MASK);
        assert_eq!(UIH_PF, UIH | PF_MASK);
    }

    #[test]
    fn round_trip() {
        for kind in [FrameKind::Sabm, FrameKind::Ua, FrameKind::Dm, FrameKind::Disc, FrameKind::Uih] {
            for pf in [false, true] {
                let ctrl = kind.to_ctrl(pf);
                assert_eq!(FrameKind::from_ctrl(ctrl), Some(kind));
                assert_eq!(pf_set(ctrl), pf);
            }
        }
    }

    #[test]
    fn unknown_opcode() {
        assert_eq!(FrameKind::from_ctrl(0x00), None);
    }
}
