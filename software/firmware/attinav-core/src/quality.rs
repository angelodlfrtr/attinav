//! Turning one shot's raw statistics into a calibrated, cross-checked result.
//!
//! The board carries three magnetometer triads and two accelerometers precisely so that every shot
//! can be checked against itself: a nearby piece of iron bends the field differently at the three
//! triad positions, a moving instrument shows up as a gyro rate and as disagreeing accelerometers,
//! an anomalous site shows up as a field magnitude or dip away from the IGRF reference.

use crate::calibration::{ACC_COUNT, Calibration, MAG_COUNT, SensorId};
use crate::math::{Vec3, rad_to_deg};
use crate::stats::SampleStats;
use crate::survey::{Attitude, attitude};

/// Quality flags of a shot; zero means every check passed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QualityFlags(pub u16);

impl QualityFlags {
    /// No calibration stored: nominal gains used, azimuth meaningless.
    pub const NO_CALIBRATION: Self = Self(1 << 0);
    /// Sensor temperature outside the calibrated range (nearest set used).
    pub const TEMP_EXTRAPOLATED: Self = Self(1 << 1);
    /// The three triads disagree beyond the threshold: field gradient, i.e. nearby iron.
    pub const MAG_DISAGREE: Self = Self(1 << 2);
    /// The two accelerometers disagree beyond the threshold.
    pub const ACC_DISAGREE: Self = Self(1 << 3);
    /// Calibrated `|B|` differs from the calibration-site (or reference) value.
    pub const MAG_NORM: Self = Self(1 << 4);
    /// Calibrated `|g|` differs from 1.
    pub const ACC_NORM: Self = Self(1 << 5);
    /// Dip differs from the calibration-site (or reference) value.
    pub const DIP: Self = Self(1 << 6);
    /// Gyro saw motion during the integration window.
    pub const MOTION: Self = Self(1 << 7);
    /// Magnetometer sample noise unusually high (interference during the window).
    pub const MAG_NOISY: Self = Self(1 << 8);
    /// Accelerometer sample noise unusually high (vibration).
    pub const ACC_NOISY: Self = Self(1 << 9);
    /// A sensor failed or timed out; its data was excluded.
    pub const SENSOR_FAULT: Self = Self(1 << 10);
    /// The `QUIET` line was not asserted (host asked for a fast / noisy shot).
    pub const QUIET_SKIPPED: Self = Self(1 << 11);
    /// Fewer samples than configured were collected.
    pub const SHORT: Self = Self(1 << 12);
    /// Shot taken at a cycle count other than the calibration's (gain rescaled nominally).
    pub const CYCLE_COUNT_MISMATCH: Self = Self(1 << 13);

    /// Set `f`.
    pub const fn set(&mut self, f: Self) {
        self.0 |= f.0;
    }

    /// `true` when `f` is set.
    #[must_use]
    pub const fn has(self, f: Self) -> bool {
        self.0 & f.0 != 0
    }

    /// `true` when no flag is set.
    #[must_use]
    pub const fn is_clean(self) -> bool {
        self.0 == 0
    }

    /// Flags that invalidate the azimuth / inclination for survey use.
    pub const SEVERE: Self = Self(
        Self::NO_CALIBRATION.0 | Self::MAG_DISAGREE.0 | Self::ACC_DISAGREE.0 | Self::MOTION.0 | Self::SENSOR_FAULT.0,
    );

    /// `true` when a severe flag is set.
    #[must_use]
    pub const fn is_severe(self) -> bool {
        self.0 & Self::SEVERE.0 != 0
    }
}

/// Thresholds for the checks. Defaults are conservative starting points; tune on the real board.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Thresholds {
    /// Maximum angle between any two triads' calibrated field directions, degrees.
    pub mag_disagree_deg: f32,
    /// Maximum angle between the two accelerometers' calibrated gravity directions, degrees.
    pub acc_disagree_deg: f32,
    /// Relative tolerance on the calibrated field norm (`1 ± tol`).
    pub mag_norm_tol: f32,
    /// Relative tolerance on the calibrated gravity norm.
    pub acc_norm_tol: f32,
    /// Tolerance on the dip angle, degrees.
    pub dip_tol_deg: f32,
    /// Gyro peak rate above which the window counts as moved, °/s.
    pub motion_dps: f32,
    /// Per-axis sample standard deviation above which a triad counts as noisy, relative to `|B|`.
    pub mag_noise_rel: f32,
    /// Per-axis sample standard deviation above which an accelerometer counts as noisy, relative to g.
    pub acc_noise_rel: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            mag_disagree_deg: 0.5,
            acc_disagree_deg: 0.3,
            mag_norm_tol: 0.03,
            acc_norm_tol: 0.02,
            dip_tol_deg: 1.0,
            motion_dps: 0.5,
            mag_noise_rel: 0.004,
            acc_noise_rel: 0.01,
        }
    }
}

