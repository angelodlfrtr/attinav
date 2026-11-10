//! Sensor calibration model.
//!
//! Every sensor is modelled affinely, after Heeb's `DistoX` formulation:
//!
//! ```text
//! calibrated = M · (raw − b)
//! ```
//!
//! with `b` the offset (hard iron / bias, in raw counts) and `M` a full 3×3 matrix that carries
//! gain, non-orthogonality, soft iron and the rotation into the body frame. Calibrated vectors are
//! dimensionless: `|g| = 1` for a resting accelerometer and `|m| = 1` for the local field at the
//! calibration site. Absolute magnitudes (µT, g) are reconstructed from the per-set
//! [`SensorSet::field_ut`] estimate and the nominal sensor gains.
//!
//! Temperature: a [`Calibration`] holds up to [`MAX_SETS`] sets, each tagged with the temperature of
//! every sensor at the time it was solved. At shot time the model is linearly interpolated between
//! the two sets bracketing the sensor's current temperature; outside the covered range the nearest
//! set is used and the shot is flagged as extrapolated (never with a single set: no range exists).
//!
//! The cycle count of the RM3100 is part of the calibration: the gain scales (almost) linearly with
//! it, so a shot taken at another cycle count is rescaled with [`rm3100_nominal_gain`] first.

use crate::math::{Mat3, Vec3};
use crate::wire::{Reader, WireError, Writer};

/// Number of magnetometer triads on the board.
pub const MAG_COUNT: usize = 3;
/// Number of accelerometers on the board.
pub const ACC_COUNT: usize = 2;
/// Maximum number of temperature sets in one calibration.
pub const MAX_SETS: usize = 4;

/// Format version written in front of every serialised calibration.
pub const FORMAT_VERSION: u8 = 1;

/// Serialised size of one [`Affine`].
pub const AFFINE_BYTES: usize = 12 * 8;
/// Serialised size of one [`SensorSet`].
pub const SET_BYTES: usize = (MAG_COUNT + ACC_COUNT) * 4 + (MAG_COUNT + ACC_COUNT) * AFFINE_BYTES + 8 + 8 + 4 + 4;
/// Serialised size of the [`Calibration`] header.
pub const HEADER_BYTES: usize = 16;
/// Upper bound of a serialised [`Calibration`].
pub const CALIBRATION_MAX_BYTES: usize = HEADER_BYTES + MAX_SETS * SET_BYTES;

/// Identifies one of the physical sensors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SensorId {
    /// RM3100 triad A (front).
    Mag1 = 0,
    /// RM3100 triad B (middle).
    Mag2 = 1,
    /// RM3100 triad C (rear).
    Mag3 = 2,
    /// SCA3300 #1 (defines the body frame).
    Acc1 = 3,
    /// SCA3300 #2 (mounted rotated 90° about Z).
    Acc2 = 4,
    /// ICM-42688-P (motion gating only, never calibrated here).
    Imu = 5,
}

impl SensorId {
    /// All sensors with a calibration entry, in wire order.
    pub const CALIBRATED: [Self; MAG_COUNT + ACC_COUNT] = [Self::Mag1, Self::Mag2, Self::Mag3, Self::Acc1, Self::Acc2];

    /// Wire tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        self as u8
    }

    /// From a wire tag.
    #[must_use]
    pub const fn from_tag(t: u8) -> Option<Self> {
        Some(match t {
            0 => Self::Mag1,
            1 => Self::Mag2,
            2 => Self::Mag3,
            3 => Self::Acc1,
            4 => Self::Acc2,
            5 => Self::Imu,
            _ => return None,
        })
    }

    /// Index among the magnetometers, if this is one.
    #[must_use]
    pub const fn mag_index(self) -> Option<usize> {
        match self {
            Self::Mag1 => Some(0),
            Self::Mag2 => Some(1),
            Self::Mag3 => Some(2),
            _ => None,
        }
    }

    /// Index among the accelerometers, if this is one.
    #[must_use]
    pub const fn acc_index(self) -> Option<usize> {
        match self {
            Self::Acc1 => Some(0),
            Self::Acc2 => Some(1),
            _ => None,
        }
    }
}

