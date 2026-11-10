//! Async `embedded-hal` drivers for the Attinav sensors.
//!
//! All drivers take an [`embedded_hal_async::spi::SpiDevice`] (bus sharing and chip-select are the
//! HAL's business) and, where start-up timing matters, an [`embedded_hal_async::delay::DelayNs`].
//! They are `no_std`, allocation-free and tested on the host against `embedded-hal-mock`.
//!
//! * [`rm3100`]   — PNI RM3100 `MagI2C` magnetometer (SPI, 24-bit, cycle-count gain).
//! * [`sca3300`]  — Murata SCA3300-D01 accelerometer / inclinometer (32-bit SPI frames with CRC-8).
//! * [`icm42688`] — TDK ICM-42688-P 6-axis IMU, used only as a motion gate.
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![warn(missing_docs)]

pub mod icm42688;
pub mod rm3100;
pub mod sca3300;

/// Error shared by the drivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// Bus error from the HAL.
    Spi(E),
    /// Identity register did not match (holds the value read).
    BadId(u8),
    /// A frame failed its integrity check (SCA3300 CRC).
    Crc,
    /// A response answered a different command than expected (SCA3300 pipeline out of sync).
    Protocol,
    /// The device reported an error or stayed in start-up (holds the raw status field).
    Status(u8),
    /// A parameter was out of the device's range.
    InvalidArgument,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Self::Spi(e)
    }
}
