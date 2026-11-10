//! TDK `InvenSense` ICM-42688-P six-axis IMU, SPI interface, bank 0 only.
//!
//! On this board the IMU is a **motion gate**: during a shot its gyroscope tells whether the
//! instrument moved. Nothing here is calibrated or fused. SPI mode 0 or 3, SCK ≤ 24 MHz, register
//! reads set bit 7 of the address.

use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::spi::SpiDevice;

use crate::Error;

/// Bank 0 register addresses.
#[allow(missing_docs)]
pub mod reg {
    pub const DEVICE_CONFIG: u8 = 0x11;
    pub const INT_CONFIG: u8 = 0x14;
    pub const TEMP_DATA1: u8 = 0x1D;
    pub const ACCEL_DATA_X1: u8 = 0x1F;
    pub const GYRO_DATA_X1: u8 = 0x25;
    pub const PWR_MGMT0: u8 = 0x4E;
    pub const GYRO_CONFIG0: u8 = 0x4F;
    pub const ACCEL_CONFIG0: u8 = 0x50;
    pub const INT_CONFIG1: u8 = 0x64;
    pub const INT_SOURCE0: u8 = 0x65;
    pub const WHO_AM_I: u8 = 0x75;
    pub const REG_BANK_SEL: u8 = 0x76;
}

/// Expected `WHO_AM_I`.
pub const WHO_AM_I: u8 = 0x47;
const READ: u8 = 0x80;

/// Gyroscope full scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum GyroScale {
    /// ±2000 °/s, 16.4 LSB/(°/s).
    Dps2000 = 0,
    /// ±1000 °/s, 32.8 LSB/(°/s).
    Dps1000 = 1,
    /// ±500 °/s, 65.5 LSB/(°/s).
    Dps500 = 2,
    /// ±250 °/s, 131 LSB/(°/s).
    Dps250 = 3,
    /// ±125 °/s, 262 LSB/(°/s).
    Dps125 = 4,
}

impl GyroScale {
    /// Sensitivity, LSB per °/s.
    #[must_use]
    pub const fn lsb_per_dps(self) -> f32 {
        match self {
            Self::Dps2000 => 16.4,
            Self::Dps1000 => 32.8,
            Self::Dps500 => 65.5,
            Self::Dps250 => 131.0,
            Self::Dps125 => 262.0,
        }
    }
}

/// Accelerometer full scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum AccelScale {
    /// ±16 g, 2048 LSB/g.
    G16 = 0,
    /// ±8 g, 4096 LSB/g.
    G8 = 1,
    /// ±4 g, 8192 LSB/g.
    G4 = 2,
    /// ±2 g, 16384 LSB/g.
    G2 = 3,
}

impl AccelScale {
    /// Sensitivity, LSB/g.
    #[must_use]
    pub const fn lsb_per_g(self) -> f32 {
        match self {
            Self::G16 => 2048.0,
            Self::G8 => 4096.0,
            Self::G4 => 8192.0,
            Self::G2 => 16384.0,
        }
    }
}

/// Output data rate (shared encoding for gyro and accel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Odr {
    /// 1 kHz
    Hz1000 = 0x06,
    /// 200 Hz
    Hz200 = 0x07,
    /// 100 Hz
    Hz100 = 0x08,
    /// 50 Hz
    Hz50 = 0x09,
    /// 25 Hz
    Hz25 = 0x0A,
}

/// IMU configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Config {
    /// Gyroscope full scale.
    pub gyro_scale: GyroScale,
    /// Gyroscope output data rate.
    pub gyro_odr: Odr,
    /// Accelerometer full scale.
    pub accel_scale: AccelScale,
    /// Accelerometer output data rate.
    pub accel_odr: Odr,
}

impl Default for Config {
    /// Motion-gate defaults: the most sensitive gyro range, 200 Hz.
    fn default() -> Self {
        Self {
            gyro_scale: GyroScale::Dps250,
            gyro_odr: Odr::Hz200,
            accel_scale: AccelScale::G2,
            accel_odr: Odr::Hz200,
        }
    }
}

