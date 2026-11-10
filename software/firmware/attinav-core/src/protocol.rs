//! Host protocol, shared by the carrier UART (LPUART1) and the USB CDC-ACM link.
//!
//! See `PROTOCOL.md` in the firmware directory for the normative description. In short:
//!
//! * **Framing** — every frame is COBS-encoded and terminated by a `0x00` byte, so a receiver can
//!   resynchronise after any corruption by waiting for the next zero.
//! * **Frame** (before COBS) — `kind:u8 | seq:u8 | payload | crc16:u16 LE`, CRC-16/CCITT-FALSE over
//!   `kind..payload`. `seq` is echoed in the device's reply so a host can match requests.
//! * **Payloads** — fixed little-endian layouts defined by the `encode`/`decode` pairs below; no
//!   schema language, no allocation, every field documented on the struct.
//!
//! Host→device messages are [`HostMessage`], device→host are [`DeviceMessage`]. Every request is
//! answered by at least one frame carrying the same `seq`: either a dedicated reply or an [`Ack`].
//! Unsolicited frames (`Sample`, `Event`, `Log`) carry `seq = 0`.

use crate::calibration::{ACC_COUNT, CALIBRATION_MAX_BYTES, Calibration, MAG_COUNT, MAX_SETS, SensorId};
use crate::crc::crc16_ccitt_false;
use crate::math::Vec3;
use crate::quality::{QualityFlags, Thresholds};
use crate::solver::{CalShot, SolveError, SolveReport};
use crate::wire::{Reader, WireError, Writer};

/// Largest payload of any message (a full [`Calibration`]).
pub const MAX_PAYLOAD: usize = CALIBRATION_MAX_BYTES;
/// `kind + seq + crc16`.
pub const FRAME_OVERHEAD: usize = 4;
/// Largest frame before COBS.
pub const MAX_FRAME: usize = MAX_PAYLOAD + FRAME_OVERHEAD;
/// Largest COBS-encoded frame including the trailing delimiter.
pub const MAX_ENCODED: usize = MAX_FRAME + MAX_FRAME / 254 + 2;
/// Frame delimiter.
pub const DELIMITER: u8 = 0x00;

/// Firmware version carried in `Info`.
pub const FIRMWARE_VERSION: [u8; 3] = [0, 1, 0];

// -------------------------------------------------------------------------------------------
// COBS
// -------------------------------------------------------------------------------------------

/// Consistent Overhead Byte Stuffing.
pub mod cobs {
    use super::WireError;

    /// Worst-case encoded length for `n` input bytes (without delimiter).
    #[must_use]
    pub const fn max_encoded_len(n: usize) -> usize {
        n + n / 254 + 1
    }

    /// Encodes `src` into `dst` (no delimiter appended); returns the encoded length.
    pub fn encode(src: &[u8], dst: &mut [u8]) -> Result<usize, WireError> {
        if dst.len() < max_encoded_len(src.len()) {
            return Err(WireError::BufferFull);
        }
        let mut code_idx = 0;
        let mut out = 1;
        let mut code: u8 = 1;
        for &b in src {
            if b == 0 {
                dst[code_idx] = code;
                code_idx = out;
                out += 1;
                code = 1;
            } else {
                dst[out] = b;
                out += 1;
                code += 1;
                if code == 0xFF {
                    dst[code_idx] = code;
                    code_idx = out;
                    out += 1;
                    code = 1;
                }
            }
        }
        dst[code_idx] = code;
        Ok(out)
    }

    /// Decodes `src` (one frame without its delimiter) into `dst`; returns the decoded length.
    pub fn decode(src: &[u8], dst: &mut [u8]) -> Result<usize, WireError> {
        let mut i = 0;
        let mut out = 0;
        while i < src.len() {
            let code = usize::from(src[i]);
            if code == 0 {
                return Err(WireError::Invalid);
            }
            i += 1;
            let end = i + code - 1;
            if end > src.len() {
                return Err(WireError::Truncated);
            }
            let chunk = &src[i..end];
            if out + chunk.len() > dst.len() {
                return Err(WireError::BufferFull);
            }
            dst[out..out + chunk.len()].copy_from_slice(chunk);
            out += chunk.len();
            i = end;
            if code != 0xFF && i < src.len() {
                if out >= dst.len() {
                    return Err(WireError::BufferFull);
                }
                dst[out] = 0;
                out += 1;
            }
        }
        Ok(out)
    }
}

// -------------------------------------------------------------------------------------------
// Frame layer
// -------------------------------------------------------------------------------------------

/// Builds a complete encoded frame (COBS + delimiter) into `out`; returns its length.
#[allow(
    clippy::large_stack_arrays,
    reason = "one frame-sized scratch buffer; the firmware stack is sized for it"
)]
pub fn encode_frame(kind: u8, seq: u8, payload: &[u8], out: &mut [u8]) -> Result<usize, WireError> {
    let mut raw = [0u8; MAX_FRAME];
    let mut w = Writer::new(&mut raw);
    w.u8(kind)?;
    w.u8(seq)?;
    w.bytes(payload)?;
    let crc = crc16_ccitt_false(w.written());
    w.u16(crc)?;
    let n = w.len();
    let enc = cobs::encode(&raw[..n], out)?;
    if enc >= out.len() {
        return Err(WireError::BufferFull);
    }
    out[enc] = DELIMITER;
    Ok(enc + 1)
}

/// A decoded frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct FrameHeader {
    /// Message kind.
    pub kind: u8,
    /// Sequence number.
    pub seq: u8,
}

/// Why a received frame was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameError {
    /// Frame too long for the decoder's buffer.
    Overflow,
    /// COBS structure invalid.
    Cobs,
    /// Shorter than the fixed overhead.
    TooShort,
    /// CRC mismatch.
    Crc,
}

/// Incremental frame decoder: push bytes, get frames.
pub struct Decoder {
    encoded: [u8; MAX_ENCODED],
    decoded: [u8; MAX_FRAME],
    len: usize,
    overflow: bool,
    payload_len: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// Empty decoder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            encoded: [0; MAX_ENCODED],
            decoded: [0; MAX_FRAME],
            len: 0,
            overflow: false,
            payload_len: 0,
        }
    }

    /// Feeds one byte. Returns `Some` when a delimiter completed a frame: `Ok(header)` for a valid
    /// frame whose payload is then available through [`Self::payload`], `Err` for a dropped one.
    pub fn push(&mut self, byte: u8) -> Option<Result<FrameHeader, FrameError>> {
        if byte != DELIMITER {
            if self.len < self.encoded.len() {
                self.encoded[self.len] = byte;
                self.len += 1;
            } else {
                self.overflow = true;
            }
            return None;
        }
        let len = self.len;
        let overflow = self.overflow;
        self.len = 0;
        self.overflow = false;
        if len == 0 {
            // Back-to-back delimiters are legal idle filler.
            return None;
        }
        if overflow {
            return Some(Err(FrameError::Overflow));
        }
        let n = match cobs::decode(&self.encoded[..len], &mut self.decoded) {
            Ok(n) => n,
            Err(WireError::BufferFull) => return Some(Err(FrameError::Overflow)),
            Err(_) => return Some(Err(FrameError::Cobs)),
        };
        if n < FRAME_OVERHEAD {
            return Some(Err(FrameError::TooShort));
        }
        let body = &self.decoded[..n - 2];
        let crc = u16::from_le_bytes([self.decoded[n - 2], self.decoded[n - 1]]);
        if crc16_ccitt_false(body) != crc {
            return Some(Err(FrameError::Crc));
        }
        self.payload_len = n - FRAME_OVERHEAD;
        Some(Ok(FrameHeader {
            kind: self.decoded[0],
            seq: self.decoded[1],
        }))
    }

    /// Payload of the last successfully decoded frame.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.decoded[2..2 + self.payload_len]
    }
}

// -------------------------------------------------------------------------------------------
// Shared payload structures
// -------------------------------------------------------------------------------------------

fn vec3_write(w: &mut Writer<'_>, v: Vec3) -> Result<(), WireError> {
    w.f32x3(v.to_f32_array())
}

fn vec3_read(r: &mut Reader<'_>) -> Result<Vec3, WireError> {
    let a = r.f32x3()?;
    Ok(Vec3::new(f64::from(a[0]), f64::from(a[1]), f64::from(a[2])))
}