/// Nominal RM3100 gain in LSB/µT for a cycle count, from the datasheet fit
/// (`≈ 0.3671 · CC + 1.5`; 75 LSB/µT at the default 200).
#[must_use]
pub fn rm3100_nominal_gain(cycle_count: u16) -> f64 {
    0.3671 * f64::from(cycle_count) + 1.5
}

/// Nominal SCA3300 sensitivity in LSB/g for mode 4 (±1.5 g, 10 Hz).
pub const SCA3300_MODE4_LSB_PER_G: f64 = 5400.0;

/// One affine sensor model, `calibrated = matrix · (raw − offset)`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Affine {
    /// Offset in raw counts.
    pub offset: Vec3,
    /// Gain / alignment matrix, counts⁻¹.
    pub matrix: Mat3,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    /// Pass-through model.
    pub const IDENTITY: Self = Self {
        offset: Vec3::ZERO,
        matrix: Mat3::IDENTITY,
    };

    /// Pure scaling by `1 / counts_per_unit`, no offset: the model used before any calibration
    /// exists, so raw streaming already shows sensible numbers.
    #[must_use]
    pub fn nominal(counts_per_unit: f64) -> Self {
        Self {
            offset: Vec3::ZERO,
            matrix: Mat3::scaling(1.0 / counts_per_unit),
        }
    }

    /// Applies the model.
    #[must_use]
    pub fn apply(&self, raw: Vec3) -> Vec3 {
        self.matrix * (raw - self.offset)
    }

    /// Linear interpolation, `t ∈ [0, 1]`.
    #[must_use]
    pub fn lerp(&self, o: &Self, t: f64) -> Self {
        Self {
            offset: self.offset * (1.0 - t) + o.offset * t,
            matrix: self.matrix * (1.0 - t) + o.matrix * t,
        }
    }

    /// Serialises as 12 `f64` (offset, then the row-major matrix).
    pub fn write(&self, w: &mut Writer<'_>) -> Result<(), WireError> {
        for v in self.offset.to_array() {
            w.f64(v)?;
        }
        for v in self.matrix.to_flat() {
            w.f64(v)?;
        }
        Ok(())
    }

    /// Inverse of [`Self::write`].
    pub fn read(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let offset = Vec3::new(r.f64()?, r.f64()?, r.f64()?);
        let mut flat = [0.0; 9];
        for v in &mut flat {
            *v = r.f64()?;
        }
        let a = Self {
            offset,
            matrix: Mat3::from_flat(flat),
        };
        if !a.is_finite() {
            return Err(WireError::Invalid);
        }
        Ok(a)
    }

    /// `true` when every coefficient is a finite number.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.offset.to_array().iter().all(|v| v.is_finite()) && self.matrix.to_flat().iter().all(|v| v.is_finite())
    }
}

/// The complete model of all sensors at one temperature.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SensorSet {
    /// Coil temperature of each triad (Pt1000) when the set was solved, °C.
    pub mag_temp_c: [f32; MAG_COUNT],
    /// Die temperature of each accelerometer when the set was solved, °C.
    pub acc_temp_c: [f32; ACC_COUNT],
    /// Magnetometer models.
    pub mag: [Affine; MAG_COUNT],
    /// Accelerometer models. `acc[0]` carries the roll gauge (`matrix.m[2][1] == 0`).
    pub acc: [Affine; ACC_COUNT],
    /// Fitted magnetic dip at the calibration site, degrees (informational).
    pub dip_deg: f64,
    /// Estimated field magnitude at the calibration site, µT, from the nominal RM3100 gain; turns
    /// the unit-norm calibrated field back into µT.
    pub field_ut: f64,
    /// RMS angular residual of the fit, degrees.
    pub residual_deg: f32,
    /// Number of shots the set was solved from.
    pub shots: u16,
}

