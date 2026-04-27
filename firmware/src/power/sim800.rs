//! SIM800L power / reset GPIO sequencing.
//!
//! T-Call wiring (active polarities in [`crate::board::modem_power`]):
//! * `POWER_ON` — gates the 5 V rail from IP5306 to VBAT_RF on the modem.
//! * `PWKEY` — pulled LOW for ≥ 1.0 s to power the modem on. Idle HIGH.
//! * `RST` — active-LOW hardware reset. Idle HIGH.
//!
//! Datasheet timings:
//! * PWKEY hold: 1.0 s typical, < 1.2 s (otherwise treated as power-off).
//! * Boot to first `RDY` URC: ≈ 3 s after PWKEY release.
//! * Hard reset pulse: ≥ 105 ms low.
//!
//! Caller awaits `RDY` (or follow-up `+CPIN: READY` etc.) on the UART
//! separately — that half is owned by the modem bring-up code.

use embassy_time::{Duration, Timer};
use esp_hal::gpio::Output;

/// The three GPIOs that gate modem power. Caller constructs them once from
/// the peripherals struct and hands the bundle here.
pub struct PowerPins<'a> {
    pub power_on: Output<'a>,
    pub pwkey: Output<'a>,
    pub rst: Output<'a>,
}

/// Drive the documented power-on sequence. Returns once PWKEY has been
/// released; the modem will print boot URCs on the UART over the next ~5 s.
///
/// Does *not* attempt to detect a stuck-off modem on its own — if the RDY
/// URC doesn't arrive, call [`hardware_reset`] from the bring-up state
/// machine and retry.
pub async fn power_on(pins: &mut PowerPins<'_>) {
    pins.rst.set_high();
    pins.pwkey.set_high();
    pins.power_on.set_high();

    Timer::after(Duration::from_millis(100)).await;

    pins.pwkey.set_low();
    Timer::after(Duration::from_millis(1100)).await;
    pins.pwkey.set_high();

    log::info!("SIM800 PWKEY released; awaiting RDY URC on modem UART");
}

/// Power-on guarded by an "is the modem already responding to AT?" probe.
///
/// If the ESP32 brown-out resets while the modem is up (very common on
/// T-Call when running on USB-only with a weak 18650), naively re-running
/// `power_on()` would hold PWKEY low for 1.1 s on a *running* modem — that
/// counts as a power-off command and bricks the bring-up.
///
/// `probe_alive` should write `AT\r` and return `true` if `OK` comes back
/// within ~500 ms. When it returns `true`, this function leaves the modem
/// alone (and just ensures POWER_ON gate is HIGH).
pub async fn power_on_if_needed<F, Fut>(pins: &mut PowerPins<'_>, probe_alive: F)
where
    F: FnOnce() -> Fut,
    Fut: core::future::Future<Output = bool>,
{
    pins.rst.set_high();
    pins.power_on.set_high();
    Timer::after(Duration::from_millis(100)).await;

    if probe_alive().await {
        log::info!("SIM800 already responding to AT, skipping PWKEY pulse");
        return;
    }
    log::info!("SIM800 silent — issuing PWKEY power-on pulse");
    pins.pwkey.set_high();
    Timer::after(Duration::from_millis(50)).await;
    pins.pwkey.set_low();
    Timer::after(Duration::from_millis(1100)).await;
    pins.pwkey.set_high();
    log::info!("SIM800 PWKEY released; awaiting RDY URC on modem UART");
}

/// Drop RST low for the documented ≥ 105 ms, then release. Modem boots
/// fresh; caller should re-await `RDY` afterwards. Reserved for the v2
/// supervisor that retries bring-up on PPP drops.
#[allow(dead_code)]
pub async fn hardware_reset(pins: &mut PowerPins<'_>) {
    pins.rst.set_low();
    Timer::after(Duration::from_millis(110)).await;
    pins.rst.set_high();
    log::warn!("SIM800 hardware reset issued");
}