/// Reply status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum Status {
    /// Done.
    Ok = 0,
    /// Another operation is in progress (a shot, a solve).
    Busy = 1,
    /// A field was out of range.
    InvalidArgument = 2,
    /// Unknown message kind or unsupported option.
    Unsupported = 3,
    /// A sensor did not answer or answered garbage.
    SensorError = 4,
    /// A data-ready or settle wait timed out.
    Timeout = 5,
    /// No calibration stored.
    NoCalibration = 6,
    /// Flash erase/write/verify failed.
    FlashError = 7,
    /// The solver refused or failed (detail carries the [`SolveError`] tag).
    SolveFailed = 8,
    /// Calibration command outside a calibration session.
    NotCalibrating = 9,
    /// The operation was aborted by the host.
    Aborted = 10,
    /// Malformed payload.
    BadPayload = 11,
    /// Firmware-internal error (should not happen; details in the log).
    Internal = 12,
    /// Firmware image rejected (CRC mismatch, wrong size, not aligned, no update in progress).
    BadImage = 13,
}

impl Status {
    /// From a wire tag.
    #[must_use]
    pub const fn from_tag(t: u8) -> Option<Self> {
        Some(match t {
            0 => Self::Ok,
            1 => Self::Busy,
            2 => Self::InvalidArgument,
            3 => Self::Unsupported,
            4 => Self::SensorError,
            5 => Self::Timeout,
            6 => Self::NoCalibration,
            7 => Self::FlashError,
            8 => Self::SolveFailed,
            9 => Self::NotCalibrating,
            10 => Self::Aborted,
            11 => Self::BadPayload,
            12 => Self::Internal,
            13 => Self::BadImage,
            _ => return None,
        })
    }
}

/// Generic reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ack {
    /// Kind of the request being answered.
    pub request: u8,
    /// Outcome.
    pub status: Status,
    /// Status-specific detail (e.g. the [`SolveError`] tag, a sensor index).
    pub detail: u16,
}

/// Device identity and state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Info {
    /// Protocol version ([`crate::PROTOCOL_VERSION`]).
    pub protocol: u8,
    /// Firmware version `major.minor.patch`.
    pub firmware: [u8; 3],
    /// Hardware revision (2 = Rev B).
    pub hardware_rev: u8,
    /// MCU unique id (96 bits).
    pub uid: [u8; 12],
    /// Milliseconds since boot.
    pub uptime_ms: u32,
    /// Stored calibration id (0 = none).
    pub calibration_id: u32,
    /// Number of temperature sets in the stored calibration.
    pub calibration_sets: u8,
    /// Date of the stored calibration (unix seconds, 0 = unknown).
    pub calibration_date: u32,
    /// Bit `i` set when sensor `i` (see [`SensorId`]) answered its identity check at boot.
    pub sensors_ok: u8,
    /// `true` once the host has set the wall-clock time.
    pub time_set: bool,
    /// Current unix time if set, else 0.
    pub unix_time: u32,
    /// Bootloader state: 0 confirmed, 1 trial boot of a new image (not yet confirmed), 2 reverted to
    /// the previous image after a failed trial, 3 reboot into DFU requested, 255 unknown.
    pub boot_state: u8,
}

/// Flags of a [`ShotRequest`].
pub mod shot_flags {
    /// Also send one [`super::ShotRaw`] per sensor after the result.
    pub const WANT_RAW: u8 = 1 << 0;
    /// Do not assert `QUIET` (fast, noisy shot — for testing).
    pub const NO_QUIET: u8 = 1 << 1;
}

/// Request one shot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShotRequest {
    /// See [`shot_flags`].
    pub flags: u8,
    /// Magnetometer samples to average (0 = configured default).
    pub mag_samples: u16,
    /// RM3100 cycle count (0 = configured default).
    pub cycle_count: u16,
}

/// Sensors selectable in a stream (bit positions in [`StreamRequest::sensors`]).
pub mod stream_sensors {
    /// Triad 1.
    pub const MAG1: u8 = 1 << 0;
    /// Triad 2.
    pub const MAG2: u8 = 1 << 1;
    /// Triad 3.
    pub const MAG3: u8 = 1 << 2;
    /// Accelerometer 1.
    pub const ACC1: u8 = 1 << 3;
    /// Accelerometer 2.
    pub const ACC2: u8 = 1 << 4;
    /// IMU gyroscope.
    pub const IMU_GYRO: u8 = 1 << 5;
    /// IMU accelerometer.
    pub const IMU_ACC: u8 = 1 << 6;
    /// Temperatures (one [`super::Temps`] per second).
    pub const TEMPS: u8 = 1 << 7;
    /// Everything.
    pub const ALL: u8 = 0xFF;
}

/// Sample source tag in [`Sample::sensor`]: `0..=4` are [`SensorId`] tags, then the IMU parts.
pub mod sample_source {
    /// IMU gyroscope, raw counts.
    pub const IMU_GYRO: u8 = 5;
    /// IMU accelerometer, raw counts.
    pub const IMU_ACC: u8 = 6;
}

/// Start or stop raw streaming.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StreamRequest {
    /// `true` to start (or change), `false` to stop.
    pub enable: bool,
    /// Sample rate per sensor, Hz (clamped by the firmware).
    pub rate_hz: u16,
    /// See [`stream_sensors`].
    pub sensors: u8,
}

/// Smallest RM3100 cycle count the driver programs (`attinav_sensors::rm3100::MIN_CYCLE_COUNT`).
pub const CYCLE_COUNT_MIN: u16 = 30;
/// Largest RM3100 cycle count the driver programs (`attinav_sensors::rm3100::MAX_CYCLE_COUNT`).
pub const CYCLE_COUNT_MAX: u16 = 4000;

/// `true` when the driver will accept `cc` (see [`CYCLE_COUNT_MIN`], [`CYCLE_COUNT_MAX`]).
#[must_use]
pub const fn cycle_count_valid(cc: u16) -> bool {
    cc >= CYCLE_COUNT_MIN && cc <= CYCLE_COUNT_MAX
}

/// Runtime configuration, persisted in flash with the calibration.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceConfig {
    /// RM3100 cycle count for shots.
    pub mag_cycle_count: u16,
    /// Magnetometer samples averaged per shot.
    pub mag_samples: u16,
    /// Accelerometer sampling rate during a shot, Hz.
    pub acc_rate_hz: u16,
    /// Delay between asserting `QUIET` and the first sample, ms.
    pub quiet_lead_ms: u16,
    /// Assert `QUIET` during shots.
    pub quiet_enabled: bool,
    /// Flash the status LED on activity.
    pub led_enabled: bool,
    /// Quality thresholds.
    pub thresholds: Thresholds,
    /// External field reference, `b_total_ut == 0` means none.
    pub ref_b_total_ut: f32,
    /// External dip reference, degrees.
    pub ref_dip_deg: f32,
    /// LPUART1 baud rate (applied at next boot).
    pub uart_baud: u32,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            mag_cycle_count: 400,
            mag_samples: 100,
            acc_rate_hz: 200,
            quiet_lead_ms: 20,
            quiet_enabled: true,
            led_enabled: true,
            thresholds: Thresholds::default(),
            ref_b_total_ut: 0.0,
            ref_dip_deg: 0.0,
            uart_baud: 115_200,
        }
    }
}

impl DeviceConfig {
    /// Serialised size.
    pub const BYTES: usize = 2 + 2 + 2 + 2 + 1 + 1 + 2 + 8 * 4 + 4 + 4 + 4;

    /// `true` when every field is within what the firmware accepts.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        cycle_count_valid(self.mag_cycle_count)
            && (1..=4000).contains(&self.mag_samples)
            && (10..=2000).contains(&self.acc_rate_hz)
            && self.quiet_lead_ms <= 5000
            && (9600..=2_000_000).contains(&self.uart_baud)
            && self.ref_b_total_ut.is_finite()
            && self.ref_dip_deg.is_finite()
            && [
                self.thresholds.mag_disagree_deg,
                self.thresholds.acc_disagree_deg,
                self.thresholds.mag_norm_tol,
                self.thresholds.acc_norm_tol,
                self.thresholds.dip_tol_deg,
                self.thresholds.motion_dps,
                self.thresholds.mag_noise_rel,
                self.thresholds.acc_noise_rel,
            ]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
    }

    /// Serialises.
    pub fn write(&self, w: &mut Writer<'_>) -> Result<(), WireError> {
        w.u16(self.mag_cycle_count)?;
        w.u16(self.mag_samples)?;
        w.u16(self.acc_rate_hz)?;
        w.u16(self.quiet_lead_ms)?;
        w.u8(u8::from(self.quiet_enabled))?;
        w.u8(u8::from(self.led_enabled))?;
        w.u16(0)?;
        let t = &self.thresholds;
        for v in [
            t.mag_disagree_deg,
            t.acc_disagree_deg,
            t.mag_norm_tol,
            t.acc_norm_tol,
            t.dip_tol_deg,
            t.motion_dps,
            t.mag_noise_rel,
            t.acc_noise_rel,
        ] {
            w.f32(v)?;
        }
        w.f32(self.ref_b_total_ut)?;
        w.f32(self.ref_dip_deg)?;
        w.u32(self.uart_baud)
    }

    /// Deserialises (does not validate; call [`Self::is_valid`]).
    pub fn read(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let mag_cycle_count = r.u16()?;
        let mag_samples = r.u16()?;
        let acc_rate_hz = r.u16()?;
        let quiet_lead_ms = r.u16()?;
        let quiet_enabled = r.u8()? != 0;
        let led_enabled = r.u8()? != 0;
        let _pad = r.u16()?;
        let mut t = [0.0f32; 8];
        for v in &mut t {
            *v = r.f32()?;
        }
        let thresholds = Thresholds {
            mag_disagree_deg: t[0],
            acc_disagree_deg: t[1],
            mag_norm_tol: t[2],
            acc_norm_tol: t[3],
            dip_tol_deg: t[4],
            motion_dps: t[5],
            mag_noise_rel: t[6],
            acc_noise_rel: t[7],
        };
        Ok(Self {
            mag_cycle_count,
            mag_samples,
            acc_rate_hz,
            quiet_lead_ms,
            quiet_enabled,
            led_enabled,
            thresholds,
            ref_b_total_ut: r.f32()?,
            ref_dip_deg: r.f32()?,
            uart_baud: r.u32()?,
        })
    }
}

