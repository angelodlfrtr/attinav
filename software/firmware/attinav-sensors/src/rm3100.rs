//! PNI RM3100 three-axis magneto-inductive magnetometer, SPI interface.
//!
//! Protocol: the first byte of every transaction is the register address with bit 7 set for a
//! read; consecutive bytes address consecutive registers. SPI mode 0, SCK ≤ 1 MHz.
//!
//! Measurement: each axis is a 24-bit two's complement count whose gain grows with the *cycle
//! count* (≈ 0.3671 · CC + 1.5 LSB/µT, 75 LSB/µT at the default 200). Higher cycle counts cost
//! time: the maximum three-axis rate is ≈ 29 000 / CC Hz (≈ 73 Hz at CC = 400).
//!
//! Two acquisition modes are supported: single polled measurements ([`Rm3100::start_single`], the
//! shot engine's choice, deterministic and overrun-free) and continuous measurement at a TMRC rate
//! ([`Rm3100::start_continuous`], used for streaming). In both, `DRDY` goes high when the three
//! axes are ready and clears when the results are read.

use embedded_hal_async::spi::SpiDevice;

use crate::Error;

/// Register addresses.
#[allow(missing_docs)]
pub mod reg {
    pub const POLL: u8 = 0x00;
    pub const CMM: u8 = 0x01;
    pub const CCX: u8 = 0x04;
    pub const CCY: u8 = 0x06;
    pub const CCZ: u8 = 0x08;
    pub const TMRC: u8 = 0x0B;
    pub const MX: u8 = 0x24;
    pub const BIST: u8 = 0x33;
    pub const STATUS: u8 = 0x34;
    pub const HSHAKE: u8 = 0x35;
    pub const REVID: u8 = 0x36;
}

/// Expected `REVID`.
pub const REVID: u8 = 0x22;
/// `POLL` value requesting all three axes.
pub const POLL_XYZ: u8 = 0x70;
/// `CMM` value: start continuous mode, all axes, `DRDY` after a full three-axis cycle.
pub const CMM_CONTINUOUS_XYZ: u8 = 0x79;
/// `CMM` value: stop.
pub const CMM_STOP: u8 = 0x00;
/// Read flag for the address byte.
const READ: u8 = 0x80;
/// Lowest cycle count the driver accepts (datasheet operating range starts at 30).
pub const MIN_CYCLE_COUNT: u16 = 30;
/// Highest cycle count the driver accepts (gain still linear, ~0.6 s per three-axis sample).
pub const MAX_CYCLE_COUNT: u16 = 4000;

/// Continuous-mode update rates (`TMRC` register values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Tmrc {
    /// ≈ 600 Hz
    Hz600 = 0x92,
    /// ≈ 300 Hz
    Hz300 = 0x93,
    /// ≈ 150 Hz
    Hz150 = 0x94,
    /// ≈ 75 Hz
    Hz75 = 0x95,
    /// ≈ 37 Hz
    Hz37 = 0x96,
    /// ≈ 18 Hz
    Hz18 = 0x97,
    /// ≈ 9 Hz
    Hz9 = 0x98,
    /// ≈ 4.5 Hz
    Hz4_5 = 0x99,
    /// ≈ 2.3 Hz
    Hz2_3 = 0x9A,
    /// ≈ 1.2 Hz
    Hz1_2 = 0x9B,
}

impl Tmrc {
    /// Nominal rate in Hz.
    #[must_use]
    pub const fn hz(self) -> f32 {
        match self {
            Self::Hz600 => 600.0,
            Self::Hz300 => 300.0,
            Self::Hz150 => 150.0,
            Self::Hz75 => 75.0,
            Self::Hz37 => 37.0,
            Self::Hz18 => 18.0,
            Self::Hz9 => 9.0,
            Self::Hz4_5 => 4.5,
            Self::Hz2_3 => 2.3,
            Self::Hz1_2 => 1.2,
        }
    }

    /// Fastest rate that a given cycle count can sustain with a 25 % margin, so continuous mode
    /// never overruns (which would silently corrupt readings).
    #[must_use]
    pub fn for_cycle_count(cycle_count: u16) -> Self {
        let max_hz = max_sample_rate_hz(cycle_count) * 0.75;
        for t in [
            Self::Hz600,
            Self::Hz300,
            Self::Hz150,
            Self::Hz75,
            Self::Hz37,
            Self::Hz18,
            Self::Hz9,
            Self::Hz4_5,
            Self::Hz2_3,
        ] {
            if t.hz() <= max_hz {
                return t;
            }
        }
        Self::Hz1_2
    }
}

