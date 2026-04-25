//! Power orchestration for the SIM800L on TTGO T-Call.
//!
//! Two stages, in order:
//!
//! 1. [`ip5306::keep_boost_on`] — convince the on-board PMIC to hold the
//!    5 V boost rail under light load. SIM800L sleeps between TX bursts and
//!    its idle current is below the IP5306 boost-off threshold; without this
//!    the rail collapses every few seconds and the modem brown-out resets.
//! 2. [`sim800::power_on`] — drive POWER_ON / PWKEY / RST through the
//!    documented timing dance.
//!
//! Awaiting `RDY` on the modem UART is *not* done here — that needs the
//! `Uart` half, which is owned by the modem bring-up state machine.

pub mod ip5306;
pub mod sim800;