/// One burst reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Sample {
    /// Raw temperature.
    pub temp: i16,
    /// Accelerometer counts.
    pub accel: [i16; 3],
    /// Gyroscope counts.
    pub gyro: [i16; 3],
}

impl Sample {
    /// Parses the 14 bytes starting at `TEMP_DATA1`.
    #[must_use]
    pub fn from_bytes(b: &[u8; 14]) -> Self {
        let w = |i: usize| i16::from_be_bytes([b[i], b[i + 1]]);
        Self {
            temp: w(0),
            accel: [w(2), w(4), w(6)],
            gyro: [w(8), w(10), w(12)],
        }
    }

    /// Die temperature, °C: `T = raw / 132.48 + 25`.
    #[must_use]
    pub fn temp_celsius(&self) -> f32 {
        f32::from(self.temp) / 132.48 + 25.0
    }

    /// Gyro rates in °/s for the given scale.
    #[must_use]
    pub fn gyro_dps(&self, scale: GyroScale) -> [f32; 3] {
        let s = scale.lsb_per_dps();
        [
            f32::from(self.gyro[0]) / s,
            f32::from(self.gyro[1]) / s,
            f32::from(self.gyro[2]) / s,
        ]
    }

    /// Magnitude of the gyro rate vector, °/s.
    #[must_use]
    pub fn gyro_rate_dps(&self, scale: GyroScale) -> f32 {
        let [x, y, z] = self.gyro_dps(scale);
        libm::sqrtf(x * x + y * y + z * z)
    }
}

/// ICM-42688-P driver.
pub struct Icm42688<SPI, D> {
    spi: SPI,
    delay: D,
    config: Config,
}

impl<SPI: SpiDevice, D: DelayNs> Icm42688<SPI, D> {
    /// Wraps a bus device.
    pub const fn new(spi: SPI, delay: D) -> Self {
        Self {
            spi,
            delay,
            config: Config {
                gyro_scale: GyroScale::Dps250,
                gyro_odr: Odr::Hz200,
                accel_scale: AccelScale::G2,
                accel_odr: Odr::Hz200,
            },
        }
    }

    /// Releases the bus device.
    pub fn release(self) -> (SPI, D) {
        (self.spi, self.delay)
    }

    /// Active configuration.
    #[must_use]
    pub const fn config(&self) -> Config {
        self.config
    }

    async fn write_reg(&mut self, r: u8, v: u8) -> Result<(), Error<SPI::Error>> {
        self.spi.write(&[r & !READ, v]).await?;
        Ok(())
    }

    async fn read_regs(&mut self, start: u8, out: &mut [u8]) -> Result<(), Error<SPI::Error>> {
        let mut tx = [0u8; 16];
        let mut rx = [0u8; 16];
        tx[0] = start | READ;
        let n = out.len() + 1;
        self.spi.transfer(&mut rx[..n], &tx[..n]).await?;
        out.copy_from_slice(&rx[1..n]);
        Ok(())
    }

    /// Soft-resets the device, checks `WHO_AM_I`, applies `config` and powers gyro and accel in
    /// low-noise mode. Takes ≈ 60 ms (reset + gyro start-up).
    pub async fn init(&mut self, config: Config) -> Result<(), Error<SPI::Error>> {
        self.write_reg(reg::DEVICE_CONFIG, 0x01).await?;
        self.delay.delay_ms(2).await;
        let mut id = [0u8; 1];
        self.read_regs(reg::WHO_AM_I, &mut id).await?;
        if id[0] != WHO_AM_I {
            return Err(Error::BadId(id[0]));
        }
        self.write_reg(reg::REG_BANK_SEL, 0).await?;
        self.write_reg(
            reg::GYRO_CONFIG0,
            ((config.gyro_scale as u8) << 5) | config.gyro_odr as u8,
        )
        .await?;
        self.write_reg(
            reg::ACCEL_CONFIG0,
            ((config.accel_scale as u8) << 5) | config.accel_odr as u8,
        )
        .await?;
        // Gyro and accel in low-noise mode; temperature enabled.
        self.write_reg(reg::PWR_MGMT0, 0x0F).await?;
        // Datasheet: no register writes for 200 µs after PWR_MGMT0, gyro needs ~45 ms to start.
        self.delay.delay_ms(50).await;
        self.config = config;
        Ok(())
    }

