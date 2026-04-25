//! Wires `log` macros into `esp-println`'s UART writer.
//!
//! Boot-time call: `logger::init()` from `main` before spawning anything that
//! logs. Log level comes from the `ESP_LOG` env var at build time (set in
//! `.cargo/config.toml`).

pub fn init() {
    // esp-println's helper installs a global `log` implementation that
    // forwards to its writer (UART0 / USB-serial-jtag depending on chip).
    esp_println::logger::init_logger_from_env();
}