/// Optional external reference for the site (IGRF values from the carrier).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FieldReference {
    /// Total field, µT.
    pub b_total_ut: f64,
    /// Dip (inclination of the field), degrees.
    pub dip_deg: f64,
}

/// Everything the engine measured during one shot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ShotInput {
    /// Raw statistics per triad; `count == 0` marks a failed sensor.
    pub mag: [SampleStats; MAG_COUNT],
    /// Raw statistics per accelerometer; `count == 0` marks a failed sensor.
    pub acc: [SampleStats; ACC_COUNT],
    /// Peak gyro rate magnitude during the window, °/s (`None` when the IMU failed).
    pub gyro_peak_dps: Option<f64>,
    /// Coil temperatures, °C (`None` when the reading failed).
    pub mag_temp_c: [Option<f64>; MAG_COUNT],
    /// Die temperatures, °C.
    pub acc_temp_c: [Option<f64>; ACC_COUNT],
    /// RM3100 cycle count used.
    pub mag_cycle_count: u16,
    /// `true` when `QUIET` was asserted for the window.
    pub quiet_asserted: bool,
    /// `true` when fewer samples than requested were collected.
    pub short: bool,
}

/// Calibrated, checked result.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShotEvaluation {
    /// Attitude from the fused vectors (zeros when it could not be computed).
    pub attitude: Attitude,
    /// Quality flags.
    pub flags: QualityFlags,
    /// Calibrated field per triad (unit-ish, body frame).
    pub mag_cal: [Vec3; MAG_COUNT],
    /// Calibrated gravity direction per accelerometer (unit-ish, body frame, points down).
    pub acc_cal: [Vec3; ACC_COUNT],
    /// Fused field vector, µT.
    pub field_ut: Vec3,
    /// Fused gravity direction (unit).
    pub gravity: Vec3,
    /// Largest angle between two triads, degrees.
    pub mag_spread_deg: f64,
    /// Angle between the two accelerometers, degrees.
    pub acc_spread_deg: f64,
    /// `|B|` of the fused vector, µT.
    pub b_total_ut: f64,
    /// Mean calibrated gravity norm (1 = nominal).
    pub g_norm: f64,
    /// 1-σ azimuth uncertainty from sample noise alone, degrees.
    pub azimuth_sigma_deg: f64,
    /// 1-σ inclination uncertainty from sample noise alone, degrees.
    pub inclination_sigma_deg: f64,
    /// Temperatures used for the lookup, °C.
    pub mag_temp_c: [f64; MAG_COUNT],
    /// Temperatures used for the lookup, °C.
    pub acc_temp_c: [f64; ACC_COUNT],
}

