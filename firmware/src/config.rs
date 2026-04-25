//! Compile-time configuration sourced from the environment via build.rs.
//!
//! Keep this file the single point of contact for env values: every other
//! module imports concrete `&'static str` constants from here. Some
//! constants are only consumed by the v2 MQTT framing wiring; allow
//! dead-code while that's still TODO.

#![allow(dead_code)]

pub const APN: &str = env!("APN");
pub const GPRS_USER: &str = env!("GPRS_USER");
pub const GPRS_PASS: &str = env!("GPRS_PASS");
pub const SIM_PIN: &str = env!("SIM_PIN");

pub const MQTT_HOST: &str = env!("MQTT_HOST");
pub const MQTT_PORT_STR: &str = env!("MQTT_PORT");
pub const MQTT_CLIENT_ID: &str = env!("MQTT_CLIENT_ID");
pub const MQTT_USER: &str = env!("MQTT_USER");
pub const MQTT_PASS: &str = env!("MQTT_PASS");
pub const MQTT_DNS: &str = env!("MQTT_DNS");

/// Broker root CA, loaded as bytes at compile time. The actual path is
/// resolved by build.rs (copied into OUT_DIR) so first builds succeed even
/// without a real cert in place.
pub const MQTT_CA_PEM: &[u8] = include_bytes!(env!("MQTT_CA_PEM_PATH"));

/// Parse `MQTT_PORT_STR` into `u16`. Falls back to 8883 (TLS) if env was
/// empty so the firmware doesn't panic on first boot.
pub fn mqtt_port() -> u16 {
    MQTT_PORT_STR.parse().unwrap_or(8883)
}
