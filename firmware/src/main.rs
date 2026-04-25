#![no_std]
#![no_main]

mod app;
mod board;
mod cmux;
mod config;
mod logger;
mod modem;
mod net;
mod power;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    time::Rate,
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, Uart},
};
use esp_println as _;

use crate::power::sim800::PowerPins;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    logger::init();
    log::info!("esp32-ppp-cmux booting");

    // rust-mqtt + embedded-tls pull in `alloc` — install global allocator
    // before any `Box`/`Vec` allocation in dependent crates.
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // ---------------- Power: IP5306 PMIC, then SIM800L power-on ------------
    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C0")
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22)
    .into_async();
    if let Err(e) = power::ip5306::keep_boost_on(&mut i2c).await {
        log::error!("IP5306 init failed: {e:?} — modem rail may be unstable");
    }

    let mut power_pins = PowerPins {
        power_on: Output::new(peripherals.GPIO23, Level::Low, OutputConfig::default()),
        pwkey: Output::new(peripherals.GPIO4, Level::High, OutputConfig::default()),
        rst: Output::new(peripherals.GPIO5, Level::High, OutputConfig::default()),
    };
    power::sim800::power_on(&mut power_pins).await;

    // ---------------- Modem UART (single port, no flow control) -----------
    let uart_cfg = UartConfig::default().with_baudrate(board::modem_uart::BAUD);
    let mut uart = Uart::new(peripherals.UART1, uart_cfg)
        .expect("UART1")
        .with_tx(peripherals.GPIO26)
        .with_rx(peripherals.GPIO27)
        .into_async();

    // Wait a beat for SIM800L to print boot URCs (RDY, +CFUN: 1, etc.).
    Timer::after(Duration::from_secs(4)).await;

    spawner.spawn(heartbeat().unwrap());

    // ---------------- Raw AT init -> CMUX entry ---------------------------
    if let Err(e) = modem::bringup::raw_at_init(&mut uart).await {
        log::error!("modem raw AT init failed: {e:?}");
        idle_panic_loop().await;
    }

    // After OK to AT+CMUX=0,..., drain UART for ~50 ms before handing it
    // over — any non-frame byte breaks the multiplexer.
    drain_uart(&mut uart, Duration::from_millis(50)).await;

    let (uart_rx, uart_tx) = uart.split();

    let mut handles = match cmux::start(spawner, uart_rx, uart_tx).await {
        Ok(h) => h,
        Err(e) => {
            log::error!("CMUX setup failed: {e:?}");
            idle_panic_loop().await;
        }
    };

    // ---------------- PPP on DLC2 -----------------------------------------
    if let Err(e) = modem::bringup::start_ppp(&mut handles.dlc2).await {
        log::error!("CGDATA PPP failed: {e:?}");
        idle_panic_loop().await;
    }

    let net = net::start(spawner, handles.dlc2);
    log::info!("net stack started, waiting for IPCP");

    // ---------------- Application tasks -----------------------------------
    spawner.spawn(app::status::status_task(handles.dlc1).unwrap());
    spawner.spawn(app::mqtt::mqtt_task(net.stack).unwrap());

    log::info!("bring-up complete");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[embassy_executor::task]
async fn heartbeat() {
    let mut tick = 0u32;
    loop {
        Timer::after(Duration::from_secs(5)).await;
        log::debug!("heartbeat #{tick}");
        tick = tick.wrapping_add(1);
    }
}

async fn drain_uart<U>(uart: &mut U, window: Duration)
where
    U: embedded_io_async::Read + Unpin,
{
    let mut scratch = [0u8; 64];
    let _ = embassy_time::with_timeout(window, async {
        loop {
            // ignore both Ok(0) and errors during a short drain
            let _ = uart.read(&mut scratch).await;
        }
    })
    .await;
}

async fn idle_panic_loop() -> ! {
    log::error!("entering idle loop — fix the failing init step and reboot");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
