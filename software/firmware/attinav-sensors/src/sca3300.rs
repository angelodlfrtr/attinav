//! Murata SCA3300-D01 three-axis accelerometer, SPI interface.
//!
//! Every exchange is one 32-bit frame, MSB first, SPI mode 0, SCK ≤ 8 MHz, ≥ 10 µs between
//! frames:
//!
//! | bits   | request              | response                          |
//! |--------|----------------------|-----------------------------------|
//! | 31..26 | RW + 5-bit address   | echo of the request's RW + address|
//! | 25..24 | 0                    | return status `RS`                |
//! | 23..8  | data (writes)        | data                              |
//! | 7..0   | CRC-8 of bits 31..8  | CRC-8 of bits 31..8               |
//!
//! Responses are **pipelined**: the frame returned while sending command *n* answers command
//! *n − 1*. `RS` is `01` in normal operation, `00` during start-up, `10` while a self-test runs,
//! `11` on error (details then in the `STATUS` register).
//!
//! Mode 4 (±1.5 g, 10 Hz low-pass, 5400 LSB/g) is the survey mode; it needs 100 ms to settle after
//! being selected and 100 ms after any power-up before the data is trustworthy.

use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::spi::SpiDevice;

use crate::Error;

/// Command words (request frames) from the datasheet, CRC included.
#[allow(missing_docs)]
pub mod cmd {
    pub const READ_ACC_X: u32 = 0x040000F7;
    pub const READ_ACC_Y: u32 = 0x080000FD;
    pub const READ_ACC_Z: u32 = 0x0C0000FB;
    pub const READ_STO: u32 = 0x100000E9;
    pub const READ_TEMP: u32 = 0x140000EF;
    pub const READ_STATUS: u32 = 0x180000E5;
    pub const MODE_1: u32 = 0xB400001F;
    pub const MODE_2: u32 = 0xB4000102;
    pub const MODE_3: u32 = 0xB4000225;
    pub const MODE_4: u32 = 0xB4000338;
    pub const POWER_DOWN: u32 = 0xB400046B;
    pub const WAKE_UP: u32 = 0xB400001F;
    pub const SW_RESET: u32 = 0xB4002098;
    pub const WHOAMI: u32 = 0x40000091;
    pub const READ_SERIAL_1: u32 = 0x640000A7;
    pub const READ_SERIAL_2: u32 = 0x680000AD;
    pub const READ_CURRENT_BANK: u32 = 0x7C0000B3;
    pub const SWITCH_BANK_0: u32 = 0xFC000073;
    /// Bank 1 select: same opcode as bank 0 with data `0x0001`. Computed from the frame CRC so
    /// it cannot drift from the quoted datasheet word (`0xFC00016E`, checked by a test).
    pub const SWITCH_BANK_1: u32 = super::with_crc(0xFC00_0100);
}

/// Completes a frame whose CRC byte is zero with its CRC-8.
#[must_use]
pub const fn with_crc(frame: u32) -> u32 {
    (frame & 0xFFFF_FF00) | crc8(frame) as u32
}

/// The RW + address field (bits 31..26) of a command word, as echoed by the response to it.
#[must_use]
pub const fn opcode(command: u32) -> u8 {
    (command >> 26) as u8
}

/// `WHOAMI` data of an SCA3300.
pub const WHOAMI_SCA3300: u8 = 0x51;

/// Return status field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum ReturnStatus {
    /// Start-up in progress.
    Startup = 0,
    /// Normal operation, no flags.
    Normal = 1,
    /// Self-test running.
    SelfTest = 2,
    /// Error flag active (read `STATUS`).
    Error = 3,
}

impl ReturnStatus {
    const fn from_bits(b: u8) -> Self {
        match b & 0x03 {
            0 => Self::Startup,
            1 => Self::Normal,
            2 => Self::SelfTest,
            _ => Self::Error,
        }
    }
}

/// Measurement modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Mode {
    /// ±3 g, 88 Hz, 2700 LSB/g.
    Mode1,
    /// ±6 g, 88 Hz, 1350 LSB/g.
    Mode2,
    /// ±1.5 g, 88 Hz, 5400 LSB/g.
    Mode3,
    /// ±1.5 g, 10 Hz, 5400 LSB/g (survey mode).
    Mode4,
}