impl Default for SensorSet {
    fn default() -> Self {
        Self::nominal(400)
    }
}

impl SensorSet {
    /// Uncalibrated placeholder using nominal sensor gains for the given cycle count.
    #[must_use]
    pub fn nominal(cycle_count: u16) -> Self {
        let mag = Affine::nominal(rm3100_nominal_gain(cycle_count) * 50.0);
        let acc = Affine::nominal(SCA3300_MODE4_LSB_PER_G);
        Self {
            mag_temp_c: [20.0; MAG_COUNT],
            acc_temp_c: [20.0; ACC_COUNT],
            mag: [mag; MAG_COUNT],
            acc: [acc; ACC_COUNT],
            dip_deg: 0.0,
            field_ut: 50.0,
            residual_deg: 0.0,
            shots: 0,
        }
    }

    /// Serialises into exactly [`SET_BYTES`] bytes.
    pub fn write(&self, w: &mut Writer<'_>) -> Result<(), WireError> {
        for t in self.mag_temp_c {
            w.f32(t)?;
        }
        for t in self.acc_temp_c {
            w.f32(t)?;
        }
        for a in &self.mag {
            a.write(w)?;
        }
        for a in &self.acc {
            a.write(w)?;
        }
        w.f64(self.dip_deg)?;
        w.f64(self.field_ut)?;
        w.f32(self.residual_deg)?;
        w.u16(self.shots)?;
        w.u16(0)
    }

    /// Inverse of [`Self::write`].
    pub fn read(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let mut s = Self::nominal(400);
        for t in &mut s.mag_temp_c {
            *t = r.f32()?;
        }
        for t in &mut s.acc_temp_c {
            *t = r.f32()?;
        }
        for a in &mut s.mag {
            *a = Affine::read(r)?;
        }
        for a in &mut s.acc {
            *a = Affine::read(r)?;
        }
        s.dip_deg = r.f64()?;
        s.field_ut = r.f64()?;
        s.residual_deg = r.f32()?;
        s.shots = r.u16()?;
        let _pad = r.u16()?;
        if !(s.dip_deg.is_finite() && s.field_ut.is_finite() && s.residual_deg.is_finite()) {
            return Err(WireError::Invalid);
        }
        // Set temperatures drive the interpolation; a NaN there would poison every lookup.
        if !s.mag_temp_c.iter().chain(s.acc_temp_c.iter()).all(|t| t.is_finite()) {
            return Err(WireError::Invalid);
        }
        Ok(s)
    }
}

/// Result of a temperature lookup.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Interpolated {
    /// The model to apply.
    pub affine: Affine,
    /// `true` when the requested temperature was outside the calibrated range (nearest set used).
    pub extrapolated: bool,
    /// Field magnitude (µT) to use with this model (magnetometers only; 1.0 for accelerometers).
    pub field_ut: f64,
}

/// A complete calibration: identity, date, cycle count and up to [`MAX_SETS`] temperature sets.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Calibration {
    /// Identifier chosen by the host when the calibration is stored (0 = none).
    pub id: u32,
    /// Unix time (seconds) of the solve, as supplied by the carrier (0 = unknown).
    pub date_unix: u32,
    /// RM3100 cycle count the magnetometer models were solved for.
    pub mag_cycle_count: u16,
    /// Number of valid entries in `sets`.
    pub n_sets: u8,
    /// Temperature sets, valid up to `n_sets`.
    pub sets: [SensorSet; MAX_SETS],
}

impl Default for Calibration {
    fn default() -> Self {
        Self::empty(400)
    }
}

impl Calibration {
    /// A calibration with no sets: everything falls back to the nominal gains.
    #[must_use]
    pub fn empty(mag_cycle_count: u16) -> Self {
        Self {
            id: 0,
            date_unix: 0,
            mag_cycle_count,
            n_sets: 0,
            sets: [SensorSet::nominal(mag_cycle_count); MAX_SETS],
        }
    }