/// Approximate maximum three-axis sample rate for a cycle count, Hz (datasheet: 1600 Hz single
/// axis at CC = 50, 440 Hz at CC = 200, i.e. ≈ 88 000 / CC per axis).
#[must_use]
pub fn max_sample_rate_hz(cycle_count: u16) -> f32 {
    88_000.0 / 3.0 / f32::from(cycle_count.max(1))
}

/// Nominal gain in LSB/µT for a cycle count.
#[must_use]
pub fn gain_lsb_per_ut(cycle_count: u16) -> f32 {
    0.3671 * f32::from(cycle_count) + 1.5
}

/// One three-axis measurement in raw counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Measurement {
    /// X counts.
    pub x: i32,
    /// Y counts.
    pub y: i32,
    /// Z counts.
    pub z: i32,
}

impl Measurement {
    /// Parses the nine bytes read from `MX`.
    #[must_use]
    pub const fn from_bytes(b: &[u8; 9]) -> Self {
        Self {
            x: i24(b[0], b[1], b[2]),
            y: i24(b[3], b[4], b[5]),
            z: i24(b[6], b[7], b[8]),
        }
    }
}

/// Sign-extends a 24-bit big-endian value.
const fn i24(msb: u8, mid: u8, lsb: u8) -> i32 {
    i32::from_be_bytes([msb, mid, lsb, 0]) >> 8
}

/// RM3100 driver.
pub struct Rm3100<SPI> {
    spi: SPI,
    cycle_count: u16,
}

impl<SPI: SpiDevice> Rm3100<SPI> {
    /// Wraps a bus device. Nothing is sent until [`Self::probe`] or a configuration call.
    pub const fn new(spi: SPI) -> Self {
        Self { spi, cycle_count: 200 }
    }

    /// Releases the bus device.
    pub fn release(self) -> SPI {
        self.spi
    }

    /// Cycle count last written (device default 200 until set).
    #[must_use]
    pub const fn cycle_count(&self) -> u16 {
        self.cycle_count
    }

    async fn write_regs(&mut self, start: u8, data: &[u8]) -> Result<(), Error<SPI::Error>> {
        // Address byte followed by the data; longest write is the 6-byte cycle count block.
        let mut buf = [0u8; 8];
        buf[0] = start & !READ;
        buf[1..=data.len()].copy_from_slice(data);
        self.spi.write(&buf[..=data.len()]).await?;
        Ok(())
    }

    async fn read_regs(&mut self, start: u8, out: &mut [u8]) -> Result<(), Error<SPI::Error>> {
        let mut tx = [0u8; 10];
        let mut rx = [0u8; 10];
        tx[0] = start | READ;
        let n = out.len() + 1;
        self.spi.transfer(&mut rx[..n], &tx[..n]).await?;
        out.copy_from_slice(&rx[1..n]);
        Ok(())
    }

    /// Reads `REVID` and checks it. Returns the value read.
    pub async fn probe(&mut self) -> Result<u8, Error<SPI::Error>> {
        let mut id = [0u8; 1];
        self.read_regs(reg::REVID, &mut id).await?;
        if id[0] == REVID {
            Ok(id[0])
        } else {
            Err(Error::BadId(id[0]))
        }
    }

    /// Sets the same cycle count on the three axes.
    pub async fn set_cycle_count(&mut self, cycle_count: u16) -> Result<(), Error<SPI::Error>> {
        if !(MIN_CYCLE_COUNT..=MAX_CYCLE_COUNT).contains(&cycle_count) {
            return Err(Error::InvalidArgument);
        }
        let [hi, lo] = cycle_count.to_be_bytes();
        self.write_regs(reg::CCX, &[hi, lo, hi, lo, hi, lo]).await?;
        self.cycle_count = cycle_count;
        Ok(())
    }

    /// Reads back the X cycle count (sanity check of the SPI link).
    pub async fn read_cycle_count(&mut self) -> Result<u16, Error<SPI::Error>> {
        let mut b = [0u8; 2];
        self.read_regs(reg::CCX, &mut b).await?;
        Ok(u16::from_be_bytes(b))
    }

