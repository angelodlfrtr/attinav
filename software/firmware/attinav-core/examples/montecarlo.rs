//! Monte-Carlo evaluation of the calibration solver.
//!
//! For each trial a random but realistic instrument is generated (sensor misalignments, scale
//! errors, non-orthogonality, hard and soft iron, a laser-to-accelerometer tilt), a 14 × 4 shot
//! calibration is simulated with averaged-shot noise, the solver runs, and the resulting
//! calibration is scored on 300 fresh random poses: azimuth error (scaled by cos inclination) and
//! inclination error of the fused result, as the firmware would compute it.
//!
//! ```sh
//! cargo run --release --example montecarlo --target x86_64-unknown-linux-gnu --features std -- [trials] [options]
//!   --tilt DEG        laser-to-accelerometer-1 tilt (default 1.5)
//!   --mag-noise N     averaged magnetometer noise, raw counts (default 3)
//!   --acc-noise N     averaged accelerometer noise, raw counts (default 0.5)
//!   --outlier UT      disturb one calibration shot's triads by this many µT (default 0)
//!   --groups N        use only the first N groups (default 14)
//!   --seed N
//! ```

use std::time::Instant;

use attinav_core::calibration::{ACC_COUNT, Affine, Calibration, MAG_COUNT, SensorSet};
use attinav_core::math::{Mat3, Vec3, deg_to_rad};
use attinav_core::quality::{self, ShotInput, Thresholds};
use attinav_core::solver::{CalShot, GROUPS, ROLLS, ShotBuffer, SolveOptions, solve};
use attinav_core::stats::SampleStats;
use attinav_core::survey::synthesize;

struct Rng(u64);
impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
    fn gauss(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-12);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
    fn noise3(&mut self, sigma: f64) -> Vec3 {
        Vec3::new(self.gauss(), self.gauss(), self.gauss()) * sigma
    }
}

struct Config {
    trials: usize,
    tilt_deg: f64,
    mag_noise: f64,
    acc_noise: f64,
    outlier_ut: f64,
    groups: usize,
    seed: u64,
}

struct Truth {
    acc: [Affine; ACC_COUNT],
    mag: [Affine; MAG_COUNT],
    field: Vec3,
}

/// Random rotation of at most `max_deg` about a random axis.
fn small_rotation(rng: &mut Rng, max_deg: f64) -> Mat3 {
    let a = deg_to_rad(rng.uniform(-max_deg, max_deg));
    let b = deg_to_rad(rng.uniform(-max_deg, max_deg));
    let c = deg_to_rad(rng.uniform(-max_deg, max_deg));
    Mat3::rot_z(a) * Mat3::rot_y(b) * Mat3::rot_x(c)
}

/// Random non-orthogonal gain error: scale ±2 %, cross-axis ±0.5 %.
fn gain_error(rng: &mut Rng) -> Mat3 {
    let mut m = [0.0; 9];
    for (i, v) in m.iter_mut().enumerate() {
        *v = if i % 4 == 0 {
            1.0 + rng.uniform(-0.02, 0.02)
        } else {
            rng.uniform(-0.005, 0.005)
        };
    }
    Mat3::from_flat(m)
}

/// Symmetric soft-iron distortion up to ±3 % (diagonal) and ±1 % (off-diagonal).
fn soft_iron(rng: &mut Rng) -> Mat3 {
    let d = [
        1.0 + rng.uniform(-0.03, 0.03),
        1.0 + rng.uniform(-0.03, 0.03),
        1.0 + rng.uniform(-0.03, 0.03),
    ];
    let o = [
        rng.uniform(-0.01, 0.01),
        rng.uniform(-0.01, 0.01),
        rng.uniform(-0.01, 0.01),
    ];
    Mat3::from_rows([d[0], o[0], o[1]], [o[0], d[1], o[2]], [o[1], o[2], d[2]])
}

/// The "calibration-like" truth model of every sensor: `laser_frame_vector = M · (raw − b)`.
///
/// Raw counts are produced with the inverse. The laser frame is the reference: body X is the
/// laser axis by definition, so the truth accelerometer-1 matrix carries the mechanical tilt
/// between the accelerometer package and the laser.
fn truth(rng: &mut Rng, cfg: &Config) -> Truth {
    let acc_counts = 5400.0;
    let tilt = small_rotation(rng, cfg.tilt_deg);
    let acc0 = Affine {
        offset: rng.noise3(30.0),
        matrix: tilt * gain_error(rng) * (1.0 / acc_counts),
    };
    let acc1 = Affine {
        offset: rng.noise3(30.0),
        matrix: Mat3::rot_z(deg_to_rad(90.0)) * small_rotation(rng, 2.0) * gain_error(rng) * (1.0 / acc_counts),
    };
    let mag_counts = 148.0 * 48.0;
    let mag = std::array::from_fn(|_| Affine {
        offset: rng.noise3(1500.0),
        matrix: small_rotation(rng, 2.0) * soft_iron(rng) * (1.0 / mag_counts),
    });
    let dip = deg_to_rad(63.0);
    Truth {
        acc: [acc0, acc1],
        mag,
        field: Vec3::new(48.0 * dip.cos(), 0.0, 48.0 * dip.sin()),
    }
}

