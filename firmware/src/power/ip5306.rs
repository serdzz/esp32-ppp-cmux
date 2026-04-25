//! IP5306 PMIC initialisation.
//!
//! Datasheet register quick-ref (only what we need):
//! * `0x00 SYS_CTL0` — bit 4 (BOOST_KEEP_ON) must be 1, else the boost
//!   converter shuts down when load drops below ~45 mA. We additionally set
//!   BOOST_EN | CHARGER_EN | AUTO_PWR_ON | KEY_OFF_EN → final value `0x37`.
//! * `0x01 SYS_CTL1` — boost output current ceiling. `0x80` = 2.1 A,
//!   needed because SIM800L draws ~2 A peaks during 2G TX bursts.

use embedded_hal_async::i2c::I2c;

use crate::board::pmic_i2c;

#[derive(Debug)]
pub enum Error<E> {
    Bus(E),
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Self::Bus(e)
    }
}

/// One-shot bring-up call. Idempotent — safe to retry on failure.
pub async fn keep_boost_on<I>(i2c: &mut I) -> Result<(), Error<I::Error>>
where
    I: I2c,
{
    i2c.write(pmic_i2c::ADDR, &[pmic_i2c::REG_SYS_CTL0, pmic_i2c::VAL_BOOST_KEEP_ON]).await?;
    i2c.write(pmic_i2c::ADDR, &[pmic_i2c::REG_SYS_CTL1, pmic_i2c::VAL_BOOST_2A]).await?;
    log::info!("IP5306 boost-keep-on configured");
    Ok(())
}