impl Mode {
    /// Command word selecting the mode.
    #[must_use]
    pub const fn command(self) -> u32 {
        match self {
            Self::Mode1 => cmd::MODE_1,
            Self::Mode2 => cmd::MODE_2,
            Self::Mode3 => cmd::MODE_3,
            Self::Mode4 => cmd::MODE_4,
        }
    }

    /// Sensitivity, LSB/g.
    #[must_use]
    pub const fn lsb_per_g(self) -> f32 {
        match self {
            Self::Mode1 => 2700.0,
            Self::Mode2 => 1350.0,
            Self::Mode3 | Self::Mode4 => 5400.0,
        }
    }

    /// Settling time after selecting the mode, ms.
    #[must_use]
    pub const fn settle_ms(self) -> u32 {
        match self {
            Self::Mode4 => 100,
            _ => 15,
        }
    }
}

/// CRC-8 used by the SCA3300 frames: poly 0x1D, init 0xFF, final XOR 0xFF, over bits 31..8.
#[must_use]
pub const fn crc8(frame_without_crc: u32) -> u8 {
    let mut crc: u8 = 0xFF;
    let mut i = 31;
    while i >= 8 {
        let bit = ((frame_without_crc >> i) & 1) as u8;
        let msb = (crc >> 7) & 1;
        crc <<= 1;
        if bit ^ msb != 0 {
            crc ^= 0x1D;
        }
        i -= 1;
    }
    !crc
}

/// A decoded response frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Frame {
    /// Echoed RW + address (bits 31..26).
    pub op: u8,
    /// Return status.
    pub rs: ReturnStatus,
    /// 16-bit data field.
    pub data: u16,
}

impl Frame {
    /// Parses and CRC-checks a response word.
    pub const fn parse<E>(word: u32) -> Result<Self, Error<E>> {
        if crc8(word) != (word & 0xFF) as u8 {
            return Err(Error::Crc);
        }
        Ok(Self {
            op: (word >> 26) as u8,
            rs: ReturnStatus::from_bits((word >> 24) as u8),
            data: (word >> 8) as u16,
        })
    }

    /// Data as a signed 16-bit value (acceleration, temperature).
    #[must_use]
    pub const fn data_i16(&self) -> i16 {
        i16::from_ne_bytes(self.data.to_ne_bytes())
    }
}

/// One complete reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Sample {
    /// X counts.
    pub x: i16,
    /// Y counts.
    pub y: i16,
    /// Z counts.
    pub z: i16,
    /// Raw temperature.
    pub temp: i16,
    /// `STATUS` summary register.
    pub status: u16,
}

/// Raw temperature to °C: `T = −273 + raw / 18.9`.
#[must_use]
pub fn temp_celsius(raw: i16) -> f32 {
    -273.0 + f32::from(raw) / 18.9
}

/// SCA3300 driver.
pub struct Sca3300<SPI, D> {
    spi: SPI,
    delay: D,
    mode: Mode,
}

impl<SPI: SpiDevice, D: DelayNs> Sca3300<SPI, D> {
    /// Wraps a bus device.
    pub const fn new(spi: SPI, delay: D) -> Self {
        Self {
            spi,
            delay,
            mode: Mode::Mode4,
        }
    }

    /// Releases the bus device.
    pub fn release(self) -> (SPI, D) {
        (self.spi, self.delay)
    }

    /// Active mode.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Sends one command word and returns the (pipelined) response to the previous one,
    /// `answers`, CRC-checked. The echoed opcode must match `answers`: a mismatch means the
    /// pipeline lost sync (missed frame, chip-select glitch) and the data would otherwise be
    /// attributed to the wrong register.
    pub async fn exchange(&mut self, command: u32, answers: u32) -> Result<Frame, Error<SPI::Error>> {
        let tx = command.to_be_bytes();
        let mut rx = [0u8; 4];
        self.gap().await;
        self.spi.transfer(&mut rx, &tx).await?;
        let frame = Frame::parse(u32::from_be_bytes(rx))?;
        if frame.op != opcode(answers) {
            return Err(Error::Protocol);
        }
        Ok(frame)
    }

    /// Sends a command and discards the response (used to prime the pipeline).
    pub async fn send(&mut self, command: u32) -> Result<(), Error<SPI::Error>> {
        let tx = command.to_be_bytes();
        let mut rx = [0u8; 4];
        self.gap().await;
        self.spi.transfer(&mut rx, &tx).await?;
        Ok(())
    }