/// Evaluates one shot.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one linear pass over the checks; splitting would scatter the flag logic"
)]
pub fn evaluate(
    input: &ShotInput,
    cal: &Calibration,
    thr: &Thresholds,
    reference: Option<FieldReference>,
) -> ShotEvaluation {
    let mut out = ShotEvaluation::default();
    let mut flags = QualityFlags::default();
    if cal.is_empty() {
        flags.set(QualityFlags::NO_CALIBRATION);
    }
    if !input.quiet_asserted {
        flags.set(QualityFlags::QUIET_SKIPPED);
    }
    if input.short {
        flags.set(QualityFlags::SHORT);
    }
    let rescale = cal.mag_rescale(input.mag_cycle_count);
    if rescale != 1.0 {
        flags.set(QualityFlags::CYCLE_COUNT_MISMATCH);
    }

    // --- Magnetometers. ---
    let mut field_ut_scale = [0.0; MAG_COUNT];
    let mut mag_ok = [false; MAG_COUNT];
    let mut mag_sigma_rel = 0.0f64;
    for k in 0..MAG_COUNT {
        let st = &input.mag[k];
        let temp = input.mag_temp_c[k].unwrap_or(20.0);
        out.mag_temp_c[k] = temp;
        if st.count == 0 {
            flags.set(QualityFlags::SENSOR_FAULT);
            continue;
        }
        let sensor = [SensorId::Mag1, SensorId::Mag2, SensorId::Mag3][k];
        let Some(model) = cal.lookup(sensor, temp) else {
            continue;
        };
        if model.extrapolated && !cal.is_empty() {
            flags.set(QualityFlags::TEMP_EXTRAPOLATED);
        }
        let v = model.affine.apply(st.mean * rescale);
        out.mag_cal[k] = v;
        field_ut_scale[k] = model.field_ut;
        mag_ok[k] = v.norm_sq() > 0.0 && v.to_array().iter().all(|x| x.is_finite());
        // Noise: standard error of the mean relative to the raw norm.
        let raw_norm = st.mean.norm().max(1e-9);
        let se = st.std_dev.max_abs() / libm::sqrt(f64::from(st.count)) / raw_norm;
        mag_sigma_rel = mag_sigma_rel.max(se);
        if st.std_dev.max_abs() / raw_norm > f64::from(thr.mag_noise_rel) {
            flags.set(QualityFlags::MAG_NOISY);
        }
    }

    // Pairwise agreement and fusion. With a disagreeing triad, keep the closest pair.
    let mut fused_m = Vec3::ZERO;
    let mut fused_scale = 0.0;
    let mut n_m = 0.0;
    let mut spread = 0.0f64;
    let mut best_pair: Option<(usize, usize, f64)> = None;
    for i in 0..MAG_COUNT {
        for j in i + 1..MAG_COUNT {
            if mag_ok[i] && mag_ok[j] {
                let a = rad_to_deg(out.mag_cal[i].angle_to(out.mag_cal[j]));
                spread = spread.max(a);
                if best_pair.is_none_or(|(_, _, b)| a < b) {
                    best_pair = Some((i, j, a));
                }
            }
        }
    }
    out.mag_spread_deg = spread;
    let use_mag: [bool; MAG_COUNT] = if spread > f64::from(thr.mag_disagree_deg) {
        flags.set(QualityFlags::MAG_DISAGREE);
        match best_pair {
            Some((i, j, _)) => {
                let mut u = [false; MAG_COUNT];
                u[i] = true;
                u[j] = true;
                u
            }
            None => mag_ok,
        }
    } else {
        mag_ok
    };
    for k in 0..MAG_COUNT {
        if use_mag[k] {
            // Each triad is unit-normalised by its own calibration; average the unit vectors
            // (direction) and the implied µT magnitudes separately.
            let v = out.mag_cal[k];
            fused_m = fused_m + v;
            fused_scale += v.norm() * field_ut_scale[k];
            n_m += 1.0;
        }
    }
    if n_m > 0.0 {
        fused_m = fused_m * (1.0 / n_m);
        out.b_total_ut = fused_scale / n_m;
        out.field_ut = fused_m.normalized().unwrap_or(Vec3::X) * out.b_total_ut;
    } else {
        flags.set(QualityFlags::SENSOR_FAULT);
    }

    // --- Accelerometers. ---
    let mut acc_ok = [false; ACC_COUNT];
    let mut g_norm_sum = 0.0;
    let mut acc_sigma_rel = 0.0f64;
    for k in 0..ACC_COUNT {
        let st = &input.acc[k];
        let temp = input.acc_temp_c[k].unwrap_or(25.0);
        out.acc_temp_c[k] = temp;
        if st.count == 0 {
            flags.set(QualityFlags::SENSOR_FAULT);
            continue;
        }
        let sensor = [SensorId::Acc1, SensorId::Acc2][k];
        let Some(model) = cal.lookup(sensor, temp) else {
            continue;
        };
        if model.extrapolated && !cal.is_empty() {
            flags.set(QualityFlags::TEMP_EXTRAPOLATED);
        }
        // The accelerometer measures the reaction force (up); gravity points the other way.
        let a = model.affine.apply(st.mean);
        let g = -a;
        out.acc_cal[k] = g;
        acc_ok[k] = g.norm_sq() > 0.0 && g.to_array().iter().all(|x| x.is_finite());
        g_norm_sum += g.norm();
        let raw_norm = st.mean.norm().max(1e-9);
        acc_sigma_rel = acc_sigma_rel.max(st.std_dev.max_abs() / libm::sqrt(f64::from(st.count)) / raw_norm);
        if st.std_dev.max_abs() / raw_norm > f64::from(thr.acc_noise_rel) {
            flags.set(QualityFlags::ACC_NOISY);
        }
    }
    let n_a = acc_ok.iter().filter(|&&o| o).count();
    if n_a == 0 {
        flags.set(QualityFlags::SENSOR_FAULT);
    } else {
        out.g_norm = g_norm_sum / n_a as f64;
    }
    // Accelerometer 1 defines the body frame; when the pair disagrees (fault, or no calibration
    // yet so the 90°-mounted second unit is still in its own frame) it alone sets the gravity
    // direction, and the disagreement is flagged rather than averaged into the result.
    let mut use_acc = acc_ok;
    if acc_ok[0] && acc_ok[1] {
        out.acc_spread_deg = rad_to_deg(out.acc_cal[0].angle_to(out.acc_cal[1]));
        if out.acc_spread_deg > f64::from(thr.acc_disagree_deg) {
            flags.set(QualityFlags::ACC_DISAGREE);
            use_acc[1] = false;
        }
    }
    let mut fused_g = Vec3::ZERO;
    for k in 0..ACC_COUNT {
        if use_acc[k] {
            fused_g = fused_g + out.acc_cal[k].normalized().unwrap_or(Vec3::Z);
        }
    }
    out.gravity = fused_g.normalized().unwrap_or(Vec3::Z);

    // --- Norm and dip checks against the reference (external, else the calibration site). ---
    let site = cal.sets().first().map(|s| FieldReference {
        b_total_ut: s.field_ut,
        dip_deg: s.dip_deg,
    });
    let reference = reference.or(site);
    if !cal.is_empty() {
        if libm::fabs(out.g_norm - 1.0) > f64::from(thr.acc_norm_tol) {
            flags.set(QualityFlags::ACC_NORM);
        }
        if let Some(r) = reference
            && r.b_total_ut > 0.0
            && libm::fabs(out.b_total_ut / r.b_total_ut - 1.0) > f64::from(thr.mag_norm_tol)
        {
            flags.set(QualityFlags::MAG_NORM);
        }
    }

    // --- Motion. ---
    match input.gyro_peak_dps {
        Some(rate) if rate > f64::from(thr.motion_dps) => flags.set(QualityFlags::MOTION),
        Some(_) => {}
        None => flags.set(QualityFlags::SENSOR_FAULT),
    }

    // --- Attitude. ---
    if let Some(att) = attitude(out.gravity, out.field_ut) {
        out.attitude = att;
        if let Some(r) = reference
            && !cal.is_empty()
            && libm::fabs(att.dip_deg - r.dip_deg) > f64::from(thr.dip_tol_deg)
        {
            flags.set(QualityFlags::DIP);
        }
        // Noise propagation: an angular error ε on the field vector maps to ε / cos(dip) on the
        // azimuth (horizontal component shrinks with dip); gravity noise maps 1:1 to inclination.
        let cos_dip = libm::cos(crate::math::deg_to_rad(att.dip_deg)).max(0.05);
        out.azimuth_sigma_deg =
            rad_to_deg(mag_sigma_rel / cos_dip + acc_sigma_rel * libm::tan(crate::math::deg_to_rad(att.dip_deg)).abs());
        out.inclination_sigma_deg = rad_to_deg(acc_sigma_rel);
    } else {
        flags.set(QualityFlags::SENSOR_FAULT);
    }

    out.flags = flags;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::{Affine, SensorSet};
    use crate::math::Mat3;
    use crate::survey::synthesize;

    fn field() -> Vec3 {
        let dip = crate::math::deg_to_rad(60.0);
        Vec3::new(48.0 * dip.cos(), 0.0, 48.0 * dip.sin())
    }

    /// A calibration where every sensor is already in the body frame with unit gain.
    fn ideal_cal() -> Calibration {
        let mut cal = Calibration::empty(400);
        let mut set = SensorSet::nominal(400);
        set.mag = [Affine::IDENTITY; MAG_COUNT];
        set.acc = [Affine::IDENTITY; ACC_COUNT];
        set.field_ut = 48.0;
        set.dip_deg = 60.0;
        cal.push_set(set);
        cal
    }

    fn stats(v: Vec3, sd: f64) -> SampleStats {
        SampleStats {
            count: 100,
            mean: v,
            std_dev: Vec3::new(sd, sd, sd),
        }
    }

    fn input(az: f64, inc: f64, roll: f64) -> ShotInput {
        let (g, m) = synthesize(az, inc, roll, field());
        let m_unit = m * (1.0 / 48.0);
        ShotInput {
            mag: [stats(m_unit, 1e-4); MAG_COUNT],
            acc: [stats(-g, 1e-4); ACC_COUNT],
            gyro_peak_dps: Some(0.1),
            mag_temp_c: [Some(20.0); MAG_COUNT],
            acc_temp_c: [Some(25.0); ACC_COUNT],
            mag_cycle_count: 400,
            quiet_asserted: true,
            short: false,
        }
    }

    #[test]
    fn clean_shot_is_clean() {
        let ev = evaluate(&input(123.0, -12.0, 30.0), &ideal_cal(), &Thresholds::default(), None);
        assert!(ev.flags.is_clean(), "{:?}", ev.flags);
        assert!((ev.attitude.azimuth_deg - 123.0).abs() < 1e-6);
        assert!((ev.attitude.inclination_deg + 12.0).abs() < 1e-6);
        assert!((ev.attitude.roll_deg - 30.0).abs() < 1e-6);
        assert!((ev.b_total_ut - 48.0).abs() < 1e-6);
        assert!((ev.g_norm - 1.0).abs() < 1e-9);
        assert!(ev.azimuth_sigma_deg > 0.0 && ev.azimuth_sigma_deg < 0.01);
    }

    #[test]
    fn disagreeing_triad_is_flagged_and_excluded() {
        let mut inp = input(90.0, 0.0, 0.0);
        // Rotate triad 3's field by 5° about Z: nearby iron.
        inp.mag[2].mean = Mat3::rot_z(5f64.to_radians()) * inp.mag[2].mean;
        let ev = evaluate(&inp, &ideal_cal(), &Thresholds::default(), None);
        assert!(ev.flags.has(QualityFlags::MAG_DISAGREE));
        assert!(ev.flags.is_severe());
        assert!(ev.mag_spread_deg > 2.0, "{}", ev.mag_spread_deg);
        // The two agreeing triads were kept: azimuth is still right.
        assert!(
            (ev.attitude.azimuth_deg - 90.0).abs() < 1e-6,
            "{}",
            ev.attitude.azimuth_deg
        );
    }

    #[test]
    fn motion_norm_dip_and_fault_flags() {
        let thr = Thresholds::default();
        let cal = ideal_cal();

        let mut inp = input(0.0, 0.0, 0.0);
        inp.gyro_peak_dps = Some(3.0);
        assert!(evaluate(&inp, &cal, &thr, None).flags.has(QualityFlags::MOTION));

        let mut inp = input(0.0, 0.0, 0.0);
        for m in &mut inp.mag {
            m.mean = m.mean * 1.1;
        }
        assert!(evaluate(&inp, &cal, &thr, None).flags.has(QualityFlags::MAG_NORM));

        let mut inp = input(0.0, 0.0, 0.0);
        for a in &mut inp.acc {
            a.mean = a.mean * 1.05;
        }
        assert!(evaluate(&inp, &cal, &thr, None).flags.has(QualityFlags::ACC_NORM));

        let inp = input(0.0, 0.0, 0.0);
        let reference = FieldReference {
            b_total_ut: 48.0,
            dip_deg: 65.0,
        };
        assert!(evaluate(&inp, &cal, &thr, Some(reference)).flags.has(QualityFlags::DIP));

        let mut inp = input(0.0, 0.0, 0.0);
        inp.mag[0].count = 0;
        inp.acc[1].count = 0;
        let ev = evaluate(&inp, &cal, &thr, None);
        assert!(ev.flags.has(QualityFlags::SENSOR_FAULT));
        assert!((ev.attitude.azimuth_deg.rem_euclid(360.0)).abs() < 1e-6);

        let inp = input(0.0, 0.0, 0.0);
        let ev = evaluate(&inp, &Calibration::empty(400), &thr, None);
        assert!(ev.flags.has(QualityFlags::NO_CALIBRATION));
        assert!(ev.flags.is_severe());
    }

    #[test]
    fn noisy_window_is_flagged() {
        let mut inp = input(0.0, 0.0, 0.0);
        for m in &mut inp.mag {
            m.std_dev = Vec3::new(0.01, 0.01, 0.01);
        }
        let ev = evaluate(&inp, &ideal_cal(), &Thresholds::default(), None);
        assert!(ev.flags.has(QualityFlags::MAG_NOISY));
        assert!(!ev.flags.is_severe());
    }
}
