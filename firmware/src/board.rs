//! TTGO T-Call v1.x pin map.
//!
//! Constants and a tiny `take_peripherals` helper. Anything that touches a
//! specific GPIO/peripheral on this board imports from here so a future
//! board variant can be slotted in without grepping the codebase.

/// SIM800L UART (single port to the modem; no HW flow control on T-Call).
pub mod modem_uart {
    /// ESP32 UART RX ← SIM800L TX
    pub const RX_GPIO: u8 = 27;
    /// ESP32 UART TX → SIM800L RX
    pub const TX_GPIO: u8 = 26;
    pub const BAUD: u32 = 115_200;
}

/// SIM800L power / reset control pins (active polarity in comments).
pub mod modem_power {
    /// Gate that connects the IP5306 boost rail to VBAT_RF on the modem.
    /// Active HIGH on T-Call v1.x.
    pub const POWER_ON_GPIO: u8 = 23;
    /// PWKEY: pulled LOW for ≥1.0s to power the modem on. Idle HIGH.
    pub const PWKEY_GPIO: u8 = 4;
    /// Hardware reset, active LOW. Idle HIGH.
    pub const RST_GPIO: u8 = 5;
}

/// IP5306 PMIC sits on the same I2C bus the SoC uses for the on-board PMIC
/// register dance. Without setting `boost-keep-on`, the modem rail collapses
/// when current draw drops momentarily and the modem resets.
pub mod pmic_i2c {
    pub const SDA_GPIO: u8 = 21;
    pub const SCL_GPIO: u8 = 22;
    pub const ADDR: u8 = 0x75;
    /// SYS_CTL0 register.
    pub const REG_SYS_CTL0: u8 = 0x00;
    /// 0x37: BOOST_EN | CHARGER_EN | AUTO_PWR_ON | KEY_OFF_EN | BOOST_KEEP_ON.
    pub const VAL_BOOST_KEEP_ON: u8 = 0x37;
    /// SYS_CTL1.
    pub const REG_SYS_CTL1: u8 = 0x01;
    /// 0x80: bump boost output current ceiling so SIM800L TX bursts (~2A peak)
    /// don't brown-out the rail.
    pub const VAL_BOOST_2A: u8 = 0x80;
}