    /// The datasheet's minimum chip-select high time between frames (10 µs). Back-to-back
    /// transactions at 160 MHz would otherwise leave only a few microseconds and desynchronise
    /// the pipeline.
    async fn gap(&mut self) {
        self.delay.delay_us(10).await;
    }

    /// Full start-up sequence from the datasheet: software reset, mode selection, settling, then
    /// three `STATUS` reads (the first two clear the start-up flags) and the identity check.
    pub async fn init(&mut self, mode: Mode) -> Result<(), Error<SPI::Error>> {
        self.delay.delay_ms(10).await;
        self.send(cmd::SW_RESET).await?;
        self.delay.delay_ms(1).await;
        self.set_mode(mode).await?;
        self.send(cmd::READ_STATUS).await?;
        self.send(cmd::READ_STATUS).await?;
        self.send(cmd::READ_STATUS).await?;
        let status = self.exchange(cmd::WHOAMI, cmd::READ_STATUS).await?;
        if status.rs != ReturnStatus::Normal {
            return Err(Error::Status(status.rs as u8));
        }
        let who = self.exchange(cmd::READ_STATUS, cmd::WHOAMI).await?;
        if (who.data & 0xFF) as u8 != WHOAMI_SCA3300 {
            return Err(Error::BadId((who.data & 0xFF) as u8));
        }
        Ok(())
    }

    /// Selects a mode and waits its settling time.
    pub async fn set_mode(&mut self, mode: Mode) -> Result<(), Error<SPI::Error>> {
        self.send(mode.command()).await?;
        self.mode = mode;
        self.delay.delay_ms(mode.settle_ms()).await;
        Ok(())
    }

    /// Reads X, Y, Z, temperature and status in one pipelined burst of six frames.
    ///
    /// Fails with [`Error::Status`] if any response carries `RS = 11` (error) or `RS = 00`
    /// (still starting up); a self-test in progress is tolerated.
    pub async fn read(&mut self) -> Result<Sample, Error<SPI::Error>> {
        self.send(cmd::READ_ACC_X).await?;
        let x = self.exchange(cmd::READ_ACC_Y, cmd::READ_ACC_X).await?;
        let y = self.exchange(cmd::READ_ACC_Z, cmd::READ_ACC_Y).await?;
        let z = self.exchange(cmd::READ_TEMP, cmd::READ_ACC_Z).await?;
        let t = self.exchange(cmd::READ_STATUS, cmd::READ_TEMP).await?;
        let s = self.exchange(cmd::READ_STATUS, cmd::READ_STATUS).await?;
        for f in [&x, &y, &z, &t, &s] {
            if matches!(f.rs, ReturnStatus::Error | ReturnStatus::Startup) {
                return Err(Error::Status(f.rs as u8));
            }
        }
        Ok(Sample {
            x: x.data_i16(),
            y: y.data_i16(),
            z: z.data_i16(),
            temp: t.data_i16(),
            status: s.data,
        })
    }

    /// Reads the 32-bit serial number.
    pub async fn serial(&mut self) -> Result<u32, Error<SPI::Error>> {
        self.send(cmd::SWITCH_BANK_1).await?;
        self.send(cmd::READ_SERIAL_1).await?;
        let s1 = self.exchange(cmd::READ_SERIAL_2, cmd::READ_SERIAL_1).await?;
        let s2 = self.exchange(cmd::SWITCH_BANK_0, cmd::READ_SERIAL_2).await?;
        Ok(u32::from(s2.data) << 16 | u32::from(s1.data))
    }