/// Result of one shot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShotResult {
    /// Shot counter since boot.
    pub shot_id: u16,
    /// Quality flags.
    pub flags: QualityFlags,
    /// Unix time of the shot (0 if the host never set the clock).
    pub unix_time: u32,
    /// Degrees, see [`crate::survey`].
    pub azimuth_deg: f32,
    /// Degrees.
    pub inclination_deg: f32,
    /// Degrees.
    pub roll_deg: f32,
    /// Degrees.
    pub dip_deg: f32,
    /// µT.
    pub b_total_ut: f32,
    /// Calibrated gravity norm (1 = nominal).
    pub g_norm: f32,
    /// Largest angle between triads, degrees.
    pub mag_spread_deg: f32,
    /// Angle between accelerometers, degrees.
    pub acc_spread_deg: f32,
    /// 1-σ azimuth uncertainty from noise, degrees.
    pub azimuth_sigma_deg: f32,
    /// 1-σ inclination uncertainty from noise, degrees.
    pub inclination_sigma_deg: f32,
    /// Peak gyro rate during the window, °/s.
    pub gyro_peak_dps: f32,
    /// Coil temperatures, °C.
    pub mag_temp_c: [f32; MAG_COUNT],
    /// Die temperatures, °C.
    pub acc_temp_c: [f32; ACC_COUNT],
    /// Calibration id used.
    pub calibration_id: u32,
    /// Magnetometer samples averaged.
    pub mag_samples: u16,
    /// Accelerometer samples averaged.
    pub acc_samples: u16,
    /// Window duration, ms.
    pub duration_ms: u16,
}

/// Per-sensor detail of a shot (sent when [`shot_flags::WANT_RAW`] was set).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShotRaw {
    /// Shot counter.
    pub shot_id: u16,
    /// Sensor tag ([`SensorId`]).
    pub sensor: u8,
    /// Samples averaged.
    pub count: u16,
    /// Mean raw counts.
    pub mean: [f32; 3],
    /// Sample standard deviation, raw counts.
    pub std_dev: [f32; 3],
    /// Calibrated vector (body frame).
    pub calibrated: [f32; 3],
}

/// One raw sample of a stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sample {
    /// Milliseconds since boot.
    pub t_ms: u32,
    /// Source tag: [`SensorId`] tag or [`sample_source`].
    pub sensor: u8,
    /// Raw counts.
    pub raw: [i32; 3],
}

/// Temperatures and supply, streamed at 1 Hz when requested.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Temps {
    /// Milliseconds since boot.
    pub t_ms: u32,
    /// Coil temperatures, °C (`NaN` when unreadable).
    pub mag_temp_c: [f32; MAG_COUNT],
    /// Accelerometer die temperatures, °C.
    pub acc_temp_c: [f32; ACC_COUNT],
    /// IMU die temperature, °C.
    pub imu_temp_c: f32,
    /// Analogue supply measured through VREFINT, mV.
    pub vdda_mv: u16,
}

/// A calibration shot as stored in the device buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CalShotReport {
    /// Slot index in the buffer.
    pub index: u8,
    /// Shots in the buffer after this one.
    pub count: u8,
    /// Quality flags of the shot as evaluated with the *current* calibration (informational).
    pub flags: QualityFlags,
    /// The stored data.
    pub shot: CalShot,
}

/// Outcome of a solve.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CalSolved {
    /// `None` on success, else the error.
    pub error: Option<SolveError>,
    /// Diagnostics (zeros on error).
    pub report: SolveReport,
    /// Fitted dip, degrees.
    pub dip_deg: f32,
    /// Fitted field, µT.
    pub field_ut: f32,
}

/// Summary of one stored temperature set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CalSetSummary {
    /// Mean coil temperature, °C.
    pub mag_temp_c: f32,
    /// Mean die temperature, °C.
    pub acc_temp_c: f32,
    /// RMS residual, degrees.
    pub residual_deg: f32,
    /// Shots used.
    pub shots: u16,
    /// Field, µT.
    pub field_ut: f32,
    /// Dip, degrees.
    pub dip_deg: f32,
}

/// Summary of the stored calibration.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CalInfo {
    /// Id.
    pub id: u32,
    /// Date, unix seconds.
    pub date_unix: u32,
    /// Cycle count.
    pub mag_cycle_count: u16,
    /// Valid entries in `sets`.
    pub n_sets: u8,
    /// Set summaries.
    pub sets: [CalSetSummary; MAX_SETS],
    /// Shots currently held in the calibration buffer (RAM).
    pub buffered_shots: u8,
    /// `true` while a calibration session is open.
    pub session_open: bool,
    /// `true` when a solved-but-not-stored set is pending.
    pub pending_set: bool,
}

/// Device event kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum EventKind {
    /// Firmware started (detail: reset reason bits, implementation-defined).
    Boot = 1,
    /// Calibration loaded from flash (detail: id).
    CalibrationLoaded = 2,
    /// No valid calibration in flash.
    CalibrationMissing = 3,
    /// A shot started (detail: shot id).
    ShotStarted = 4,
    /// A shot finished (detail: shot id).
    ShotFinished = 5,
    /// A stream stopped by itself (detail: reason).
    StreamStopped = 6,
    /// A sensor failed at runtime (detail: sensor tag).
    SensorFault = 7,
}

impl EventKind {
    /// From a wire tag.
    #[must_use]
    pub const fn from_tag(t: u8) -> Option<Self> {
        Some(match t {
            1 => Self::Boot,
            2 => Self::CalibrationLoaded,
            3 => Self::CalibrationMissing,
            4 => Self::ShotStarted,
            5 => Self::ShotFinished,
            6 => Self::StreamStopped,
            7 => Self::SensorFault,
            _ => return None,
        })
    }
}

/// Unsolicited event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Event {
    /// Kind.
    pub kind: EventKind,
    /// Kind-specific detail.
    pub detail: u32,
}

/// Maximum text length of a [`LogLine`].
pub const LOG_TEXT_MAX: usize = 96;

/// A short diagnostic text line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LogLine {
    /// 0 error, 1 warn, 2 info, 3 debug.
    pub level: u8,
    /// Bytes used in `text`.
    pub len: u8,
    /// UTF-8 text.
    #[cfg_attr(feature = "serde", serde(with = "serde_big_array"))]
    pub text: [u8; LOG_TEXT_MAX],
}

#[cfg(feature = "serde")]
mod serde_big_array {
    //! serde does not derive for arrays longer than 32; serialise the log text as a string.
    use super::LOG_TEXT_MAX;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; LOG_TEXT_MAX], s: S) -> Result<S::Ok, S::Error> {
        let end = v.iter().position(|&b| b == 0).unwrap_or(LOG_TEXT_MAX);
        core::str::from_utf8(&v[..end]).unwrap_or("").serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; LOG_TEXT_MAX], D::Error> {
        let s = <&str>::deserialize(d)?;
        let mut out = [0u8; LOG_TEXT_MAX];
        let n = s.len().min(LOG_TEXT_MAX);
        out[..n].copy_from_slice(&s.as_bytes()[..n]);
        Ok(out)
    }
}

impl LogLine {
    /// Builds a line, truncating the text.
    #[must_use]
    pub fn new(level: u8, text: &str) -> Self {
        let mut out = [0u8; LOG_TEXT_MAX];
        let mut n = text.len().min(LOG_TEXT_MAX);
        // Do not cut inside a UTF-8 sequence.
        while n > 0 && !text.is_char_boundary(n) {
            n -= 1;
        }
        out[..n].copy_from_slice(&text.as_bytes()[..n]);
        Self {
            level,
            len: n as u8,
            text: out,
        }
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..usize::from(self.len).min(LOG_TEXT_MAX)]).unwrap_or("")
    }
}