fn raw_of(model: &Affine, v: Vec3) -> Vec3 {
    model.matrix.inverse().expect("invertible truth") * v + model.offset
}

/// The calibration shots and the index of the disturbed one, if any.
fn shots(truth: &Truth, cfg: &Config, rng: &mut Rng) -> (ShotBuffer, Option<usize>) {
    let mut buf = ShotBuffer::new();
    let outlier_at = if cfg.outlier_ut > 0.0 {
        Some(rng.uniform(0.0, (cfg.groups * ROLLS) as f64) as usize)
    } else {
        None
    };
    let mut n = 0;
    for gi in 0..cfg.groups.min(GROUPS) {
        let az = gi as f64 * 360.0 / GROUPS as f64 + rng.uniform(-10.0, 10.0);
        let inc = [0.0, 25.0, -25.0, 55.0, -55.0, 80.0, -80.0][gi % 7] + rng.uniform(-5.0, 5.0);
        for ri in 0..ROLLS {
            let roll = ri as f64 * 90.0 + rng.uniform(-15.0, 15.0);
            let (g, m) = synthesize(az, inc, roll, truth.field);
            let mut m_ut = m;
            if outlier_at == Some(n) {
                // A nearby ferrous object: the same disturbance on all triads (uniform field error).
                m_ut = m_ut + Vec3::new(cfg.outlier_ut, 0.0, 0.0);
            }
            let m_unit = m_ut * (1.0 / truth.field.norm());
            let mut shot = CalShot {
                group: gi as u8,
                roll: ri as u8,
                ..CalShot::default()
            };
            for k in 0..MAG_COUNT {
                shot.mag[k] = raw_of(&truth.mag[k], m_unit) + rng.noise3(cfg.mag_noise);
                shot.mag_temp_c[k] = 12.0;
            }
            for k in 0..ACC_COUNT {
                shot.acc[k] = raw_of(&truth.acc[k], -g) + rng.noise3(cfg.acc_noise);
                shot.acc_temp_c[k] = 14.0;
            }
            buf.push(shot);
            n += 1;
        }
    }
    (buf, outlier_at)
}

