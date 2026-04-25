#![cfg_attr(not(test), no_std)]

//! 3GPP TS 27.010 basic-mode multiplexer framing.
//!
//! This crate is intentionally I/O-free: it parses and emits frames as byte
//! buffers. The firmware crate wires it to an async UART and a per-DLC pipe.
//!
//! Only the subset needed for SIM800L bring-up + PPP is implemented:
//! basic mode, single-byte length (MTU = 127), no advanced options, no
//! convergence layer, no software flow control (MSC) negotiation.

pub mod address;
pub mod control;
pub mod fcs;
pub mod frame;
pub mod state;

pub use address::Address;
pub use control::{Control, FrameKind};
pub use frame::{DecodeError, EncodeError, Frame, FrameDecoder, FrameView};

/// Frame delimiter. Spec calls it "flag", value 0xF9.
pub const FLAG: u8 = 0xF9;

/// Maximum payload length in basic mode with a single-byte length field.
pub const MAX_INFO_LEN: usize = 127;

/// DLCI 0 carries the multiplexer control channel (per spec §5.4.6).
pub const DLCI_CONTROL: u8 = 0;