/// Largest firmware chunk per frame. A multiple of the STM32U5 flash write granularity (16 bytes).
pub const FW_CHUNK_MAX: usize = 1024;

/// Announces a firmware update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FwBegin {
    /// Image size in bytes (the raw binary of the application, linked at the active partition).
    pub size: u32,
    /// CRC-32/ISO-HDLC of the whole image, verified on the device after the last chunk.
    pub crc32: u32,
    /// Version of the new image (informational).
    pub version: [u8; 3],
}

/// One chunk of a firmware image.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FwChunk {
    /// Byte offset of this chunk in the image; must be a multiple of 16.
    pub offset: u32,
    len: u16,
    data: [u8; FW_CHUNK_MAX],
}

impl FwChunk {
    /// Builds a chunk; `None` when `data` is empty or longer than [`FW_CHUNK_MAX`].
    #[must_use]
    pub fn new(offset: u32, data: &[u8]) -> Option<Self> {
        if data.is_empty() || data.len() > FW_CHUNK_MAX {
            return None;
        }
        let mut buf = [0xFFu8; FW_CHUNK_MAX];
        buf[..data.len()].copy_from_slice(data);
        Some(Self {
            offset,
            len: data.len() as u16,
            data: buf,
        })
    }

    /// The chunk payload.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data[..usize::from(self.len)]
    }
}

impl core::fmt::Debug for FwChunk {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FwChunk")
            .field("offset", &self.offset)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for FwChunk {
    fn format(&self, f: defmt::Formatter<'_>) {
        defmt::write!(f, "FwChunk {{ offset: {}, len: {} }}", self.offset, self.len);
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for FwChunk {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("FwChunk", 2)?;
        st.serialize_field("offset", &self.offset)?;
        st.serialize_field("data", self.data())?;
        st.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for FwChunk {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            offset: u32,
            data: fixed_bytes::Bytes,
        }
        let raw = Raw::deserialize(d)?;
        Self::new(raw.offset, raw.data.as_slice()).ok_or_else(|| serde::de::Error::custom("bad chunk length"))
    }
}

#[cfg(feature = "serde")]
mod fixed_bytes {
    //! A fixed-capacity byte buffer deserialisable from a JSON array of numbers without `alloc`.
    use super::FW_CHUNK_MAX;
    use serde::de::{Deserialize, Deserializer, SeqAccess, Visitor};

    pub struct Bytes {
        buf: [u8; FW_CHUNK_MAX],
        len: usize,
    }

    impl Bytes {
        pub fn as_slice(&self) -> &[u8] {
            &self.buf[..self.len]
        }
    }

    impl<'de> Deserialize<'de> for Bytes {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Bytes;
                fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                    write!(f, "at most {FW_CHUNK_MAX} bytes")
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Bytes, A::Error> {
                    let mut out = Bytes {
                        buf: [0; FW_CHUNK_MAX],
                        len: 0,
                    };
                    while let Some(b) = seq.next_element::<u8>()? {
                        if out.len >= FW_CHUNK_MAX {
                            return Err(serde::de::Error::invalid_length(out.len + 1, &self));
                        }
                        out.buf[out.len] = b;
                        out.len += 1;
                    }
                    Ok(out)
                }
                fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Bytes, E> {
                    if v.len() > FW_CHUNK_MAX {
                        return Err(E::invalid_length(v.len(), &self));
                    }
                    let mut out = Bytes {
                        buf: [0; FW_CHUNK_MAX],
                        len: v.len(),
                    };
                    out.buf[..v.len()].copy_from_slice(v);
                    Ok(out)
                }
            }
            d.deserialize_seq(V)
        }
    }
}

// -------------------------------------------------------------------------------------------
// Messages
// -------------------------------------------------------------------------------------------

/// Message kinds, host → device.
pub mod host_kind {
    /// `Ping`.
    pub const PING: u8 = 0x01;
    /// `GetInfo`.
    pub const GET_INFO: u8 = 0x02;
    /// `SetTime`.
    pub const SET_TIME: u8 = 0x03;
    /// `Reset`.
    pub const RESET: u8 = 0x04;
    /// `Shot`.
    pub const SHOT: u8 = 0x10;
    /// `Abort`.
    pub const ABORT: u8 = 0x11;
    /// `Stream`.
    pub const STREAM: u8 = 0x12;
    /// `GetConfig`.
    pub const GET_CONFIG: u8 = 0x20;
    /// `SetConfig`.
    pub const SET_CONFIG: u8 = 0x21;
    /// `CalBegin`.
    pub const CAL_BEGIN: u8 = 0x30;
    /// `CalShot`.
    pub const CAL_SHOT: u8 = 0x31;
    /// `CalRemove`.
    pub const CAL_REMOVE: u8 = 0x32;
    /// `CalSolve`.
    pub const CAL_SOLVE: u8 = 0x33;
    /// `CalStore`.
    pub const CAL_STORE: u8 = 0x34;
    /// `CalLoad`.
    pub const CAL_LOAD: u8 = 0x35;
    /// `CalGet`.
    pub const CAL_GET: u8 = 0x36;
    /// `CalSet`.
    pub const CAL_SET: u8 = 0x37;
    /// `CalClear`.
    pub const CAL_CLEAR: u8 = 0x38;
    /// `CalInfo`.
    pub const CAL_INFO: u8 = 0x39;
    /// `CalGetShots`.
    pub const CAL_GET_SHOTS: u8 = 0x3A;
    /// `CalEnd`.
    pub const CAL_END: u8 = 0x3B;
    /// `FwBegin`.
    pub const FW_BEGIN: u8 = 0x40;
    /// `FwChunk`.
    pub const FW_CHUNK: u8 = 0x41;
    /// `FwFinish`.
    pub const FW_FINISH: u8 = 0x42;
    /// `FwAbort`.
    pub const FW_ABORT: u8 = 0x43;
}

/// Message kinds, device → host.
pub mod device_kind {
    /// `Ack`.
    pub const ACK: u8 = 0x80;
    /// `Info`.
    pub const INFO: u8 = 0x81;
    /// `Config`.
    pub const CONFIG: u8 = 0x82;
    /// `ShotResult`.
    pub const SHOT_RESULT: u8 = 0x90;
    /// `ShotRaw`.
    pub const SHOT_RAW: u8 = 0x91;
    /// `Sample`.
    pub const SAMPLE: u8 = 0x92;
    /// `Temps`.
    pub const TEMPS: u8 = 0x93;
    /// `CalShotDone`.
    pub const CAL_SHOT_DONE: u8 = 0xA0;
    /// `CalShotData`.
    pub const CAL_SHOT_DATA: u8 = 0xA1;
    /// `CalSolved`.
    pub const CAL_SOLVED: u8 = 0xA2;
    /// `CalData`.
    pub const CAL_DATA: u8 = 0xA3;
    /// `CalInfo`.
    pub const CAL_INFO: u8 = 0xA4;
    /// `Log`.
    pub const LOG: u8 = 0xF0;
    /// `Event`.
    pub const EVENT: u8 = 0xF1;
}

/// Host → device.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message is a transient value encoded straight into a frame buffer"
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", content = "data"))]
pub enum HostMessage {
    /// Liveness check → `Ack`.
    Ping,
    /// → `Info`.
    GetInfo,
    /// Sets the wall clock (unix seconds) → `Ack`.
    SetTime(u32),
    /// Reboots the device after the `Ack`.
    Reset,
    /// Takes a shot → `ShotResult` (+ `ShotRaw` ×5 with `WANT_RAW`).
    Shot(ShotRequest),
    /// Aborts a running shot or stream → `Ack`.
    Abort,
    /// Starts / stops raw streaming → `Ack`, then `Sample`s.
    Stream(StreamRequest),
    /// → `Config`.
    GetConfig,
    /// Applies and persists a configuration → `Ack`.
    SetConfig(DeviceConfig),
    /// Opens a calibration session (clears the buffer) → `Ack`.
    CalBegin,
    /// Takes a calibration shot into slot `(group, roll)` → `CalShotDone`.
    CalShot {
        /// Direction group.
        group: u8,
        /// Roll slot.
        roll: u8,
    },
    /// Removes a buffered shot → `Ack`.
    CalRemove {
        /// Direction group.
        group: u8,
        /// Roll slot.
        roll: u8,
    },
    /// Solves the buffered shots → `CalSolved`.
    CalSolve,
    /// Stores the pending solved set under `id` into the calibration and flash → `Ack`.
    CalStore {
        /// Calibration id to record.
        id: u32,
    },
    /// Reloads the calibration from flash → `Ack`.
    CalLoad,
    /// → `CalData` (full calibration).
    CalGet,
    /// Replaces the calibration (RAM + flash) → `Ack`.
    CalSet(Calibration),
    /// Erases the stored calibration → `Ack`.
    CalClear,
    /// → `CalInfo`.
    CalInfo,
    /// → `CalShotData` × n, then `Ack`.
    CalGetShots,
    /// Closes the calibration session, keeping the buffer → `Ack`.
    CalEnd,
    /// Starts a firmware update: erases the DFU partition → `Ack`.
    FwBegin(FwBegin),
    /// One chunk of the new image → `Ack` (detail = chunks received so far, low 16 bits).
    FwChunk(FwChunk),
    /// Verifies the staged image, marks it for swap and reboots → `Ack`, then reset.
    FwFinish,
    /// Abandons a started update → `Ack`.
    FwAbort,
}