/// Scores a solved set the way the firmware uses it: every sensor calibrated, triads and
/// accelerometers fused by `quality::evaluate`, attitude compared with the true pose.
fn score(truth: &Truth, set: &SensorSet, rng: &mut Rng, cfg: &Config) -> (f64, f64, f64, f64) {
    const N: usize = 300;
    let mut cal = Calibration::empty(400);
    cal.push_set(*set);
    let thr = Thresholds::default();
    let mut az_sq = 0.0;
    let mut inc_sq = 0.0;
    let mut az_max = 0.0f64;
    let mut inc_max = 0.0f64;
    for _ in 0..N {
        let az = rng.uniform(0.0, 360.0);
        let inc = rng.uniform(-85.0, 85.0);
        let roll = rng.uniform(-180.0, 180.0);
        let (g, m) = synthesize(az, inc, roll, truth.field);
        let m_unit = m * (1.0 / truth.field.norm());
        let mut input = ShotInput {
            mag_cycle_count: 400,
            quiet_asserted: true,
            gyro_peak_dps: Some(0.0),
            ..ShotInput::default()
        };
        for k in 0..MAG_COUNT {
            input.mag[k] = SampleStats {
                count: 100,
                mean: raw_of(&truth.mag[k], m_unit) + rng.noise3(cfg.mag_noise),
                std_dev: Vec3::new(1.0, 1.0, 1.0) * (cfg.mag_noise * 10.0),
            };
            input.mag_temp_c[k] = Some(12.0);
        }
        for k in 0..ACC_COUNT {
            input.acc[k] = SampleStats {
                count: 280,
                mean: raw_of(&truth.acc[k], -g) + rng.noise3(cfg.acc_noise),
                std_dev: Vec3::new(1.0, 1.0, 1.0) * (cfg.acc_noise * 16.0),
            };
            input.acc_temp_c[k] = Some(14.0);
        }
        let ev = quality::evaluate(&input, &cal, &thr, None);
        let d_az = ((ev.attitude.azimuth_deg - az + 540.0).rem_euclid(360.0) - 180.0) * deg_to_rad(inc).cos();
        let d_inc = ev.attitude.inclination_deg - inc;
        az_sq += d_az * d_az;
        inc_sq += d_inc * d_inc;
        az_max = az_max.max(d_az.abs());
        inc_max = inc_max.max(d_inc.abs());
    }
    ((az_sq / N as f64).sqrt(), az_max, (inc_sq / N as f64).sqrt(), inc_max)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn parse(args: &[String]) -> Config {
    let mut cfg = Config {
        trials: 50,
        tilt_deg: 1.5,
        mag_noise: 3.0,
        acc_noise: 0.5,
        outlier_ut: 0.0,
        groups: GROUPS,
        seed: 0x5EED,
    };
    let mut i = 0;
    while i < args.len() {
        let next = |i: &mut usize| -> f64 {
            *i += 1;
            args[*i].parse().expect("number")
        };
        match args[i].as_str() {
            "--tilt" => cfg.tilt_deg = next(&mut i),
            "--mag-noise" => cfg.mag_noise = next(&mut i),
            "--acc-noise" => cfg.acc_noise = next(&mut i),
            "--outlier" => cfg.outlier_ut = next(&mut i),
            "--groups" => cfg.groups = next(&mut i) as usize,
            "--seed" => cfg.seed = next(&mut i) as u64,
            other => cfg.trials = other.parse().expect("trial count"),
        }
        i += 1;
    }
    cfg
}

fn main() {
    let cfg = parse(&std::env::args().skip(1).collect::<Vec<_>>());
    let mut rng = Rng(cfg.seed);
    println!(
        "trials {} | tilt {:.2}° | noise mag {:.2} acc {:.2} counts | outlier {:.1} µT | groups {}",
        cfg.trials, cfg.tilt_deg, cfg.mag_noise, cfg.acc_noise, cfg.outlier_ut, cfg.groups
    );
    let opts = SolveOptions {
        mag_cycle_count: 400,
        ..SolveOptions::default()
    };
    let mut az_rms = Vec::new();
    let mut az_max = Vec::new();
    let mut inc_rms = Vec::new();
    let mut inc_max = Vec::new();
    let mut residuals = Vec::new();
    let mut iterations = Vec::new();
    let mut excluded = Vec::new();
    let mut missed_outliers = 0usize;
    let mut failures = std::collections::BTreeMap::new();
    let mut total_time = 0.0;
    for _ in 0..cfg.trials {
        let t = truth(&mut rng, &cfg);
        let (buf, outlier_at) = shots(&t, &cfg, &mut rng);
        let started = Instant::now();
        let result = solve(buf.as_slice(), &opts);
        total_time += started.elapsed().as_secs_f64();
        match result {
            Ok((set, report)) => {
                let (ar, am, ir, im) = score(&t, &set, &mut rng, &cfg);
                az_rms.push(ar);
                az_max.push(am);
                inc_rms.push(ir);
                inc_max.push(im);
                residuals.push(report.residual_deg);
                iterations.push(f64::from(report.iterations));
                excluded.push(f64::from(report.excluded_mask.count_ones()));
                if let Some(i) = outlier_at
                    && report.excluded_mask & (1u64 << i) == 0
                {
                    missed_outliers += 1;
                }
            }
            Err(e) => {
                *failures.entry(format!("{e:?}")).or_insert(0usize) += 1;
            }
        }
    }
    for v in [
        &mut az_rms,
        &mut az_max,
        &mut inc_rms,
        &mut inc_max,
        &mut residuals,
        &mut iterations,
    ] {
        v.sort_by(f64::total_cmp);
    }
    let ok = az_rms.len();
    println!(
        "solved {ok}/{} in {:.1} ms per solve",
        cfg.trials,
        1000.0 * total_time / cfg.trials as f64
    );
    for (name, count) in &failures {
        println!("  failed {name}: {count}");
    }
    if ok > 0 {
        let row = |name: &str, v: &[f64]| {
            println!(
                "  {name:<18} median {:>8.4}°  p90 {:>8.4}°  max {:>8.4}°",
                percentile(v, 0.5),
                percentile(v, 0.9),
                percentile(v, 1.0)
            );
        };
        row("azimuth RMS", &az_rms);
        row("azimuth max", &az_max);
        row("inclination RMS", &inc_rms);
        row("inclination max", &inc_max);
        row("fit residual", &residuals);
        println!(
            "  iterations         median {:>8.0}   max {:>8.0}",
            percentile(&iterations, 0.5),
            percentile(&iterations, 1.0)
        );
        println!(
            "  excluded shots     median {:>8.0}   max {:>8.0}   injected outlier missed in {missed_outliers} trial(s)",
            percentile(&excluded, 0.5),
            percentile(&excluded, 1.0)
        );
    }
}
