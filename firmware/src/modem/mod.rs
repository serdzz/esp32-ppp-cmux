//! Modem bring-up state machine for SIM800L.
//!
//! Two phases:
//!
//! 1. **Raw-AT** phase ([`bringup::raw_at_init`]) — owns the UART directly,
//!    runs the AT command sequence up to `AT+CMUX=0`, and returns once the
//!    modem has switched into mux mode.
//! 2. **CMUX** phase — caller hands the split UART halves to
//!    [`crate::cmux::start`], gets per-DLC handles back, then calls
//!    [`bringup::start_ppp`] on the PPP DLC to put it into data mode.
//!
//! The `atat` client for routine status (CSQ, CREG, …) is mounted on DLC1
//! after CMUX is up — see [`crate::app::status`] (pending).

pub mod bringup;