impl HostMessage {
    /// Wire kind.
    #[must_use]
    pub const fn kind(&self) -> u8 {
        #[allow(clippy::wildcard_imports, reason = "the kind constants are used exhaustively below")]
        use host_kind::*;
        match self {
            Self::Ping => PING,
            Self::GetInfo => GET_INFO,
            Self::SetTime(_) => SET_TIME,
            Self::Reset => RESET,
            Self::Shot(_) => SHOT,
            Self::Abort => ABORT,
            Self::Stream(_) => STREAM,
            Self::GetConfig => GET_CONFIG,
            Self::SetConfig(_) => SET_CONFIG,
            Self::CalBegin => CAL_BEGIN,
            Self::CalShot { .. } => CAL_SHOT,
            Self::CalRemove { .. } => CAL_REMOVE,
            Self::CalSolve => CAL_SOLVE,
            Self::CalStore { .. } => CAL_STORE,
            Self::CalLoad => CAL_LOAD,
            Self::CalGet => CAL_GET,
            Self::CalSet(_) => CAL_SET,
            Self::CalClear => CAL_CLEAR,
            Self::CalInfo => CAL_INFO,
            Self::CalGetShots => CAL_GET_SHOTS,
            Self::CalEnd => CAL_END,
            Self::FwBegin(_) => FW_BEGIN,
            Self::FwChunk(_) => FW_CHUNK,
            Self::FwFinish => FW_FINISH,
            Self::FwAbort => FW_ABORT,
        }
    }

    /// Serialises the payload.
    pub fn write_payload(&self, w: &mut Writer<'_>) -> Result<(), WireError> {
        match self {
            Self::Ping
            | Self::GetInfo
            | Self::Reset
            | Self::Abort
            | Self::GetConfig
            | Self::CalBegin
            | Self::CalSolve
            | Self::CalLoad
            | Self::CalGet
            | Self::CalClear
            | Self::CalInfo
            | Self::CalGetShots
            | Self::CalEnd
            | Self::FwFinish
            | Self::FwAbort => Ok(()),
            Self::FwBegin(b) => {
                w.u32(b.size)?;
                w.u32(b.crc32)?;
                w.bytes(&b.version)
            }
            Self::FwChunk(c) => {
                w.u32(c.offset)?;
                w.bytes(c.data())
            }
            Self::SetTime(t) => w.u32(*t),
            Self::Shot(s) => {
                w.u8(s.flags)?;
                w.u16(s.mag_samples)?;
                w.u16(s.cycle_count)
            }
            Self::Stream(s) => {
                w.u8(u8::from(s.enable))?;
                w.u16(s.rate_hz)?;
                w.u8(s.sensors)
            }
            Self::SetConfig(c) => c.write(w),
            Self::CalShot { group, roll } | Self::CalRemove { group, roll } => {
                w.u8(*group)?;
                w.u8(*roll)
            }
            Self::CalStore { id } => w.u32(*id),
            Self::CalSet(c) => c.write(w),
        }
    }

    /// Parses a payload of the given kind. `Err(Invalid)` for an unknown kind.
    #[allow(clippy::too_many_lines, reason = "one match arm per message kind")]
    pub fn read_payload(kind: u8, payload: &[u8]) -> Result<Self, WireError> {
        #[allow(clippy::wildcard_imports, reason = "the kind constants are used exhaustively below")]
        use host_kind::*;
        let mut r = Reader::new(payload);
        let msg = match kind {
            PING => Self::Ping,
            GET_INFO => Self::GetInfo,
            SET_TIME => Self::SetTime(r.u32()?),
            RESET => Self::Reset,
            SHOT => Self::Shot(ShotRequest {
                flags: r.u8()?,
                mag_samples: r.u16()?,
                cycle_count: r.u16()?,
            }),
            ABORT => Self::Abort,
            STREAM => Self::Stream(StreamRequest {
                enable: r.u8()? != 0,
                rate_hz: r.u16()?,
                sensors: r.u8()?,
            }),
            GET_CONFIG => Self::GetConfig,
            SET_CONFIG => Self::SetConfig(DeviceConfig::read(&mut r)?),
            CAL_BEGIN => Self::CalBegin,
            CAL_SHOT => Self::CalShot {
                group: r.u8()?,
                roll: r.u8()?,
            },
            CAL_REMOVE => Self::CalRemove {
                group: r.u8()?,
                roll: r.u8()?,
            },
            CAL_SOLVE => Self::CalSolve,
            CAL_STORE => Self::CalStore { id: r.u32()? },
            CAL_LOAD => Self::CalLoad,
            CAL_GET => Self::CalGet,
            CAL_SET => Self::CalSet(Calibration::read(&mut r)?),
            CAL_CLEAR => Self::CalClear,
            CAL_INFO => Self::CalInfo,
            CAL_GET_SHOTS => Self::CalGetShots,
            CAL_END => Self::CalEnd,
            FW_BEGIN => Self::FwBegin(FwBegin {
                size: r.u32()?,
                crc32: r.u32()?,
                version: r.array()?,
            }),
            FW_CHUNK => {
                let offset = r.u32()?;
                Self::FwChunk(FwChunk::new(offset, r.rest()).ok_or(WireError::Invalid)?)
            }
            FW_FINISH => Self::FwFinish,
            FW_ABORT => Self::FwAbort,
            _ => return Err(WireError::Invalid),
        };
        Ok(msg)
    }

    /// Encodes a complete frame.
    #[allow(
        clippy::large_stack_arrays,
        reason = "one payload-sized scratch buffer; the firmware stack is sized for it"
    )]
    pub fn encode(&self, seq: u8, out: &mut [u8]) -> Result<usize, WireError> {
        let mut payload = [0u8; MAX_PAYLOAD];
        let mut w = Writer::new(&mut payload);
        self.write_payload(&mut w)?;
        let n = w.len();
        encode_frame(self.kind(), seq, &payload[..n], out)
    }
}

/// Device → host.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message is a transient value encoded straight into a frame buffer"
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", content = "data"))]
pub enum DeviceMessage {
    /// Generic reply.
    Ack(Ack),
    /// Identity and state.
    Info(Info),
    /// Current configuration.
    Config(DeviceConfig),
    /// Shot result.
    ShotResult(ShotResult),
    /// Per-sensor shot detail.
    ShotRaw(ShotRaw),
    /// Stream sample.
    Sample(Sample),
    /// Temperatures.
    Temps(Temps),
    /// A calibration shot was taken.
    CalShotDone(CalShotReport),
    /// A buffered calibration shot (dump).
    CalShotData(CalShotReport),
    /// Solve outcome.
    CalSolved(CalSolved),
    /// Full calibration.
    CalData(Calibration),
    /// Calibration summary.
    CalInfo(CalInfo),
    /// Diagnostic text.
    Log(LogLine),
    /// Event.
    Event(Event),
}

fn write_cal_shot(w: &mut Writer<'_>, s: &CalShot) -> Result<(), WireError> {
    w.u8(s.group)?;
    w.u8(s.roll)?;
    for v in &s.mag {
        vec3_write(w, *v)?;
    }
    for v in &s.acc {
        vec3_write(w, *v)?;
    }
    for t in s.mag_temp_c {
        w.f32(t)?;
    }
    for t in s.acc_temp_c {
        w.f32(t)?;
    }
    Ok(())
}

fn read_cal_shot(r: &mut Reader<'_>) -> Result<CalShot, WireError> {
    let mut s = CalShot {
        group: r.u8()?,
        roll: r.u8()?,
        ..CalShot::default()
    };
    for v in &mut s.mag {
        *v = vec3_read(r)?;
    }
    for v in &mut s.acc {
        *v = vec3_read(r)?;
    }
    for t in &mut s.mag_temp_c {
        *t = r.f32()?;
    }
    for t in &mut s.acc_temp_c {
        *t = r.f32()?;
    }
    Ok(s)
}

fn write_cal_shot_report(w: &mut Writer<'_>, rep: &CalShotReport) -> Result<(), WireError> {
    w.u8(rep.index)?;
    w.u8(rep.count)?;
    w.u16(rep.flags.0)?;
    write_cal_shot(w, &rep.shot)
}