    /// Stops continuous mode (also the state after power-up).
    pub async fn stop(&mut self) -> Result<(), Error<SPI::Error>> {
        self.write_regs(reg::CMM, &[CMM_STOP]).await
    }

    /// Requests one three-axis measurement. `DRDY` rises when it is complete.
    pub async fn start_single(&mut self) -> Result<(), Error<SPI::Error>> {
        self.write_regs(reg::POLL, &[POLL_XYZ]).await
    }

    /// Starts continuous measurement at the given rate.
    pub async fn start_continuous(&mut self, rate: Tmrc) -> Result<(), Error<SPI::Error>> {
        self.write_regs(reg::TMRC, &[rate as u8]).await?;
        self.write_regs(reg::CMM, &[CMM_CONTINUOUS_XYZ]).await
    }

    /// `true` when the `STATUS` register reports data ready (bit 7). Prefer the `DRDY` pin.
    pub async fn data_ready(&mut self) -> Result<bool, Error<SPI::Error>> {
        let mut s = [0u8; 1];
        self.read_regs(reg::STATUS, &mut s).await?;
        Ok(s[0] & 0x80 != 0)
    }

    /// Reads the three axes (clears `DRDY`).
    pub async fn read_measurement(&mut self) -> Result<Measurement, Error<SPI::Error>> {
        let mut b = [0u8; 9];
        self.read_regs(reg::MX, &mut b).await?;
        Ok(Measurement::from_bytes(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal_mock::eh1::spi::{Mock, Transaction};

    #[test]
    fn sign_extension_and_gain() {
        assert_eq!(i24(0xFF, 0xFF, 0xFF), -1);
        assert_eq!(i24(0x7F, 0xFF, 0xFF), 8_388_607);
        assert_eq!(i24(0x80, 0x00, 0x00), -8_388_608);
        assert_eq!(i24(0x00, 0x01, 0x00), 256);
        assert!((gain_lsb_per_ut(200) - 74.92).abs() < 0.01);
        assert_eq!(Tmrc::for_cycle_count(400), Tmrc::Hz37);
        assert_eq!(Tmrc::for_cycle_count(200), Tmrc::Hz75);
        assert_eq!(Tmrc::for_cycle_count(50), Tmrc::Hz300);
    }

    #[tokio::test]
    async fn probe_configure_and_read() {
        let expectations = [
            Transaction::transaction_start(),
            Transaction::transfer(vec![0x80 | reg::REVID, 0], vec![0, REVID]),
            Transaction::transaction_end(),
            Transaction::transaction_start(),
            Transaction::write_vec(vec![reg::CCX, 0x01, 0x90, 0x01, 0x90, 0x01, 0x90]),
            Transaction::transaction_end(),
            Transaction::transaction_start(),
            Transaction::write_vec(vec![reg::POLL, POLL_XYZ]),
            Transaction::transaction_end(),
            Transaction::transaction_start(),
            Transaction::transfer(vec![0x80 | reg::STATUS, 0], vec![0, 0x80]),
            Transaction::transaction_end(),
            Transaction::transaction_start(),
            Transaction::transfer(
                vec![0x80 | reg::MX, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                vec![0, 0x00, 0x10, 0x00, 0xFF, 0xFF, 0xFE, 0x00, 0x00, 0x05],
            ),
            Transaction::transaction_end(),
        ];
        let mut dev = Rm3100::new(Mock::new(&expectations));
        assert_eq!(dev.probe().await.unwrap(), REVID);
        dev.set_cycle_count(400).await.unwrap();
        assert_eq!(dev.cycle_count(), 400);
        dev.start_single().await.unwrap();
        assert!(dev.data_ready().await.unwrap());
        let m = dev.read_measurement().await.unwrap();
        assert_eq!(m, Measurement { x: 4096, y: -2, z: 5 });
        dev.release().done();
    }

    #[tokio::test]
    async fn bad_id_and_bad_cycle_count() {
        let expectations = [
            Transaction::transaction_start(),
            Transaction::transfer(vec![0x80 | reg::REVID, 0], vec![0, 0x00]),
            Transaction::transaction_end(),
        ];
        let mut dev = Rm3100::new(Mock::new(&expectations));
        assert_eq!(dev.probe().await, Err(Error::BadId(0)));
        assert_eq!(dev.set_cycle_count(10).await, Err(Error::InvalidArgument));
        dev.release().done();
    }
}