    /// Enters power-down (≈ 90 µA). Leave with [`Self::init`].
    pub async fn power_down(&mut self) -> Result<(), Error<SPI::Error>> {
        self.send(cmd::POWER_DOWN).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal_mock::eh1::delay::NoopDelay;
    use embedded_hal_mock::eh1::spi::{Mock, Transaction};

    /// Builds a response word with a valid CRC.
    fn response(op: u8, rs: u8, data: u16) -> u32 {
        let w = (u32::from(op) << 26) | (u32::from(rs & 3) << 24) | (u32::from(data) << 8);
        w | u32::from(crc8(w))
    }

    #[test]
    fn crc_matches_datasheet_command_words() {
        for c in [
            cmd::READ_ACC_X,
            cmd::READ_ACC_Y,
            cmd::READ_ACC_Z,
            cmd::READ_STO,
            cmd::READ_TEMP,
            cmd::READ_STATUS,
            cmd::MODE_1,
            cmd::MODE_2,
            cmd::MODE_3,
            cmd::MODE_4,
            cmd::POWER_DOWN,
            cmd::SW_RESET,
            cmd::WHOAMI,
            cmd::READ_SERIAL_1,
            cmd::READ_SERIAL_2,
            cmd::READ_CURRENT_BANK,
            cmd::SWITCH_BANK_0,
            cmd::SWITCH_BANK_1,
        ] {
            assert_eq!(crc8(c), (c & 0xFF) as u8, "command {c:#010X}");
        }
        // The computed bank-1 word equals the one printed in the datasheet.
        assert_eq!(cmd::SWITCH_BANK_1, 0xFC00_016E);
    }

    #[tokio::test]
    async fn pipeline_desync_is_detected() {
        let mut ex = Vec::new();
        ex.extend(xfer(cmd::READ_ACC_X, response(0x06, 1, 0)));
        // The response to READ_ACC_X should echo opcode 0x01; a frame echoing READ_ACC_Y (0x02)
        // means a frame went missing and the data belongs to another axis.
        ex.extend(xfer(cmd::READ_ACC_Y, response(0x02, 1, 100)));
        let mut dev = Sca3300::new(Mock::new(&ex), NoopDelay);
        assert_eq!(dev.read().await, Err(Error::Protocol));
        let (mut spi, _) = dev.release();
        spi.done();
    }

    #[test]
    fn frame_parse_and_temperature() {
        let w = response(0x01, 1, 0xFFFE);
        let f = Frame::parse::<()>(w).unwrap();
        assert_eq!(f.rs, ReturnStatus::Normal);
        assert_eq!(f.data_i16(), -2);
        assert_eq!(Frame::parse::<()>(w ^ 0x100), Err(Error::Crc));
        // 25 °C ≈ 5632 raw.
        assert!((temp_celsius(5632) - 25.0).abs() < 0.1);
    }

    fn xfer(tx: u32, rx: u32) -> [Transaction<u8>; 3] {
        [
            Transaction::transaction_start(),
            Transaction::transfer(tx.to_be_bytes().to_vec(), rx.to_be_bytes().to_vec()),
            Transaction::transaction_end(),
        ]
    }

    #[tokio::test]
    async fn init_and_read() {
        let mut ex = Vec::new();
        ex.extend(xfer(cmd::SW_RESET, 0));
        ex.extend(xfer(cmd::MODE_4, response(0x2D, 0, 0)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x2D, 0, 0)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x06, 0, 0x0002)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x06, 1, 0x0000)));
        ex.extend(xfer(cmd::WHOAMI, response(0x06, 1, 0x0000)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x10, 1, 0x0051)));
        // read(): X, Y, Z, TEMP, STATUS, STATUS
        ex.extend(xfer(cmd::READ_ACC_X, response(0x06, 1, 0)));
        ex.extend(xfer(cmd::READ_ACC_Y, response(0x01, 1, 100)));
        ex.extend(xfer(cmd::READ_ACC_Z, response(0x02, 1, 0xFF9C)));
        ex.extend(xfer(cmd::READ_TEMP, response(0x03, 1, 5400)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x05, 1, 5632)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x06, 1, 0)));
        let mut dev = Sca3300::new(Mock::new(&ex), NoopDelay);
        dev.init(Mode::Mode4).await.unwrap();
        assert_eq!(dev.mode(), Mode::Mode4);
        let s = dev.read().await.unwrap();
        assert_eq!(
            s,
            Sample {
                x: 100,
                y: -100,
                z: 5400,
                temp: 5632,
                status: 0
            }
        );
        let (mut spi, _) = dev.release();
        spi.done();
    }

    #[tokio::test]
    async fn error_status_is_reported() {
        let mut ex = Vec::new();
        ex.extend(xfer(cmd::READ_ACC_X, response(0x06, 1, 0)));
        ex.extend(xfer(cmd::READ_ACC_Y, response(0x01, 3, 100)));
        ex.extend(xfer(cmd::READ_ACC_Z, response(0x02, 1, 0)));
        ex.extend(xfer(cmd::READ_TEMP, response(0x03, 1, 0)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x05, 1, 0)));
        ex.extend(xfer(cmd::READ_STATUS, response(0x06, 1, 0x0010)));
        let mut dev = Sca3300::new(Mock::new(&ex), NoopDelay);
        assert_eq!(dev.read().await, Err(Error::Status(3)));
        let (mut spi, _) = dev.release();
        spi.done();
    }
}