fn read_cal_shot_report(r: &mut Reader<'_>) -> Result<CalShotReport, WireError> {
    Ok(CalShotReport {
        index: r.u8()?,
        count: r.u8()?,
        flags: QualityFlags(r.u16()?),
        shot: read_cal_shot(r)?,
    })
}

const fn solve_error_tag(e: Option<SolveError>) -> u8 {
    match e {
        None => 0,
        Some(SolveError::NotEnoughShots) => 1,
        Some(SolveError::GroupTooLarge) => 2,
        Some(SolveError::BadInput) => 3,
        Some(SolveError::Singular) => 4,
        Some(SolveError::NotConverged) => 5,
        Some(SolveError::ResidualTooLarge) => 6,
    }
}

const fn solve_error_from_tag(t: u8) -> Result<Option<SolveError>, WireError> {
    Ok(match t {
        0 => None,
        1 => Some(SolveError::NotEnoughShots),
        2 => Some(SolveError::GroupTooLarge),
        3 => Some(SolveError::BadInput),
        4 => Some(SolveError::Singular),
        5 => Some(SolveError::NotConverged),
        6 => Some(SolveError::ResidualTooLarge),
        _ => return Err(WireError::Invalid),
    })
}

/// Wire tag of a [`SolveError`] (for `Ack.detail`).
#[must_use]
pub const fn solve_error_detail(e: SolveError) -> u16 {
    solve_error_tag(Some(e)) as u16
}

impl DeviceMessage {
    /// Wire kind.
    #[must_use]
    pub const fn kind(&self) -> u8 {
        #[allow(clippy::wildcard_imports, reason = "the kind constants are used exhaustively below")]
        use device_kind::*;
        match self {
            Self::Ack(_) => ACK,
            Self::Info(_) => INFO,
            Self::Config(_) => CONFIG,
            Self::ShotResult(_) => SHOT_RESULT,
            Self::ShotRaw(_) => SHOT_RAW,
            Self::Sample(_) => SAMPLE,
            Self::Temps(_) => TEMPS,
            Self::CalShotDone(_) => CAL_SHOT_DONE,
            Self::CalShotData(_) => CAL_SHOT_DATA,
            Self::CalSolved(_) => CAL_SOLVED,
            Self::CalData(_) => CAL_DATA,
            Self::CalInfo(_) => CAL_INFO,
            Self::Log(_) => LOG,
            Self::Event(_) => EVENT,
        }
    }

    /// Serialises the payload.
    #[allow(clippy::too_many_lines, reason = "one match arm per message kind")]
    pub fn write_payload(&self, w: &mut Writer<'_>) -> Result<(), WireError> {
        match self {
            Self::Ack(a) => {
                w.u8(a.request)?;
                w.u8(a.status as u8)?;
                w.u16(a.detail)
            }
            Self::Info(i) => {
                w.u8(i.protocol)?;
                w.bytes(&i.firmware)?;
                w.u8(i.hardware_rev)?;
                w.bytes(&i.uid)?;
                w.u32(i.uptime_ms)?;
                w.u32(i.calibration_id)?;
                w.u8(i.calibration_sets)?;
                w.u32(i.calibration_date)?;
                w.u8(i.sensors_ok)?;
                w.u8(u8::from(i.time_set))?;
                w.u32(i.unix_time)?;
                w.u8(i.boot_state)
            }
            Self::Config(c) => c.write(w),
            Self::ShotResult(s) => {
                w.u16(s.shot_id)?;
                w.u16(s.flags.0)?;
                w.u32(s.unix_time)?;
                for v in [
                    s.azimuth_deg,
                    s.inclination_deg,
                    s.roll_deg,
                    s.dip_deg,
                    s.b_total_ut,
                    s.g_norm,
                    s.mag_spread_deg,
                    s.acc_spread_deg,
                    s.azimuth_sigma_deg,
                    s.inclination_sigma_deg,
                    s.gyro_peak_dps,
                ] {
                    w.f32(v)?;
                }
                for t in s.mag_temp_c {
                    w.f32(t)?;
                }
                for t in s.acc_temp_c {
                    w.f32(t)?;
                }
                w.u32(s.calibration_id)?;
                w.u16(s.mag_samples)?;
                w.u16(s.acc_samples)?;
                w.u16(s.duration_ms)
            }
            Self::ShotRaw(s) => {
                w.u16(s.shot_id)?;
                w.u8(s.sensor)?;
                w.u16(s.count)?;
                w.f32x3(s.mean)?;
                w.f32x3(s.std_dev)?;
                w.f32x3(s.calibrated)
            }
            Self::Sample(s) => {
                w.u32(s.t_ms)?;
                w.u8(s.sensor)?;
                for v in s.raw {
                    w.i32(v)?;
                }
                Ok(())
            }
            Self::Temps(t) => {
                w.u32(t.t_ms)?;
                for v in t.mag_temp_c {
                    w.f32(v)?;
                }
                for v in t.acc_temp_c {
                    w.f32(v)?;
                }
                w.f32(t.imu_temp_c)?;
                w.u16(t.vdda_mv)
            }
            Self::CalShotDone(rep) | Self::CalShotData(rep) => write_cal_shot_report(w, rep),
            Self::CalSolved(s) => {
                w.u8(solve_error_tag(s.error))?;
                w.u32(s.report.iterations)?;
                w.f32(s.report.cost as f32)?;
                w.f32(s.report.residual_deg as f32)?;
                for v in s.report.mag_residual_deg {
                    w.f32(v as f32)?;
                }
                for v in s.report.acc_residual_deg {
                    w.f32(v as f32)?;
                }
                w.u16(s.report.shots)?;
                w.f32(s.dip_deg)?;
                w.f32(s.field_ut)
            }
            Self::CalData(c) => c.write(w),
            Self::CalInfo(i) => {
                w.u32(i.id)?;
                w.u32(i.date_unix)?;
                w.u16(i.mag_cycle_count)?;
                w.u8(i.n_sets)?;
                w.u8(i.buffered_shots)?;
                w.u8(u8::from(i.session_open))?;
                w.u8(u8::from(i.pending_set))?;
                for s in &i.sets {
                    w.f32(s.mag_temp_c)?;
                    w.f32(s.acc_temp_c)?;
                    w.f32(s.residual_deg)?;
                    w.u16(s.shots)?;
                    w.f32(s.field_ut)?;
                    w.f32(s.dip_deg)?;
                }
                Ok(())
            }
            Self::Log(l) => {
                w.u8(l.level)?;
                w.bytes(&l.text[..usize::from(l.len).min(LOG_TEXT_MAX)])
            }
            Self::Event(e) => {
                w.u8(e.kind as u8)?;
                w.u32(e.detail)
            }
        }
    }