    /// Reads temperature, accelerometer and gyroscope in one burst.
    pub async fn read(&mut self) -> Result<Sample, Error<SPI::Error>> {
        let mut b = [0u8; 14];
        self.read_regs(reg::TEMP_DATA1, &mut b).await?;
        Ok(Sample::from_bytes(&b))
    }

    /// Turns gyro and accel off (sleep, ≈ 10 µA).
    pub async fn sleep(&mut self) -> Result<(), Error<SPI::Error>> {
        self.write_reg(reg::PWR_MGMT0, 0x00).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal_mock::eh1::delay::NoopDelay;
    use embedded_hal_mock::eh1::spi::{Mock, Transaction};

    fn write(r: u8, v: u8) -> [Transaction<u8>; 3] {
        [
            Transaction::transaction_start(),
            Transaction::write_vec(vec![r, v]),
            Transaction::transaction_end(),
        ]
    }

    #[test]
    fn sample_parsing_and_scaling() {
        let mut b = [0u8; 14];
        b[0..2].copy_from_slice(&0i16.to_be_bytes());
        b[8..10].copy_from_slice(&131i16.to_be_bytes());
        b[10..12].copy_from_slice(&(-262i16).to_be_bytes());
        let s = Sample::from_bytes(&b);
        assert!((s.temp_celsius() - 25.0).abs() < 1e-6);
        let g = s.gyro_dps(GyroScale::Dps250);
        assert!((g[0] - 1.0).abs() < 1e-6 && (g[1] + 2.0).abs() < 1e-6 && g[2] == 0.0);
        assert!((s.gyro_rate_dps(GyroScale::Dps250) - 5f32.sqrt()).abs() < 1e-5);
    }

    #[tokio::test]
    async fn init_and_read() {
        let mut ex = Vec::new();
        ex.extend(write(reg::DEVICE_CONFIG, 0x01));
        ex.extend([
            Transaction::transaction_start(),
            Transaction::transfer(vec![reg::WHO_AM_I | READ, 0], vec![0, WHO_AM_I]),
            Transaction::transaction_end(),
        ]);
        ex.extend(write(reg::REG_BANK_SEL, 0));
        ex.extend(write(reg::GYRO_CONFIG0, (3 << 5) | 0x07));
        ex.extend(write(reg::ACCEL_CONFIG0, (3 << 5) | 0x07));
        ex.extend(write(reg::PWR_MGMT0, 0x0F));
        let mut resp = vec![0u8; 15];
        resp[1..3].copy_from_slice(&1325i16.to_be_bytes());
        resp[9..11].copy_from_slice(&262i16.to_be_bytes());
        let mut tx = vec![0u8; 15];
        tx[0] = reg::TEMP_DATA1 | READ;
        ex.extend([
            Transaction::transaction_start(),
            Transaction::transfer(tx, resp),
            Transaction::transaction_end(),
        ]);
        ex.extend(write(reg::PWR_MGMT0, 0x00));
        let mut dev = Icm42688::new(Mock::new(&ex), NoopDelay);
        dev.init(Config::default()).await.unwrap();
        let s = dev.read().await.unwrap();
        assert_eq!(s.gyro[0], 262);
        assert!((s.temp_celsius() - 35.0).abs() < 0.01);
        dev.sleep().await.unwrap();
        let (mut spi, _) = dev.release();
        spi.done();
    }

    #[tokio::test]
    async fn wrong_id() {
        let mut ex = Vec::new();
        ex.extend(write(reg::DEVICE_CONFIG, 0x01));
        ex.extend([
            Transaction::transaction_start(),
            Transaction::transfer(vec![reg::WHO_AM_I | READ, 0], vec![0, 0x12]),
            Transaction::transaction_end(),
        ]);
        let mut dev = Icm42688::new(Mock::new(&ex), NoopDelay);
        assert_eq!(dev.init(Config::default()).await, Err(Error::BadId(0x12)));
        let (mut spi, _) = dev.release();
        spi.done();
    }
}
