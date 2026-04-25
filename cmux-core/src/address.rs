//! Address octet (3GPP TS 27.010 §5.2.1.2).
//!
//! Layout (basic mode, single-byte address — EA = 1):
//!
//! ```text
//!  bit 7 6 5 4 3 2 | 1   | 0
//!       DLCI       | C/R | EA
//! ```

/// Direction-aware role of a frame relative to the link.
///
/// In basic mode, only the initiator role is implemented (we are always the
/// TE/host, the modem is responder). The C/R bit is therefore set per spec
/// table §5.2.1.2.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// Frame is a command (initiator → responder).
    Command,
    /// Frame is a response (responder → initiator).
    Response,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub dlci: u8,
    pub cr: bool,
    pub ea: bool,
}

impl Address {
    /// Build an address byte for a frame the host emits.
    ///
    /// Per spec, when the initiator (host) sends a command the C/R bit is 1;
    /// when it sends a response the C/R bit is 0. `dlci` must be in 0..=63.
    pub fn outgoing(dlci: u8, role: Role) -> Self {
        debug_assert!(dlci < 64, "DLCI must fit in 6 bits");
        Self {
            dlci,
            cr: matches!(role, Role::Command),
            ea: true,
        }
    }

    /// Encode to a single byte.
    pub fn to_byte(self) -> u8 {
        ((self.dlci & 0x3F) << 2) | (u8::from(self.cr) << 1) | u8::from(self.ea)
    }

    /// Parse an address byte. Returns `None` if EA bit is 0 (extended address
    /// not supported in basic mode here).
    pub fn from_byte(b: u8) -> Option<Self> {
        if b & 0x01 == 0 {
            return None;
        }
        Some(Self {
            ea: true,
            cr: (b & 0x02) != 0,
            dlci: (b >> 2) & 0x3F,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for dlci in 0..64u8 {
            for &role in &[Role::Command, Role::Response] {
                let a = Address::outgoing(dlci, role);
                let b = a.to_byte();
                let parsed = Address::from_byte(b).unwrap();
                assert_eq!(parsed, a, "dlci={dlci} role={role:?}");
            }
        }
    }

    #[test]
    fn known_dlci0_command() {
        // DLCI 0, command, EA=1 → 0b0000_0011 = 0x03
        assert_eq!(Address::outgoing(0, Role::Command).to_byte(), 0x03);
    }

    #[test]
    fn known_dlci2_command() {
        // DLCI 2, command, EA=1 → 0b0000_1011 = 0x0B
        assert_eq!(Address::outgoing(2, Role::Command).to_byte(), 0x0B);
    }

    #[test]
    fn rejects_extended_address() {
        assert!(Address::from_byte(0x00).is_none());
    }
}