    /// Parses a payload of the given kind.
    #[allow(clippy::too_many_lines, reason = "one match arm per message kind")]
    pub fn read_payload(kind: u8, payload: &[u8]) -> Result<Self, WireError> {
        #[allow(clippy::wildcard_imports, reason = "the kind constants are used exhaustively below")]
        use device_kind::*;
        let mut r = Reader::new(payload);
        let msg = match kind {
            ACK => Self::Ack(Ack {
                request: r.u8()?,
                status: Status::from_tag(r.u8()?).ok_or(WireError::Invalid)?,
                detail: r.u16()?,
            }),
            INFO => Self::Info(Info {
                protocol: r.u8()?,
                firmware: r.array()?,
                hardware_rev: r.u8()?,
                uid: r.array()?,
                uptime_ms: r.u32()?,
                calibration_id: r.u32()?,
                calibration_sets: r.u8()?,
                calibration_date: r.u32()?,
                sensors_ok: r.u8()?,
                time_set: r.u8()? != 0,
                unix_time: r.u32()?,
                boot_state: r.u8()?,
            }),
            CONFIG => Self::Config(DeviceConfig::read(&mut r)?),
            SHOT_RESULT => {
                let shot_id = r.u16()?;
                let flags = QualityFlags(r.u16()?);
                let unix_time = r.u32()?;
                let mut f = [0f32; 11];
                for v in &mut f {
                    *v = r.f32()?;
                }
                let mut mag_temp_c = [0f32; MAG_COUNT];
                for t in &mut mag_temp_c {
                    *t = r.f32()?;
                }
                let mut acc_temp_c = [0f32; ACC_COUNT];
                for t in &mut acc_temp_c {
                    *t = r.f32()?;
                }
                Self::ShotResult(ShotResult {
                    shot_id,
                    flags,
                    unix_time,
                    azimuth_deg: f[0],
                    inclination_deg: f[1],
                    roll_deg: f[2],
                    dip_deg: f[3],
                    b_total_ut: f[4],
                    g_norm: f[5],
                    mag_spread_deg: f[6],
                    acc_spread_deg: f[7],
                    azimuth_sigma_deg: f[8],
                    inclination_sigma_deg: f[9],
                    gyro_peak_dps: f[10],
                    mag_temp_c,
                    acc_temp_c,
                    calibration_id: r.u32()?,
                    mag_samples: r.u16()?,
                    acc_samples: r.u16()?,
                    duration_ms: r.u16()?,
                })
            }
            SHOT_RAW => Self::ShotRaw(ShotRaw {
                shot_id: r.u16()?,
                sensor: r.u8()?,
                count: r.u16()?,
                mean: r.f32x3()?,
                std_dev: r.f32x3()?,
                calibrated: r.f32x3()?,
            }),
            SAMPLE => Self::Sample(Sample {
                t_ms: r.u32()?,
                sensor: r.u8()?,
                raw: [r.i32()?, r.i32()?, r.i32()?],
            }),
            TEMPS => {
                let t_ms = r.u32()?;
                let mut mag_temp_c = [0f32; MAG_COUNT];
                for t in &mut mag_temp_c {
                    *t = r.f32()?;
                }
                let mut acc_temp_c = [0f32; ACC_COUNT];
                for t in &mut acc_temp_c {
                    *t = r.f32()?;
                }
                Self::Temps(Temps {
                    t_ms,
                    mag_temp_c,
                    acc_temp_c,
                    imu_temp_c: r.f32()?,
                    vdda_mv: r.u16()?,
                })
            }
            CAL_SHOT_DONE => Self::CalShotDone(read_cal_shot_report(&mut r)?),
            CAL_SHOT_DATA => Self::CalShotData(read_cal_shot_report(&mut r)?),
            CAL_SOLVED => {
                let error = solve_error_from_tag(r.u8()?)?;
                let iterations = r.u32()?;
                let cost = f64::from(r.f32()?);
                let residual_deg = f64::from(r.f32()?);
                let mut mag_residual_deg = [0.0; MAG_COUNT];
                for v in &mut mag_residual_deg {
                    *v = f64::from(r.f32()?);
                }
                let mut acc_residual_deg = [0.0; ACC_COUNT];
                for v in &mut acc_residual_deg {
                    *v = f64::from(r.f32()?);
                }
                let shots = r.u16()?;
                Self::CalSolved(CalSolved {
                    error,
                    report: SolveReport {
                        iterations,
                        cost,
                        residual_deg,
                        mag_residual_deg,
                        acc_residual_deg,
                        shots,
                    },
                    dip_deg: r.f32()?,
                    field_ut: r.f32()?,
                })
            }
            CAL_DATA => Self::CalData(Calibration::read(&mut r)?),
            CAL_INFO => {
                let id = r.u32()?;
                let date_unix = r.u32()?;
                let mag_cycle_count = r.u16()?;
                let n_sets = r.u8()?;
                let buffered_shots = r.u8()?;
                let session_open = r.u8()? != 0;
                let pending_set = r.u8()? != 0;
                let mut sets = [CalSetSummary::default(); MAX_SETS];
                for s in &mut sets {
                    *s = CalSetSummary {
                        mag_temp_c: r.f32()?,
                        acc_temp_c: r.f32()?,
                        residual_deg: r.f32()?,
                        shots: r.u16()?,
                        field_ut: r.f32()?,
                        dip_deg: r.f32()?,
                    };
                }
                Self::CalInfo(CalInfo {
                    id,
                    date_unix,
                    mag_cycle_count,
                    n_sets,
                    sets,
                    buffered_shots,
                    session_open,
                    pending_set,
                })
            }
            LOG => {
                let level = r.u8()?;
                let rest = r.rest();
                let text = core::str::from_utf8(rest).map_err(|_| WireError::Invalid)?;
                Self::Log(LogLine::new(level, text))
            }
            EVENT => Self::Event(Event {
                kind: EventKind::from_tag(r.u8()?).ok_or(WireError::Invalid)?,
                detail: r.u32()?,
            }),
            _ => return Err(WireError::Invalid),
        };
        Ok(msg)
    }

    /// Encodes a complete frame.
    #[allow(
        clippy::large_stack_arrays,
        reason = "one payload-sized scratch buffer; the firmware stack is sized for it"
    )]
    pub fn encode(&self, seq: u8, out: &mut [u8]) -> Result<usize, WireError> {
        let mut payload = [0u8; MAX_PAYLOAD];
        let mut w = Writer::new(&mut payload);
        self.write_payload(&mut w)?;
        let n = w.len();
        encode_frame(self.kind(), seq, &payload[..n], out)
    }
}

/// Convenience: an `Ack`.
#[must_use]
pub const fn ack(request: u8, status: Status, detail: u16) -> DeviceMessage {
    DeviceMessage::Ack(Ack {
        request,
        status,
        detail,
    })
}

/// `SensorId` tag helper for `ShotRaw`.
#[must_use]
pub const fn sensor_tag(s: SensorId) -> u8 {
    s.tag()
}

#[cfg(test)]
#[allow(
    clippy::large_stack_arrays,
    clippy::too_many_lines,
    reason = "frame-sized buffers and one exhaustive roundtrip per message"
)]
mod tests {
    use super::*;
    use crate::calibration::SensorSet;

    #[test]
    fn cobs_vectors() {
        // Classic test vectors from the COBS paper / Wikipedia.
        let cases: [(&[u8], &[u8]); 6] = [
            (&[0x00], &[0x01, 0x01]),
            (&[0x00, 0x00], &[0x01, 0x01, 0x01]),
            (&[0x00, 0x11, 0x00], &[0x01, 0x02, 0x11, 0x01]),
            (&[0x11, 0x22, 0x00, 0x33], &[0x03, 0x11, 0x22, 0x02, 0x33]),
            (&[0x11, 0x22, 0x33, 0x44], &[0x05, 0x11, 0x22, 0x33, 0x44]),
            (&[0x11, 0x00, 0x00, 0x00], &[0x02, 0x11, 0x01, 0x01, 0x01]),
        ];
        for (plain, enc) in cases {
            let mut buf = [0u8; 16];
            let n = cobs::encode(plain, &mut buf).unwrap();
            assert_eq!(&buf[..n], enc, "encode {plain:x?}");
            let mut back = [0u8; 16];
            let m = cobs::decode(enc, &mut back).unwrap();
            assert_eq!(&back[..m], plain, "decode {enc:x?}");
        }
        // Long runs without zeros exercise the 0xFF code.
        let plain: Vec<u8> = (1..=600u32).map(|i| (i % 255 + 1) as u8).collect();
        let mut enc = vec![0u8; cobs::max_encoded_len(plain.len())];
        let n = cobs::encode(&plain, &mut enc).unwrap();
        assert!(!enc[..n].contains(&0));
        let mut back = vec![0u8; plain.len()];
        let m = cobs::decode(&enc[..n], &mut back).unwrap();
        assert_eq!(&back[..m], &plain[..]);
        let mut small = [0u8; 4];
        assert_eq!(cobs::decode(&enc[..n], &mut small), Err(WireError::BufferFull));
    }

    fn roundtrip_device(msg: &DeviceMessage) {
        let mut frame = [0u8; MAX_ENCODED];
        let n = msg.encode(0x5A, &mut frame).unwrap();
        assert_eq!(frame[n - 1], DELIMITER);
        assert!(!frame[..n - 1].contains(&DELIMITER));
        let mut dec = Decoder::new();
        let mut got = None;
        for &b in &frame[..n] {
            if let Some(r) = dec.push(b) {
                got = Some(r.unwrap());
            }
        }
        let hdr = got.expect("frame");
        assert_eq!(hdr.seq, 0x5A);
        assert_eq!(hdr.kind, msg.kind());
        let back = DeviceMessage::read_payload(hdr.kind, dec.payload()).unwrap();
        assert_eq!(back, *msg);
    }

    fn roundtrip_host(msg: &HostMessage) {
        let mut frame = [0u8; MAX_ENCODED];
        let n = msg.encode(7, &mut frame).unwrap();
        let mut dec = Decoder::new();
        let mut got = None;
        for &b in &frame[..n] {
            if let Some(r) = dec.push(b) {
                got = Some(r.unwrap());
            }
        }
        let hdr = got.expect("frame");
        assert_eq!(hdr.kind, msg.kind());
        let back = HostMessage::read_payload(hdr.kind, dec.payload()).unwrap();
        assert_eq!(back, *msg);
    }