    /// `true` when no solved set is present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.n_sets == 0
    }

    /// The valid sets.
    #[must_use]
    pub fn sets(&self) -> &[SensorSet] {
        &self.sets[..(self.n_sets as usize).min(MAX_SETS)]
    }

    /// Appends a set; returns `false` when full.
    pub const fn push_set(&mut self, set: SensorSet) -> bool {
        let n = self.n_sets as usize;
        if n >= MAX_SETS {
            return false;
        }
        self.sets[n] = set;
        self.n_sets += 1;
        true
    }

    /// Replaces the set whose temperatures are closest to `set`'s, or appends when there is room,
    /// or replaces the closest one when full. Returns the index used.
    ///
    /// Distance is the largest per-sensor temperature difference over coils and dies alike.
    pub fn upsert_set(&mut self, set: SensorSet, same_temperature_tol_c: f32) -> usize {
        let dist = |s: &SensorSet| {
            let mut d: f32 = 0.0;
            for i in 0..MAG_COUNT {
                d = d.max((s.mag_temp_c[i] - set.mag_temp_c[i]).abs());
            }
            for i in 0..ACC_COUNT {
                d = d.max((s.acc_temp_c[i] - set.acc_temp_c[i]).abs());
            }
            d
        };
        let mut best: Option<(usize, f32)> = None;
        for (i, s) in self.sets().iter().enumerate() {
            let d = dist(s);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        match best {
            Some((i, d)) if d <= same_temperature_tol_c || self.n_sets as usize >= MAX_SETS => {
                self.sets[i] = set;
                i
            }
            _ => {
                let i = self.n_sets as usize;
                self.push_set(set);
                i
            }
        }
    }

    /// Model for one sensor at the given temperature.
    ///
    /// Returns `None` for the IMU (never calibrated).
    #[must_use]
    pub fn lookup(&self, sensor: SensorId, temp_c: f64) -> Option<Interpolated> {
        if let Some(k) = sensor.mag_index() {
            return Some(self.interpolate(temp_c, |s| f64::from(s.mag_temp_c[k]), |s| s.mag[k], |s| s.field_ut));
        }
        if let Some(k) = sensor.acc_index() {
            return Some(self.interpolate(temp_c, |s| f64::from(s.acc_temp_c[k]), |s| s.acc[k], |_| 1.0));
        }
        None
    }

    fn interpolate(
        &self,
        temp_c: f64,
        temp_of: impl Fn(&SensorSet) -> f64,
        affine_of: impl Fn(&SensorSet) -> Affine,
        field_of: impl Fn(&SensorSet) -> f64,
    ) -> Interpolated {
        let sets = self.sets();
        if sets.is_empty() {
            let s = &self.sets[0];
            return Interpolated {
                affine: affine_of(s),
                extrapolated: true,
                field_ut: field_of(s),
            };
        }
        // Nearest set below (or at) and nearest above the requested temperature.
        let mut below: Option<(&SensorSet, f64)> = None;
        let mut above: Option<(&SensorSet, f64)> = None;
        for s in sets {
            let t = temp_of(s);
            if t <= temp_c && below.is_none_or(|(_, bt)| t > bt) {
                below = Some((s, t));
            }
            if t >= temp_c && above.is_none_or(|(_, at)| t < at) {
                above = Some((s, t));
            }
        }
        match (below, above) {
            (Some((lo, t_lo)), Some((hi, t_hi))) => {
                let t = if t_hi > t_lo {
                    (temp_c - t_lo) / (t_hi - t_lo)
                } else {
                    0.0
                };
                Interpolated {
                    affine: affine_of(lo).lerp(&affine_of(hi), t),
                    extrapolated: false,
                    field_ut: field_of(lo) * (1.0 - t) + field_of(hi) * t,
                }
            }
            (Some((s, _)), None) | (None, Some((s, _))) => Interpolated {
                affine: affine_of(s),
                extrapolated: sets.len() > 1,
                field_ut: field_of(s),
            },
            // Only reachable with a NaN temperature (requested or stored): no comparison holds.
            // Fall back to the first set and say so rather than panic in the shot path.
            (None, None) => Interpolated {
                affine: affine_of(&sets[0]),
                extrapolated: true,
                field_ut: field_of(&sets[0]),
            },
        }
    }

    /// Raw-count scale to apply to a magnetometer reading taken at `shot_cycle_count` before using
    /// a model solved at `self.mag_cycle_count`.
    #[must_use]
    pub fn mag_rescale(&self, shot_cycle_count: u16) -> f64 {
        if shot_cycle_count == self.mag_cycle_count || shot_cycle_count == 0 {
            1.0
        } else {
            rm3100_nominal_gain(self.mag_cycle_count) / rm3100_nominal_gain(shot_cycle_count)
        }
    }

    /// Serialises the calibration; returns the number of bytes written.
    pub fn write(&self, w: &mut Writer<'_>) -> Result<(), WireError> {
        w.u8(FORMAT_VERSION)?;
        w.u8(self.n_sets.min(MAX_SETS as u8))?;
        w.u16(self.mag_cycle_count)?;
        w.u32(self.id)?;
        w.u32(self.date_unix)?;
        w.u32(0)?;
        for s in self.sets() {
            s.write(w)?;
        }
        Ok(())
    }

    /// Serialises into `buf`, returning the used prefix length.
    pub fn to_bytes(&self, buf: &mut [u8]) -> Result<usize, WireError> {
        let mut w = Writer::new(buf);
        self.write(&mut w)?;
        Ok(w.len())
    }

    /// Inverse of [`Self::write`].
    pub fn read(r: &mut Reader<'_>) -> Result<Self, WireError> {
        if r.u8()? != FORMAT_VERSION {
            return Err(WireError::Invalid);
        }
        let n_sets = r.u8()?;
        if n_sets as usize > MAX_SETS {
            return Err(WireError::Invalid);
        }
        let mag_cycle_count = r.u16()?;
        if mag_cycle_count == 0 {
            // Would make `mag_rescale` divide by the gain at CC = 0 (1.5 LSB/µT).
            return Err(WireError::Invalid);
        }
        let id = r.u32()?;
        let date_unix = r.u32()?;
        let _reserved = r.u32()?;
        let mut cal = Self::empty(mag_cycle_count);
        cal.id = id;
        cal.date_unix = date_unix;
        for _ in 0..n_sets {
            let set = SensorSet::read(r)?;
            cal.push_set(set);
        }
        Ok(cal)
    }

    /// Inverse of [`Self::to_bytes`].
    pub fn from_bytes(buf: &[u8]) -> Result<Self, WireError> {
        Self::read(&mut Reader::new(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_at(t: f32, gain: f64) -> SensorSet {
        let mut s = SensorSet::nominal(400);
        s.mag_temp_c = [t; MAG_COUNT];
        s.acc_temp_c = [t; ACC_COUNT];
        s.mag[1].offset = Vec3::new(f64::from(t), 0.0, 0.0);
        s.mag[1].matrix = Mat3::scaling(gain);
        s.field_ut = 40.0 + f64::from(t);
        s
    }

    #[test]
    fn interpolation_and_extrapolation() {
        let mut cal = Calibration::empty(400);
        assert!(cal.lookup(SensorId::Mag2, 15.0).unwrap().extrapolated);
        cal.push_set(set_at(10.0, 1.0));
        cal.push_set(set_at(30.0, 3.0));
        let mid = cal.lookup(SensorId::Mag2, 20.0).unwrap();
        assert!(!mid.extrapolated);
        assert!((mid.affine.offset.x - 20.0).abs() < 1e-12);
        assert!((mid.affine.matrix.m[0][0] - 2.0).abs() < 1e-12);
        assert!((mid.field_ut - 60.0).abs() < 1e-12);
        let cold = cal.lookup(SensorId::Mag2, -5.0).unwrap();
        assert!(cold.extrapolated);
        assert!((cold.affine.offset.x - 10.0).abs() < 1e-12);
        let hot = cal.lookup(SensorId::Mag2, 50.0).unwrap();
        assert!(hot.extrapolated);
        assert!((hot.affine.matrix.m[0][0] - 3.0).abs() < 1e-12);
        assert!(cal.lookup(SensorId::Imu, 20.0).is_none());
        assert!(!cal.lookup(SensorId::Mag2, 10.0).unwrap().extrapolated);
        // A NaN temperature (failed thermometer) must degrade, never panic.
        let nan = cal.lookup(SensorId::Mag2, f64::NAN).unwrap();
        assert!(nan.extrapolated);
        assert!(nan.affine.is_finite());
    }

    #[test]
    fn codec_rejects_poisonous_values() {
        let mut buf = [0u8; CALIBRATION_MAX_BYTES];
        let mut cal = Calibration::empty(400);
        cal.push_set(set_at(10.0, 1.0));
        let n = cal.to_bytes(&mut buf).unwrap();
        assert_eq!(Calibration::from_bytes(&buf[..n]).unwrap(), cal);

        let mut bad = cal;
        bad.sets[0].acc_temp_c[1] = f32::NAN;
        let n = bad.to_bytes(&mut buf).unwrap();
        assert_eq!(Calibration::from_bytes(&buf[..n]), Err(WireError::Invalid));

        let mut bad = cal;
        bad.mag_cycle_count = 0;
        let n = bad.to_bytes(&mut buf).unwrap();
        assert_eq!(Calibration::from_bytes(&buf[..n]), Err(WireError::Invalid));
    }

    #[test]
    fn upsert_replaces_close_temperatures() {
        let mut cal = Calibration::empty(400);
        assert_eq!(cal.upsert_set(set_at(10.0, 1.0), 2.0), 0);
        assert_eq!(cal.upsert_set(set_at(11.0, 2.0), 2.0), 0);
        assert_eq!(cal.n_sets, 1);
        assert_eq!(cal.upsert_set(set_at(30.0, 2.0), 2.0), 1);
        assert_eq!(cal.upsert_set(set_at(50.0, 2.0), 2.0), 2);
        assert_eq!(cal.upsert_set(set_at(70.0, 2.0), 2.0), 3);
        // Full: the closest set is replaced.
        assert_eq!(cal.upsert_set(set_at(66.0, 9.0), 2.0), 3);
        assert_eq!(cal.n_sets, 4);
    }

    #[test]
    fn serialisation_roundtrip() {
        let mut cal = Calibration::empty(400);
        cal.id = 0xDEAD_BEEF;
        cal.date_unix = 1_800_000_000;
        cal.push_set(set_at(10.0, 1.5));
        cal.push_set(set_at(30.0, 2.5));
        let mut buf = [0u8; CALIBRATION_MAX_BYTES];
        let n = cal.to_bytes(&mut buf).unwrap();
        assert_eq!(n, HEADER_BYTES + 2 * SET_BYTES);
        let back = Calibration::from_bytes(&buf[..n]).unwrap();
        assert_eq!(back, cal);
        // Corrupt the version byte.
        buf[0] = 99;
        assert_eq!(Calibration::from_bytes(&buf[..n]), Err(WireError::Invalid));
        // Truncated.
        buf[0] = FORMAT_VERSION;
        assert_eq!(Calibration::from_bytes(&buf[..n - 1]), Err(WireError::Truncated));
    }

    #[test]
    fn rescale_follows_nominal_gain() {
        let cal = Calibration::empty(400);
        assert_eq!(cal.mag_rescale(400), 1.0);
        let s = cal.mag_rescale(200);
        assert!((s - rm3100_nominal_gain(400) / rm3100_nominal_gain(200)).abs() < 1e-12);
        assert!(s > 1.9 && s < 2.1);
    }
}
