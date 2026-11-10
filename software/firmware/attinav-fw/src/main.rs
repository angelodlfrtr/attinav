//! Attinav Rev B firmware.
//!
//! Task layout (single thread-mode executor; every long computation yields):
//!
//! * `usb_task` / `uart_task` ([`link`]) — framing on both host links.
//! * the engine ([`engine`]) — owns the sensors, flash state and protocol semantics; runs in the
//!   main task after bring-up.
//!
//! Boot sequence: clocks → peripherals and USB → flash state (config + calibration) → carrier UART
//! at the stored baud rate → sensor power-up and identity checks → confirm the image to the
//! bootloader → serve requests.
#![no_std]
#![no_main]
#![allow(
    clippy::future_not_send,
    reason = "single-core thread-mode executor; futures never cross threads"
)]
#![allow(
    clippy::large_futures,
    reason = "tasks are statically allocated by embassy-executor and the main stack is sized for the engine"
)]

mod board;
mod ecc;
mod engine;
mod link;
mod sensors;
mod store;

use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use panic_probe as _;

use attinav_core::calibration::Calibration;
use attinav_core::protocol::{DeviceConfig, FIRMWARE_VERSION};

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_stm32::init(board::clock_config());
    info!(
        "attinav-fw {}.{}.{} starting",
        FIRMWARE_VERSION[0], FIRMWARE_VERSION[1], FIRMWARE_VERSION[2]
    );

    let board = board::init(p, spawner);

    let flash = store::init_flash(board.flash);
    let mut store = store::Store::new(flash);
    let (config, calibration) = match store.load().await {
        Some((config, cal)) if config.is_valid() => (config, cal),
        Some((_, cal)) => {
            info!("stored configuration invalid, using defaults");
            (DeviceConfig::default(), cal)
        }
        None => {
            info!("no stored state, using defaults");
            let config = DeviceConfig::default();
            (config, Calibration::empty(config.mag_cycle_count))
        }
    };
    board::start_uart(board.uart, config.uart_baud, spawner);
    let updater = store::FwUpdate::new(flash);

    engine::run(
        board.sensors,
        board.led,
        board.watchdog,
        store,
        updater,
        config,
        calibration,
    )
    .await
}