    #[test]
    fn host_messages_roundtrip() {
        let mut cal = Calibration::empty(400);
        cal.id = 42;
        cal.push_set(SensorSet::nominal(400));
        for m in [
            HostMessage::Ping,
            HostMessage::GetInfo,
            HostMessage::SetTime(1_800_000_000),
            HostMessage::Reset,
            HostMessage::Shot(ShotRequest {
                flags: shot_flags::WANT_RAW,
                mag_samples: 50,
                cycle_count: 200,
            }),
            HostMessage::Abort,
            HostMessage::Stream(StreamRequest {
                enable: true,
                rate_hz: 100,
                sensors: stream_sensors::ALL,
            }),
            HostMessage::GetConfig,
            HostMessage::SetConfig(DeviceConfig::default()),
            HostMessage::CalBegin,
            HostMessage::CalShot { group: 3, roll: 1 },
            HostMessage::CalRemove { group: 3, roll: 1 },
            HostMessage::CalSolve,
            HostMessage::CalStore { id: 99 },
            HostMessage::CalLoad,
            HostMessage::CalGet,
            HostMessage::CalSet(cal),
            HostMessage::CalClear,
            HostMessage::CalInfo,
            HostMessage::CalGetShots,
            HostMessage::CalEnd,
            HostMessage::FwBegin(FwBegin {
                size: 123_456,
                crc32: 0xDEAD_BEEF,
                version: [0, 2, 0],
            }),
            HostMessage::FwChunk(FwChunk::new(4096, &[7u8; FW_CHUNK_MAX]).unwrap()),
            HostMessage::FwChunk(FwChunk::new(0, &[1, 2, 3]).unwrap()),
            HostMessage::FwFinish,
            HostMessage::FwAbort,
        ] {
            roundtrip_host(&m);
        }
    }

    #[test]
    fn device_messages_roundtrip() {
        let mut cal = Calibration::empty(400);
        cal.push_set(SensorSet::nominal(400));
        cal.push_set(SensorSet::nominal(400));
        let shot = CalShot {
            group: 2,
            roll: 3,
            mag: [Vec3::new(1.0, 2.0, 3.0); MAG_COUNT],
            acc: [Vec3::new(-4.0, 5.0, 6.5); ACC_COUNT],
            mag_temp_c: [20.5; MAG_COUNT],
            acc_temp_c: [24.25; ACC_COUNT],
        };
        for m in [
            ack(host_kind::PING, Status::Ok, 0),
            ack(
                host_kind::CAL_SOLVE,
                Status::SolveFailed,
                solve_error_detail(SolveError::Singular),
            ),
            DeviceMessage::Info(Info {
                protocol: 1,
                firmware: FIRMWARE_VERSION,
                hardware_rev: 2,
                uid: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
                uptime_ms: 123_456,
                calibration_id: 7,
                calibration_sets: 2,
                calibration_date: 1_700_000_000,
                sensors_ok: 0x3F,
                time_set: true,
                unix_time: 1_800_000_000,
                boot_state: 1,
            }),
            DeviceMessage::Config(DeviceConfig::default()),
            DeviceMessage::ShotResult(ShotResult {
                shot_id: 9,
                flags: QualityFlags::MAG_DISAGREE,
                unix_time: 5,
                azimuth_deg: 123.5,
                inclination_deg: -12.25,
                roll_deg: 3.0,
                dip_deg: 63.0,
                b_total_ut: 48.1,
                g_norm: 1.001,
                mag_spread_deg: 0.7,
                acc_spread_deg: 0.1,
                azimuth_sigma_deg: 0.02,
                inclination_sigma_deg: 0.01,
                gyro_peak_dps: 0.2,
                mag_temp_c: [20.0, 20.5, 21.0],
                acc_temp_c: [25.0, 25.5],
                calibration_id: 7,
                mag_samples: 100,
                acc_samples: 280,
                duration_ms: 1400,
            }),
            DeviceMessage::ShotRaw(ShotRaw {
                shot_id: 9,
                sensor: SensorId::Mag2.tag(),
                count: 100,
                mean: [1.0, 2.0, 3.0],
                std_dev: [0.1, 0.2, 0.3],
                calibrated: [0.5, 0.5, 0.7],
            }),
            DeviceMessage::Sample(Sample {
                t_ms: 1000,
                sensor: sample_source::IMU_GYRO,
                raw: [-1, 2, -3],
            }),
            DeviceMessage::Temps(Temps {
                t_ms: 2000,
                mag_temp_c: [20.0, 20.1, 20.2],
                acc_temp_c: [25.0, 25.1],
                imu_temp_c: 26.0,
                vdda_mv: 3245,
            }),
            DeviceMessage::CalShotDone(CalShotReport {
                index: 11,
                count: 12,
                flags: QualityFlags::default(),
                shot,
            }),
            DeviceMessage::CalShotData(CalShotReport {
                index: 0,
                count: 1,
                flags: QualityFlags::MOTION,
                shot,
            }),
            DeviceMessage::CalSolved(CalSolved {
                error: None,
                report: SolveReport {
                    iterations: 12,
                    cost: 0.5,
                    residual_deg: 0.0625,
                    mag_residual_deg: [0.0625, 0.125, 0.25],
                    acc_residual_deg: [0.0625, 0.5],
                    shots: 56,
                },
                dip_deg: 63.5,
                field_ut: 48.2,
            }),
            DeviceMessage::CalSolved(CalSolved {
                error: Some(SolveError::NotConverged),
                report: SolveReport::default(),
                dip_deg: 0.0,
                field_ut: 0.0,
            }),
            DeviceMessage::CalData(cal),
            DeviceMessage::CalInfo(CalInfo {
                id: 1,
                date_unix: 2,
                mag_cycle_count: 400,
                n_sets: 1,
                sets: [CalSetSummary {
                    mag_temp_c: 20.0,
                    acc_temp_c: 25.0,
                    residual_deg: 0.04,
                    shots: 56,
                    field_ut: 48.0,
                    dip_deg: 63.0,
                }; MAX_SETS],
                buffered_shots: 3,
                session_open: true,
                pending_set: false,
            }),
            DeviceMessage::Log(LogLine::new(2, "hello 🌍 world")),
            DeviceMessage::Event(Event {
                kind: EventKind::ShotFinished,
                detail: 9,
            }),
        ] {
            roundtrip_device(&m);
        }
    }

    #[test]
    fn decoder_resynchronises_and_rejects() {
        let mut frame = [0u8; MAX_ENCODED];
        let n = HostMessage::Ping.encode(1, &mut frame).unwrap();
        let mut dec = Decoder::new();
        // Garbage, then a corrupted frame, then a good one.
        for &b in b"\x05garbage" {
            assert!(dec.push(b).is_none());
        }
        assert_eq!(dec.push(0), Some(Err(FrameError::Cobs)));
        let mut corrupt = frame;
        corrupt[1] ^= 0x40;
        let mut result = None;
        for &b in &corrupt[..n] {
            if let Some(r) = dec.push(b) {
                result = Some(r);
            }
        }
        assert_eq!(result, Some(Err(FrameError::Crc)));
        // Idle delimiters are ignored.
        assert!(dec.push(0).is_none());
        let mut result = None;
        for &b in &frame[..n] {
            if let Some(r) = dec.push(b) {
                result = Some(r);
            }
        }
        assert_eq!(
            result,
            Some(Ok(FrameHeader {
                kind: host_kind::PING,
                seq: 1
            }))
        );
        // Too short.
        assert_eq!(dec.push(0x02), None);
        assert_eq!(dec.push(0x01), None);
        assert_eq!(dec.push(0), Some(Err(FrameError::TooShort)));
        // Overflow.
        for _ in 0..MAX_ENCODED + 10 {
            dec.push(1);
        }
        assert_eq!(dec.push(0), Some(Err(FrameError::Overflow)));
        // Unknown kinds are reported by the payload parsers.
        assert_eq!(HostMessage::read_payload(0x7F, &[]), Err(WireError::Invalid));
        assert_eq!(DeviceMessage::read_payload(0x7F, &[]), Err(WireError::Invalid));
    }

    #[test]
    fn config_validation_and_size() {
        let c = DeviceConfig::default();
        assert!(c.is_valid());
        let mut buf = [0u8; 128];
        let mut w = Writer::new(&mut buf);
        c.write(&mut w).unwrap();
        assert_eq!(w.len(), DeviceConfig::BYTES);
        let mut bad = c;
        bad.mag_samples = 0;
        assert!(!bad.is_valid());
        let mut bad = c;
        bad.thresholds.motion_dps = f32::NAN;
        assert!(!bad.is_valid());
        // The RM3100 driver refuses cycle counts outside 30..=4000; the config must too.
        let mut bad = c;
        bad.mag_cycle_count = CYCLE_COUNT_MAX + 1;
        assert!(!bad.is_valid());
        bad.mag_cycle_count = CYCLE_COUNT_MIN - 1;
        assert!(!bad.is_valid());
        bad.mag_cycle_count = CYCLE_COUNT_MAX;
        assert!(bad.is_valid());
    }

    #[test]
    fn log_line_truncates_on_char_boundary() {
        let long = "é".repeat(60);
        let l = LogLine::new(1, &long);
        assert!(usize::from(l.len) <= LOG_TEXT_MAX);
        assert_eq!(l.as_str().len(), usize::from(l.len));
        assert!(l.as_str().chars().all(|c| c == 'é'));
    }
}
