//! On-device calibration solver.
//!
//! # Procedure
//!
//! The instrument is pointed in [`GROUPS`] (14) different directions; in each direction
//! [`ROLLS`] (4) shots are taken at roll angles ~90° apart (the Heeb / `DistoX` protocol, 56 shots).
//! No reference direction is needed: the solver only exploits
//!
//! 1. `|g| = 1` for every shot (resting accelerometer),
//! 2. `|m| = 1` for every shot (constant local field),
//! 3. `g · m` constant (constant dip),
//! 4. within a group the laser direction computed from `(g, m)` must not change with roll.
//!
//! # Parameterisation
//!
//! The pair (accelerometer 1, triad 1) is solved first, jointly: 9 accelerometer parameters
//! (offset + **upper-triangular** gain matrix, which pins the body frame to the accelerometer axes
//! and removes the rotational gauge freedom), 12 magnetometer parameters (offset + full matrix,
//! including the rotation into the body frame) and the dip, 22 in total.
//! Then every other sensor is solved alone against that pair, with its laser direction constrained
//! shot-by-shot to the pair's — the other triads and the second accelerometer are thereby brought
//! into the same body frame.
//!
//! # Numerics
//!
//! Levenberg–Marquardt in `f64` with central finite-difference Jacobians, accumulating `JᵀJ` and
//! `Jᵀr` group by group so no Jacobian is ever stored. Raw counts are normalised by their mean norm
//! before the fit so every parameter is O(1).
//!
//! # Cooperative execution
//!
//! The Cortex-M33 has no double-precision FPU; a full solve takes seconds of soft-float. The solver
//! is therefore a [`SolveJob`] that performs **one iteration per [`SolveJob::step`]**, so the
//! firmware can yield to the USB and UART tasks between steps. [`solve`] runs a job to completion
//! for host-side use and tests.

use crate::calibration::{ACC_COUNT, Affine, MAG_COUNT, SensorSet, rm3100_nominal_gain};
use crate::math::{Mat3, Vec3, rad_to_deg};
use crate::survey::laser_direction_world;

/// Number of pointing directions in the procedure.
pub const GROUPS: usize = 14;
/// Number of roll angles per direction.
pub const ROLLS: usize = 4;
/// Nominal number of shots.
pub const SHOTS: usize = GROUPS * ROLLS;
/// Capacity of a [`ShotBuffer`] (extra room for repeated shots).
pub const SHOT_CAPACITY: usize = SHOTS + 8;
/// Maximum shots per group accepted by the solver.
pub const MAX_PER_GROUP: usize = 8;
/// Minimum number of usable shots before a solve is attempted.
pub const MIN_SHOTS: usize = 24;
/// Minimum number of groups holding at least two shots.
pub const MIN_MULTI_GROUPS: usize = 6;

/// Raw averaged readings of one calibration shot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CalShot {
    /// Direction group `0..GROUPS` (any `u8` is accepted; shots are grouped by equal value).
    pub group: u8,
    /// Roll slot within the group (informational).
    pub roll: u8,
    /// Mean raw magnetometer counts per triad.
    pub mag: [Vec3; MAG_COUNT],
    /// Mean raw accelerometer counts per sensor.
    pub acc: [Vec3; ACC_COUNT],
    /// Coil temperatures, °C.
    pub mag_temp_c: [f32; MAG_COUNT],
    /// Die temperatures, °C.
    pub acc_temp_c: [f32; ACC_COUNT],
}

/// Fixed-capacity buffer of calibration shots (the firmware keeps one in RAM during a session).
#[derive(Clone, Copy, Debug)]
pub struct ShotBuffer {
    shots: [CalShot; SHOT_CAPACITY],
    len: usize,
}

impl Default for ShotBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl ShotBuffer {
    /// Empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shots: [CalShot {
                group: 0,
                roll: 0,
                mag: [Vec3::ZERO; MAG_COUNT],
                acc: [Vec3::ZERO; ACC_COUNT],
                mag_temp_c: [0.0; MAG_COUNT],
                acc_temp_c: [0.0; ACC_COUNT],
            }; SHOT_CAPACITY],
            len: 0,
        }
    }

    /// Stored shots.
    #[must_use]
    pub fn as_slice(&self) -> &[CalShot] {
        &self.shots[..self.len]
    }

    /// Number of stored shots.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// `true` when nothing is stored.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes every shot.
    pub const fn clear(&mut self) {
        self.len = 0;
    }

    /// Adds a shot. A shot with the same `(group, roll)` replaces the previous one, so a bad shot
    /// can simply be retaken. Returns the slot index, or `None` when full.
    pub fn push(&mut self, shot: CalShot) -> Option<usize> {
        if let Some(i) = self.shots[..self.len]
            .iter()
            .position(|s| s.group == shot.group && s.roll == shot.roll)
        {
            self.shots[i] = shot;
            return Some(i);
        }
        if self.len >= SHOT_CAPACITY {
            return None;
        }
        self.shots[self.len] = shot;
        self.len += 1;
        Some(self.len - 1)
    }

    /// Removes the shot at `(group, roll)`; `true` when something was removed.
    pub fn remove(&mut self, group: u8, roll: u8) -> bool {
        if let Some(i) = self.shots[..self.len]
            .iter()
            .position(|s| s.group == group && s.roll == roll)
        {
            self.shots.copy_within(i + 1..self.len, i);
            self.len -= 1;
            true
        } else {
            false
        }
    }
}

/// Why a solve was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SolveError {
    /// Fewer than [`MIN_SHOTS`] shots, or fewer than [`MIN_MULTI_GROUPS`] groups with ≥ 2 shots.
    NotEnoughShots,
    /// A group holds more than [`MAX_PER_GROUP`] shots.
    GroupTooLarge,
    /// A raw vector was zero or not finite.
    BadInput,
    /// The normal equations became singular (degenerate geometry, e.g. all shots in one plane).
    Singular,
    /// The iteration did not reduce the cost to the requested tolerance.
    NotConverged,
    /// The fit converged but its residual exceeds the acceptance threshold.
    ResidualTooLarge,
}

#[cfg(feature = "std")]
impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NotEnoughShots => "not enough shots",
            Self::GroupTooLarge => "too many shots in one group",
            Self::BadInput => "zero or non-finite raw vector",
            Self::Singular => "degenerate geometry (singular normal equations)",
            Self::NotConverged => "solver did not converge",
            Self::ResidualTooLarge => "residual above acceptance threshold",
        };
        f.write_str(s)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SolveError {}

/// Solver tuning.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SolveOptions {
    /// Maximum Levenberg–Marquardt iterations per sub-problem.
    pub max_iterations: u32,
    /// Stop when the relative cost decrease of an accepted step falls below this.
    pub relative_tolerance: f64,
    /// Reject the result when the RMS angular residual (degrees) of the primary pair exceeds this.
    pub max_residual_deg: f64,
    /// RM3100 cycle count the shots were taken at (for the µT estimate).
    pub mag_cycle_count: u16,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            max_iterations: 80,
            relative_tolerance: 1e-12,
            max_residual_deg: 0.5,
            mag_cycle_count: 400,
        }
    }
}

/// Diagnostics of a completed solve.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SolveReport {
    /// Iterations used by the primary pair solve.
    pub iterations: u32,
    /// Final sum of squared residuals of the primary pair.
    pub cost: f64,
    /// RMS angular scatter of the laser direction within groups, degrees (primary pair).
    pub residual_deg: f64,
    /// Same, per magnetometer against the body frame (`[0]` is the primary).
    pub mag_residual_deg: [f64; MAG_COUNT],
    /// Same, per accelerometer (`[0]` is the primary).
    pub acc_residual_deg: [f64; ACC_COUNT],
    /// Shots used.
    pub shots: u16,
}

/// Which sub-problem a [`SolveJob`] is working on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Phase {
    /// Joint accelerometer-1 / triad-1 solve.
    Pair,
    /// Triad `k` (1-based index in the wire sense: 2 or 3) against the pair.
    Mag(u8),
    /// Accelerometer 2 against the pair.
    Acc(u8),
    /// Finished.
    Done,
}

/// Outcome of one [`SolveJob::step`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the finished variant carries the result once; it is never stored"
)]
pub enum Progress {
    /// More steps needed; `iteration` counts within the current phase.
    Running {
        /// Current sub-problem.
        phase: Phase,
        /// Iterations completed in that sub-problem.
        iteration: u32,
    },
    /// Finished.
    Finished(Result<(SensorSet, SolveReport), SolveError>),
}

/// Runs the complete solve over `shots` to completion.
pub fn solve(shots: &[CalShot], opts: &SolveOptions) -> Result<(SensorSet, SolveReport), SolveError> {
    let mut job = SolveJob::new(shots, *opts)?;
    loop {
        if let Progress::Finished(r) = job.step() {
            return r;
        }
    }
}

/// A calibration solve in progress. See the module documentation.
pub struct SolveJob<'a> {
    shots: &'a [CalShot],
    opts: SolveOptions,
    layout: GroupLayout,
    mag_scale: [f64; MAG_COUNT],
    acc_scale: [f64; ACC_COUNT],
    phase: Phase,
    lm_pair: Lm<PAIR_NP>,
    lm_single: Lm<SINGLE_NP>,
    // Primary-pair results, used as the reference by the single-sensor phases.
    ref_dir: [Vec3; SHOT_CAPACITY],
    ref_g: [Vec3; SHOT_CAPACITY],
    ref_m: [Vec3; SHOT_CAPACITY],
    gm_dot: f64,
    set: SensorSet,
    report: SolveReport,
}

impl<'a> SolveJob<'a> {
    /// Validates the shots and prepares the first phase.
    pub fn new(shots: &'a [CalShot], opts: SolveOptions) -> Result<Self, SolveError> {
        let layout = GroupLayout::build(shots)?;
        for s in shots {
            let finite = s
                .mag
                .iter()
                .chain(s.acc.iter())
                .all(|v| v.to_array().iter().all(|x| x.is_finite()) && v.norm_sq() > 0.0);
            if !finite {
                return Err(SolveError::BadInput);
            }
        }
        let n = shots.len() as f64;
        let mut mag_scale = [0.0; MAG_COUNT];
        let mut acc_scale = [0.0; ACC_COUNT];
        for s in shots {
            for k in 0..MAG_COUNT {
                mag_scale[k] += s.mag[k].norm() / n;
            }
            for k in 0..ACC_COUNT {
                acc_scale[k] += s.acc[k].norm() / n;
            }
        }
        let mut job = Self {
            shots,
            opts,
            layout,
            mag_scale,
            acc_scale,
            phase: Phase::Pair,
            lm_pair: Lm::new([0.0; PAIR_NP]),
            lm_single: Lm::new([0.0; SINGLE_NP]),
            ref_dir: [Vec3::ZERO; SHOT_CAPACITY],
            ref_g: [Vec3::ZERO; SHOT_CAPACITY],
            ref_m: [Vec3::ZERO; SHOT_CAPACITY],
            gm_dot: 0.0,
            set: SensorSet::nominal(opts.mag_cycle_count),
            report: SolveReport {
                shots: shots.len() as u16,
                ..SolveReport::default()
            },
        };
        let pair = pair_problem(job.shots, &job.layout, job.acc_scale[0], job.mag_scale[0]);
        let mut p = [0.0; PAIR_NP];
        pair.initial(&mut p);
        let mut lm = Lm::new(p);
        lm.start(&pair);
        job.lm_pair = lm;
        Ok(job)
    }

    /// Current phase.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// Performs one iteration. Call until it returns [`Progress::Finished`].
    pub fn step(&mut self) -> Progress {
        match self.phase {
            Phase::Pair => {
                // Borrow only the fields the problem needs so `lm_pair` can be borrowed mutably.
                let prob = pair_problem(self.shots, &self.layout, self.acc_scale[0], self.mag_scale[0]);
                match self.lm_pair.step(&prob, &self.opts) {
                    LmStep::Continue => Progress::Running {
                        phase: self.phase,
                        iteration: self.lm_pair.iterations,
                    },
                    LmStep::Failed(e) => self.finish(Err(e)),
                    LmStep::Converged => match self.finish_pair() {
                        Ok(()) => self.start_single(Source::Mag(1)),
                        Err(e) => self.finish(Err(e)),
                    },
                }
            }
            Phase::Mag(k) | Phase::Acc(k) => {
                let source = match self.phase {
                    Phase::Mag(_) => Source::Mag(usize::from(k)),
                    _ => Source::Acc(usize::from(k)),
                };
                let prob = single_problem(
                    self.shots,
                    source,
                    &self.mag_scale,
                    &self.acc_scale,
                    &self.ref_g,
                    &self.ref_m,
                    &self.ref_dir,
                    self.gm_dot,
                );
                match self.lm_single.step(&prob, &self.opts) {
                    LmStep::Continue => Progress::Running {
                        phase: self.phase,
                        iteration: self.lm_single.iterations,
                    },
                    LmStep::Failed(e) => self.finish(Err(e)),
                    LmStep::Converged => {
                        let model = unscale(unpack_full(&self.lm_single.p), prob.scale);
                        let scatter = single_scatter_deg(&prob, &model);
                        match source {
                            Source::Mag(k) => {
                                self.set.mag[k] = model;
                                self.report.mag_residual_deg[k] = scatter;
                            }
                            Source::Acc(k) => {
                                self.set.acc[k] = model;
                                self.report.acc_residual_deg[k] = scatter;
                            }
                        }
                        if let Some(next) = source.next() {
                            self.start_single(next)
                        } else {
                            self.finalize_metadata();
                            self.finish(Ok(()))
                        }
                    }
                }
            }
            Phase::Done => Progress::Finished(Err(SolveError::NotConverged)),
        }
    }

    fn finish(&mut self, r: Result<(), SolveError>) -> Progress {
        self.phase = Phase::Done;
        Progress::Finished(r.map(|()| (self.set, self.report)))
    }

    fn start_single(&mut self, source: Source) -> Progress {
        self.phase = match source {
            Source::Mag(k) => Phase::Mag(k as u8),
            Source::Acc(k) => Phase::Acc(k as u8),
        };
        let prob = single_problem(
            self.shots,
            source,
            &self.mag_scale,
            &self.acc_scale,
            &self.ref_g,
            &self.ref_m,
            &self.ref_dir,
            self.gm_dot,
        );
        let mut p = [0.0; SINGLE_NP];
        prob.initial(&mut p);
        let mut lm = Lm::new(p);
        lm.start(&prob);
        self.lm_single = lm;
        Progress::Running {
            phase: self.phase,
            iteration: 0,
        }
    }

    /// Converts the converged pair solution into reference vectors for the other sensors.
    fn finish_pair(&mut self) -> Result<(), SolveError> {
        let (acc0, mag0, _) = unpack_pair(&self.lm_pair.p);
        let acc0 = unscale(acc0, self.acc_scale[0]);
        let mag0 = unscale(mag0, self.mag_scale[0]);
        let n = self.shots.len();
        let mut dot_sum = 0.0;
        for (i, s) in self.shots.iter().enumerate() {
            let a = acc0.apply(s.acc[0]);
            let m = mag0.apply(s.mag[0]);
            let g = -a;
            self.ref_g[i] = g.normalized().ok_or(SolveError::Singular)?;
            self.ref_m[i] = m.normalized().ok_or(SolveError::Singular)?;
            self.ref_dir[i] = laser_direction_world(g, m).ok_or(SolveError::Singular)?;
            dot_sum += self.ref_g[i].dot(self.ref_m[i]);
        }
        self.gm_dot = dot_sum / n as f64;
        let residual_deg = group_scatter_deg(&self.ref_dir[..n], &self.layout);
        if !residual_deg.is_finite() || residual_deg > self.opts.max_residual_deg {
            return Err(SolveError::ResidualTooLarge);
        }
        // Field magnitude from the offset-corrected raw norm (the plain mean norm would be biased
        // upward by the hard-iron offset) and the nominal gain at the shot cycle count.
        let mut raw_norm = 0.0;
        for s in self.shots {
            raw_norm += (s.mag[0] - mag0.offset).norm() / n as f64;
        }
        self.set.field_ut = raw_norm / rm3100_nominal_gain(self.opts.mag_cycle_count);
        self.set.acc[0] = acc0;
        self.set.mag[0] = mag0;
        self.report.iterations = self.lm_pair.iterations;
        self.report.cost = self.lm_pair.current;
        self.report.residual_deg = residual_deg;
        self.report.mag_residual_deg[0] = residual_deg;
        self.report.acc_residual_deg[0] = residual_deg;
        Ok(())
    }

    /// Dip, residual summary and the set's temperature tags (`field_ut` is set by `finish_pair`).
    ///
    /// Temperatures are averaged over the shots whose reading was finite; a sensor whose
    /// thermometer never answered gets [`FALLBACK_TEMP_C`] so the set stays usable (and
    /// serialisable: a NaN tag would be rejected by the codec and would break interpolation).
    fn finalize_metadata(&mut self) {
        self.set.dip_deg = rad_to_deg(libm::asin(self.gm_dot.clamp(-1.0, 1.0)));
        self.set.residual_deg = self.report.residual_deg as f32;
        self.set.shots = self.shots.len() as u16;
        for k in 0..MAG_COUNT {
            self.set.mag_temp_c[k] = finite_mean(self.shots.iter().map(|s| s.mag_temp_c[k]));
        }
        for k in 0..ACC_COUNT {
            self.set.acc_temp_c[k] = finite_mean(self.shots.iter().map(|s| s.acc_temp_c[k]));
        }
    }
}

/// Temperature tag used when no shot of a set carried a finite reading for a sensor.
pub const FALLBACK_TEMP_C: f32 = 20.0;

/// Mean of the finite values, or [`FALLBACK_TEMP_C`] when there is none.
fn finite_mean(values: impl Iterator<Item = f32>) -> f32 {
    let (sum, n) = values
        .filter(|v| v.is_finite())
        .fold((0.0f32, 0u32), |(s, n), v| (s + v, n + 1));
    if n == 0 { FALLBACK_TEMP_C } else { sum / n as f32 }
}

/// Builds the primary-pair problem from the job's fields (free function so the borrow checker sees
/// which fields are borrowed).
const fn pair_problem<'a, 'j>(
    shots: &'a [CalShot],
    layout: &'j GroupLayout,
    acc_scale: f64,
    mag_scale: f64,
) -> PairProblem<'a, 'j> {
    PairProblem {
        shots,
        layout,
        acc_scale,
        mag_scale,
    }
}

/// Builds a secondary-sensor problem from the job's fields.
#[allow(clippy::too_many_arguments)]
fn single_problem<'a, 'j>(
    shots: &'a [CalShot],
    source: Source,
    mag_scale: &[f64; MAG_COUNT],
    acc_scale: &[f64; ACC_COUNT],
    ref_g: &'j [Vec3; SHOT_CAPACITY],
    ref_m: &'j [Vec3; SHOT_CAPACITY],
    ref_dir: &'j [Vec3; SHOT_CAPACITY],
    gm_dot: f64,
) -> SingleProblem<'a, 'j> {
    let n = shots.len();
    let (scale, partner) = match source {
        Source::Mag(k) => (mag_scale[k], &ref_g[..n]),
        Source::Acc(k) => (acc_scale[k], &ref_m[..n]),
    };
    SingleProblem {
        shots,
        source,
        scale,
        partner,
        ref_dir: &ref_dir[..n],
        gm_dot,
    }
}

/// Which secondary sensor a [`SingleProblem`] fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Mag(usize),
    Acc(usize),
}

impl Source {
    /// Order of the secondary phases: triads 2..n, then accelerometers 2..n.
    const fn next(self) -> Option<Self> {
        match self {
            Self::Mag(k) if k + 1 < MAG_COUNT => Some(Self::Mag(k + 1)),
            Self::Mag(_) => {
                if ACC_COUNT > 1 {
                    Some(Self::Acc(1))
                } else {
                    None
                }
            }
            Self::Acc(k) if k + 1 < ACC_COUNT => Some(Self::Acc(k + 1)),
            Self::Acc(_) => None,
        }
    }
}

/// Converts a model solved on `raw / scale` back to raw counts.
fn unscale(a: Affine, scale: f64) -> Affine {
    Affine {
        offset: a.offset * scale,
        matrix: a.matrix * (1.0 / scale),
    }
}

/// Grouping of the shot list by `group` value.
struct GroupLayout {
    /// For each group: indices into the shot slice.
    members: [[usize; MAX_PER_GROUP]; SHOT_CAPACITY],
    counts: [usize; SHOT_CAPACITY],
    n_groups: usize,
}

impl GroupLayout {
    fn build(shots: &[CalShot]) -> Result<Self, SolveError> {
        if shots.len() < MIN_SHOTS || shots.len() > SHOT_CAPACITY {
            return Err(SolveError::NotEnoughShots);
        }
        let mut layout = Self {
            members: [[0; MAX_PER_GROUP]; SHOT_CAPACITY],
            counts: [0; SHOT_CAPACITY],
            n_groups: 0,
        };
        let mut group_ids = [0u8; SHOT_CAPACITY];
        for (i, s) in shots.iter().enumerate() {
            let gi = group_ids[..layout.n_groups]
                .iter()
                .position(|&g| g == s.group)
                .unwrap_or_else(|| {
                    group_ids[layout.n_groups] = s.group;
                    layout.n_groups += 1;
                    layout.n_groups - 1
                });
            if layout.counts[gi] >= MAX_PER_GROUP {
                return Err(SolveError::GroupTooLarge);
            }
            layout.members[gi][layout.counts[gi]] = i;
            layout.counts[gi] += 1;
        }
        let multi = layout.counts[..layout.n_groups].iter().filter(|&&c| c >= 2).count();
        if multi < MIN_MULTI_GROUPS {
            return Err(SolveError::NotEnoughShots);
        }
        Ok(layout)
    }

    fn group(&self, gi: usize) -> &[usize] {
        &self.members[gi][..self.counts[gi]]
    }
}

/// RMS angle (degrees) between each shot's laser direction and its group mean.
fn group_scatter_deg(dirs: &[Vec3], layout: &GroupLayout) -> f64 {
    let mut sum_sq = 0.0;
    let mut count = 0usize;
    for gi in 0..layout.n_groups {
        let idx = layout.group(gi);
        if idx.len() < 2 {
            continue;
        }
        let mut mean = Vec3::ZERO;
        for &i in idx {
            mean = mean + dirs[i];
        }
        let Some(mean) = mean.normalized() else { continue };
        for &i in idx {
            let a = dirs[i].angle_to(mean);
            sum_sq += a * a;
            count += 1;
        }
    }
    if count == 0 {
        return f64::INFINITY;
    }
    rad_to_deg(libm::sqrt(sum_sq / count as f64))
}

/// RMS angle (degrees) between the laser direction obtained with a secondary sensor and the
/// primary pair's, shot by shot.
fn single_scatter_deg(prob: &SingleProblem<'_, '_>, model: &Affine) -> f64 {
    let mut sum_sq = 0.0;
    let mut n = 0usize;
    for (i, s) in prob.shots.iter().enumerate() {
        let v = model.apply(prob.raw(s));
        let dir = match prob.source {
            Source::Mag(_) => laser_direction_world(prob.partner[i], v),
            Source::Acc(_) => laser_direction_world(-v, prob.partner[i]),
        };
        if let Some(d) = dir {
            let a = d.angle_to(prob.ref_dir[i]);
            sum_sq += a * a;
            n += 1;
        }
    }
    if n == 0 {
        f64::INFINITY
    } else {
        rad_to_deg(libm::sqrt(sum_sq / n as f64))
    }
}

// ---------------------------------------------------------------------------------------------
// Least-squares problem abstraction
// ---------------------------------------------------------------------------------------------

/// Largest residual block any problem produces (one group of shots).
const MAX_BLOCK_RESIDUALS: usize = MAX_PER_GROUP * 6;

/// A least-squares problem whose residuals split into independent blocks.
trait Problem<const NP: usize> {
    /// Number of residual blocks.
    fn blocks(&self) -> usize;
    /// Writes the residuals of block `b` at parameters `p` into `out`; returns how many.
    fn residuals(&self, b: usize, p: &[f64; NP], out: &mut [f64; MAX_BLOCK_RESIDUALS]) -> usize;
}

/// Offset (3) + upper-triangular matrix (6) → [`Affine`] on normalised counts.
const fn unpack_upper(p: &[f64]) -> Affine {
    Affine {
        offset: Vec3::new(p[0], p[1], p[2]),
        matrix: Mat3::from_rows([p[3], p[4], p[5]], [0.0, p[6], p[7]], [0.0, 0.0, p[8]]),
    }
}

/// Offset (3) + full matrix (9) → [`Affine`] on normalised counts.
const fn unpack_full(p: &[f64]) -> Affine {
    Affine {
        offset: Vec3::new(p[0], p[1], p[2]),
        matrix: Mat3::from_flat([p[3], p[4], p[5], p[6], p[7], p[8], p[9], p[10], p[11]]),
    }
}

const PAIR_NP: usize = 9 + 12 + 1;

fn unpack_pair(p: &[f64; PAIR_NP]) -> (Affine, Affine, f64) {
    (unpack_upper(&p[0..9]), unpack_full(&p[9..21]), p[21])
}

/// Joint accelerometer-0 / triad-0 problem.
struct PairProblem<'a, 'j> {
    shots: &'a [CalShot],
    layout: &'j GroupLayout,
    acc_scale: f64,
    mag_scale: f64,
}

impl PairProblem<'_, '_> {
    /// Start point: accelerometer at the upper-triangular identity (it defines the body frame),
    /// the triad at the signed axis permutation with the lowest cost, so a triad mounted at any
    /// multiple of 90° to the accelerometer starts within a few degrees of its solution. The dip
    /// term is initialised from the raw data under that permutation.
    fn initial(&self, p: &mut [f64; PAIR_NP]) {
        let mut best = (f64::INFINITY, [0.0; PAIR_NP]);
        for perm in AXIS_MAPS {
            let mut q = [0.0; PAIR_NP];
            q[3] = 1.0;
            q[6] = 1.0;
            q[8] = 1.0;
            q[12..21].copy_from_slice(&perm);
            let rot = Mat3::from_flat(perm);
            let mut dot = 0.0;
            for s in self.shots {
                if let (Some(g), Some(m)) = ((-s.acc[0]).normalized(), (rot * s.mag[0]).normalized()) {
                    dot += g.dot(m);
                }
            }
            q[21] = dot / self.shots.len() as f64;
            let c = cost(self, &q);
            if c < best.0 {
                best = (c, q);
            }
        }
        *p = best.1;
    }
}

impl Problem<PAIR_NP> for PairProblem<'_, '_> {
    fn blocks(&self) -> usize {
        self.layout.n_groups
    }

    fn residuals(&self, b: usize, p: &[f64; PAIR_NP], out: &mut [f64; MAX_BLOCK_RESIDUALS]) -> usize {
        let (acc, mag, gm_dot) = unpack_pair(p);
        let idx = self.layout.group(b);
        let mut dirs = [Vec3::ZERO; MAX_PER_GROUP];
        let mut n = 0;
        for (j, &i) in idx.iter().enumerate() {
            let s = &self.shots[i];
            let a = acc.apply(s.acc[0] * (1.0 / self.acc_scale));
            let m = mag.apply(s.mag[0] * (1.0 / self.mag_scale));
            let an = a.norm();
            let mn = m.norm();
            out[n] = an - 1.0;
            out[n + 1] = mn - 1.0;
            let g_hat = -a * (1.0 / an);
            let m_hat = m * (1.0 / mn);
            out[n + 2] = g_hat.dot(m_hat) - gm_dot;
            n += 3;
            dirs[j] = laser_direction_world(g_hat, m_hat).unwrap_or(Vec3::X);
        }
        if idx.len() >= 2 {
            let mut mean = Vec3::ZERO;
            for d in &dirs[..idx.len()] {
                mean = mean + *d;
            }
            mean = mean * (1.0 / idx.len() as f64);
            for d in &dirs[..idx.len()] {
                let r = *d - mean;
                out[n] = r.x;
                out[n + 1] = r.y;
                out[n + 2] = r.z;
                n += 3;
            }
        }
        n
    }
}

const SINGLE_NP: usize = 12;

/// One sensor solved against the fixed primary pair.
struct SingleProblem<'a, 'j> {
    shots: &'a [CalShot],
    source: Source,
    scale: f64,
    /// Unit gravity directions (for a magnetometer) or unit field vectors (for an accelerometer),
    /// from the primary pair.
    partner: &'j [Vec3],
    ref_dir: &'j [Vec3],
    gm_dot: f64,
}

impl SingleProblem<'_, '_> {
    const fn raw(&self, s: &CalShot) -> Vec3 {
        match self.source {
            Source::Mag(k) => s.mag[k],
            Source::Acc(k) => s.acc[k],
        }
    }

    /// Start point: the signed axis permutation with the lowest cost, so a sensor mounted at any
    /// multiple of 90° (the second accelerometer is rotated 90° about Z) starts within a few degrees
    /// of its solution.
    fn initial(&self, p: &mut [f64; SINGLE_NP]) {
        let mut best = (f64::INFINITY, [0.0; SINGLE_NP]);
        for perm in AXIS_MAPS {
            let mut q = [0.0; SINGLE_NP];
            q[3..12].copy_from_slice(&perm);
            let c = cost(self, &q);
            if c < best.0 {
                best = (c, q);
            }
        }
        *p = best.1;
    }
}

/// The 24 signed axis permutations (proper and improper rotations by multiples of 90°), as
/// row-major 3×3 matrices.
const AXIS_MAPS: [[f64; 9]; 24] = {
    let mut maps = [[0.0; 9]; 24];
    let perms: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let signs: [[f64; 3]; 4] = [[1.0, 1.0, 1.0], [1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, 1.0]];
    let mut i = 0;
    while i < 6 {
        let mut j = 0;
        while j < 4 {
            let mut m = [0.0; 9];
            let mut r = 0;
            while r < 3 {
                m[r * 3 + perms[i][r]] = signs[j][r];
                r += 1;
            }
            maps[i * 4 + j] = m;
            j += 1;
        }
        i += 1;
    }
    maps
};

impl Problem<SINGLE_NP> for SingleProblem<'_, '_> {
    fn blocks(&self) -> usize {
        self.shots.len()
    }

    fn residuals(&self, b: usize, p: &[f64; SINGLE_NP], out: &mut [f64; MAX_BLOCK_RESIDUALS]) -> usize {
        let model = unpack_full(p);
        let v = model.apply(self.raw(&self.shots[b]) * (1.0 / self.scale));
        let vn = v.norm();
        out[0] = vn - 1.0;
        let v_hat = v * (1.0 / vn);
        let (g_hat, m_hat) = match self.source {
            Source::Mag(_) => (self.partner[b], v_hat),
            Source::Acc(_) => (-v_hat, self.partner[b]),
        };
        out[1] = g_hat.dot(m_hat) - self.gm_dot;
        let d = laser_direction_world(g_hat, m_hat).unwrap_or(Vec3::X) - self.ref_dir[b];
        out[2] = d.x;
        out[3] = d.y;
        out[4] = d.z;
        5
    }
}

// ---------------------------------------------------------------------------------------------
// Levenberg–Marquardt
// ---------------------------------------------------------------------------------------------

/// Sum of squared residuals.
fn cost<const NP: usize>(prob: &impl Problem<NP>, p: &[f64; NP]) -> f64 {
    let mut out = [0.0; MAX_BLOCK_RESIDUALS];
    let mut c = 0.0;
    for b in 0..prob.blocks() {
        let n = prob.residuals(b, p, &mut out);
        for r in &out[..n] {
            c += r * r;
        }
    }
    c
}

/// Accumulates `JᵀJ` and `Jᵀr` with central finite differences, block by block; returns the cost.
fn normal_equations<const NP: usize>(
    prob: &impl Problem<NP>,
    p: &[f64; NP],
    jtj: &mut [[f64; NP]; NP],
    jtr: &mut [f64; NP],
) -> f64 {
    const H: f64 = 1e-6;
    *jtj = [[0.0; NP]; NP];
    *jtr = [0.0; NP];
    let mut c = 0.0;
    let mut r0 = [0.0; MAX_BLOCK_RESIDUALS];
    let mut rp = [0.0; MAX_BLOCK_RESIDUALS];
    let mut rm = [0.0; MAX_BLOCK_RESIDUALS];
    // Jacobian of one block, residual-major.
    let mut jac = [[0.0; NP]; MAX_BLOCK_RESIDUALS];
    for b in 0..prob.blocks() {
        let n = prob.residuals(b, p, &mut r0);
        for r in &r0[..n] {
            c += r * r;
        }
        for j in 0..NP {
            let h = H * p[j].abs().max(1.0);
            let mut pp = *p;
            pp[j] += h;
            let mut pm = *p;
            pm[j] -= h;
            prob.residuals(b, &pp, &mut rp);
            prob.residuals(b, &pm, &mut rm);
            for i in 0..n {
                jac[i][j] = (rp[i] - rm[i]) / (2.0 * h);
            }
        }
        for (i, row) in jac[..n].iter().enumerate() {
            for j in 0..NP {
                jtr[j] += row[j] * r0[i];
                for k in j..NP {
                    jtj[j][k] += row[j] * row[k];
                }
            }
        }
    }
    for j in 0..NP {
        for k in 0..j {
            jtj[j][k] = jtj[k][j];
        }
    }
    c
}

/// Solves `A x = b` for symmetric positive definite `A` by Cholesky decomposition.
fn cholesky_solve<const NP: usize>(a: &[[f64; NP]; NP], b: &[f64; NP]) -> Option<[f64; NP]> {
    let mut l = [[0.0; NP]; NP];
    for i in 0..NP {
        for j in 0..=i {
            let mut sum = a[i][j];
            for k in 0..j {
                sum -= l[i][k] * l[j][k];
            }
            if i == j {
                if sum <= 0.0 || !sum.is_finite() {
                    return None;
                }
                l[i][i] = libm::sqrt(sum);
            } else {
                l[i][j] = sum / l[j][j];
            }
        }
    }
    let mut y = [0.0; NP];
    for i in 0..NP {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i][k] * y[k];
        }
        y[i] = sum / l[i][i];
    }
    let mut x = [0.0; NP];
    for i in (0..NP).rev() {
        let mut sum = y[i];
        for k in i + 1..NP {
            sum -= l[k][i] * x[k];
        }
        x[i] = sum / l[i][i];
    }
    Some(x)
}

/// Result of one LM iteration.
enum LmStep {
    Continue,
    Converged,
    Failed(SolveError),
}

/// Levenberg–Marquardt state for one sub-problem.
struct Lm<const NP: usize> {
    p: [f64; NP],
    jtj: [[f64; NP]; NP],
    jtr: [f64; NP],
    lambda: f64,
    current: f64,
    iterations: u32,
}

impl<const NP: usize> Lm<NP> {
    const fn new(p: [f64; NP]) -> Self {
        Self {
            p,
            jtj: [[0.0; NP]; NP],
            jtr: [0.0; NP],
            lambda: 1e-3,
            current: f64::INFINITY,
            iterations: 0,
        }
    }

    /// Computes the initial normal equations.
    fn start(&mut self, prob: &impl Problem<NP>) {
        self.current = normal_equations(prob, &self.p, &mut self.jtj, &mut self.jtr);
    }

    /// One outer iteration: increase damping until a step reduces the cost, then take it.
    fn step(&mut self, prob: &impl Problem<NP>, opts: &SolveOptions) -> LmStep {
        if !self.current.is_finite() {
            return LmStep::Failed(SolveError::BadInput);
        }
        if self.iterations >= opts.max_iterations {
            return LmStep::Failed(SolveError::NotConverged);
        }
        self.iterations += 1;
        let mut attempts = 0;
        loop {
            let mut damped = self.jtj;
            for j in 0..NP {
                damped[j][j] += self.lambda * self.jtj[j][j].max(1e-12);
            }
            let Some(delta) = cholesky_solve(&damped, &self.jtr) else {
                self.lambda *= 10.0;
                attempts += 1;
                if attempts > 30 {
                    return LmStep::Failed(SolveError::Singular);
                }
                continue;
            };
            let mut trial = self.p;
            for j in 0..NP {
                trial[j] -= delta[j];
            }
            let trial_cost = cost(prob, &trial);
            if trial_cost.is_finite() && trial_cost < self.current {
                let rel = (self.current - trial_cost) / self.current.max(1e-300);
                self.p = trial;
                self.current = normal_equations(prob, &self.p, &mut self.jtj, &mut self.jtr);
                self.lambda = (self.lambda / 3.0).max(1e-12);
                if rel < opts.relative_tolerance || self.current < 1e-28 {
                    return LmStep::Converged;
                }
                return LmStep::Continue;
            }
            self.lambda *= 4.0;
            attempts += 1;
            if attempts > 30 {
                // No step of any size improves: we are at a (local) minimum.
                return LmStep::Converged;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survey::{attitude, synthesize};

    /// Tiny deterministic PRNG for the tests (xorshift64*).
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
            // Box–Muller.
            let u1 = self.next_f64().max(1e-12);
            let u2 = self.next_f64();
            (-2.0 * u1.ln()).sqrt() * (2.0 * core::f64::consts::PI * u2).cos()
        }
    }

    struct Truth {
        acc: [Affine; ACC_COUNT],
        mag: [Affine; MAG_COUNT],
        field: Vec3,
    }

    /// Inverse models: `raw = M⁻¹ v + b`.
    fn raw_of(model: &Affine, v: Vec3) -> Vec3 {
        model.matrix.inverse().unwrap() * v + model.offset
    }

    fn truth() -> Truth {
        // Accelerometer 1 defines the frame: upper-triangular gain (counts⁻¹) with some
        // non-orthogonality, plus a bias.
        let acc0 = Affine {
            offset: Vec3::new(35.0, -12.0, 60.0),
            matrix: Mat3::from_rows(
                [1.0 / 5400.0, 0.003 / 5400.0, -0.002 / 5400.0],
                [0.0, 1.0 / 5380.0, 0.004 / 5400.0],
                [0.0, 0.0, 1.0 / 5420.0],
            ),
        };
        // Accelerometer 2 is mounted rotated 90° about Z and slightly misaligned.
        let acc1 = Affine {
            offset: Vec3::new(-20.0, 8.0, 15.0),
            matrix: (Mat3::rot_z(core::f64::consts::FRAC_PI_2 + 0.01) * Mat3::rot_x(0.005)).transpose()
                * Mat3::diag(Vec3::new(1.0 / 5410.0, 1.0 / 5390.0, 1.0 / 5405.0)),
        };
        // Triads: gain ≈ 148 LSB/µT at CC=400, hard-iron offsets, soft iron and small rotations.
        let g = 148.0;
        let soft = Mat3::from_rows([1.02, 0.01, -0.005], [0.01, 0.98, 0.008], [-0.005, 0.008, 1.01]);
        let mag0 = Affine {
            offset: Vec3::new(1200.0, -800.0, 300.0),
            matrix: (Mat3::rot_x(0.004) * Mat3::rot_y(-0.006) * soft).inverse().unwrap() * (1.0 / (g * 48.0)),
        };
        let mag1 = Affine {
            offset: Vec3::new(-500.0, 900.0, -2500.0),
            matrix: (Mat3::rot_z(0.02) * Mat3::rot_y(0.01) * soft).inverse().unwrap() * (1.0 / (g * 48.0)),
        };
        let mag2 = Affine {
            offset: Vec3::new(300.0, 300.0, 4000.0),
            matrix: (Mat3::rot_x(-0.015) * soft).inverse().unwrap() * (1.0 / (g * 48.0)),
        };
        let dip = crate::math::deg_to_rad(63.0);
        Truth {
            acc: [acc0, acc1],
            mag: [mag0, mag1, mag2],
            field: Vec3::new(48.0 * dip.cos(), 0.0, 48.0 * dip.sin()),
        }
    }

    fn make_shots(truth: &Truth, noise_counts_acc: f64, noise_counts_mag: f64, rng: &mut Rng) -> ShotBuffer {
        let mut buf = ShotBuffer::new();
        // 14 directions spread over azimuth and inclination like a real calibration.
        let mut dirs = Vec::new();
        for i in 0..GROUPS {
            let az = (i as f64) * 360.0 / GROUPS as f64 + rng.uniform(-10.0, 10.0);
            let inc = match i % 7 {
                0 => 0.0,
                1 => 25.0,
                2 => -25.0,
                3 => 55.0,
                4 => -55.0,
                5 => 80.0,
                _ => -80.0,
            } + rng.uniform(-5.0, 5.0);
            dirs.push((az, inc));
        }
        for (gi, (az, inc)) in dirs.iter().enumerate() {
            for roll_i in 0..ROLLS {
                let roll = roll_i as f64 * 90.0 + rng.uniform(-15.0, 15.0);
                let (g, m) = synthesize(*az, *inc, roll, truth.field);
                // Unit field vector in body frame (calibrated space) and reaction force (up).
                let m_unit = m * (1.0 / truth.field.norm());
                let a = -g;
                let mut shot = CalShot {
                    group: gi as u8,
                    roll: roll_i as u8,
                    ..CalShot::default()
                };
                for k in 0..MAG_COUNT {
                    let raw = raw_of(&truth.mag[k], m_unit);
                    shot.mag[k] = raw + Vec3::new(rng.gauss(), rng.gauss(), rng.gauss()) * noise_counts_mag;
                    shot.mag_temp_c[k] = 21.0;
                }
                for k in 0..ACC_COUNT {
                    let raw = raw_of(&truth.acc[k], a);
                    shot.acc[k] = raw + Vec3::new(rng.gauss(), rng.gauss(), rng.gauss()) * noise_counts_acc;
                    shot.acc_temp_c[k] = 24.0;
                }
                buf.push(shot).unwrap();
            }
        }
        buf
    }

    /// Worst attitude error (degrees) over a fresh set of random orientations, for one
    /// (accelerometer, triad) pair.
    fn attitude_error_deg(truth: &Truth, set: &SensorSet, acc_k: usize, mag_k: usize, rng: &mut Rng) -> f64 {
        let mut worst = 0.0f64;
        for _ in 0..200 {
            let az = rng.uniform(0.0, 360.0);
            let inc = rng.uniform(-85.0, 85.0);
            let roll = rng.uniform(-180.0, 180.0);
            let (g, m) = synthesize(az, inc, roll, truth.field);
            let a_raw = raw_of(&truth.acc[acc_k], -g);
            let m_raw = raw_of(&truth.mag[mag_k], m * (1.0 / truth.field.norm()));
            let a_cal = set.acc[acc_k].apply(a_raw);
            let m_cal = set.mag[mag_k].apply(m_raw);
            let att = attitude(-a_cal, m_cal).unwrap();
            let d_az = ((att.azimuth_deg - az + 540.0).rem_euclid(360.0) - 180.0) * inc.to_radians().cos();
            worst = worst.max(d_az.abs()).max((att.inclination_deg - inc).abs());
        }
        worst
    }

    #[test]
    fn recovers_exact_model_without_noise() {
        let truth = truth();
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let shots = make_shots(&truth, 0.0, 0.0, &mut rng);
        let opts = SolveOptions::default();
        let (set, report) = solve(shots.as_slice(), &opts).expect("solve");
        assert!(report.residual_deg < 1e-5, "{report:?}");
        for k in 0..MAG_COUNT {
            for a in 0..ACC_COUNT {
                let e = attitude_error_deg(&truth, &set, a, k, &mut rng);
                assert!(e < 1e-4, "acc{a}/mag{k}: worst error {e}°, report {report:?}");
            }
        }
        assert!((set.dip_deg - 63.0).abs() < 1e-4, "dip {}", set.dip_deg);
        assert!((set.field_ut - 48.0).abs() < 1.0, "field {}", set.field_ut);
        // The primary accelerometer matrix must be upper triangular.
        let m = set.acc[0].matrix.m;
        assert!(m[1][0].abs() < 1e-15 && m[2][0].abs() < 1e-15 && m[2][1].abs() < 1e-15);
    }

    #[test]
    fn recovers_model_with_realistic_noise() {
        let truth = truth();
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        // Averaged-shot noise: ~0.5 counts on the accelerometers (5400/g), ~3 counts on the
        // magnetometers (~7100 counts per 48 µT) → ~0.02° / ~0.025° per axis.
        let shots = make_shots(&truth, 0.5, 3.0, &mut rng);
        let (set, report) = solve(shots.as_slice(), &SolveOptions::default()).expect("solve");
        assert!(report.residual_deg < 0.1, "{report:?}");
        for k in 0..MAG_COUNT {
            for a in 0..ACC_COUNT {
                let e = attitude_error_deg(&truth, &set, a, k, &mut rng);
                assert!(e < 0.15, "acc{a}/mag{k}: worst error {e}°");
            }
        }
    }

    #[test]
    fn refuses_bad_inputs() {
        let truth = truth();
        let mut rng = Rng(42);
        let mut shots = make_shots(&truth, 0.0, 0.0, &mut rng);
        let opts = SolveOptions::default();
        assert_eq!(solve(&shots.as_slice()[..10], &opts), Err(SolveError::NotEnoughShots));
        // All shots in distinct groups → no roll information.
        let mut lonely = *shots.as_slice().first().unwrap();
        let mut buf = ShotBuffer::new();
        for i in 0..30u8 {
            lonely.group = i;
            buf.push(lonely);
        }
        assert_eq!(solve(buf.as_slice(), &opts), Err(SolveError::NotEnoughShots));
        // A zero vector.
        let mut bad = *shots.as_slice().first().unwrap();
        bad.mag[1] = Vec3::ZERO;
        shots.push(bad);
        assert_eq!(solve(shots.as_slice(), &opts), Err(SolveError::BadInput));
    }

    #[test]
    fn shot_buffer_replaces_and_removes() {
        let mut buf = ShotBuffer::new();
        let mut s = CalShot {
            group: 3,
            roll: 1,
            ..CalShot::default()
        };
        s.mag[0] = Vec3::X;
        assert_eq!(buf.push(s), Some(0));
        s.mag[0] = Vec3::Y;
        assert_eq!(buf.push(s), Some(0));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.as_slice()[0].mag[0], Vec3::Y);
        s.roll = 2;
        assert_eq!(buf.push(s), Some(1));
        assert!(buf.remove(3, 1));
        assert!(!buf.remove(3, 1));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.as_slice()[0].roll, 2);
    }

    #[test]
    fn cholesky_solves_spd_system() {
        let a = [[4.0, 1.0, 0.5], [1.0, 3.0, 0.2], [0.5, 0.2, 2.0]];
        let b = [1.0, 2.0, 3.0];
        let x = cholesky_solve(&a, &b).unwrap();
        for i in 0..3 {
            let mut s = 0.0;
            for j in 0..3 {
                s += a[i][j] * x[j];
            }
            assert!((s - b[i]).abs() < 1e-12);
        }
        assert!(cholesky_solve(&[[1.0, 2.0], [2.0, 1.0]], &[1.0, 1.0]).is_none());
    }

    #[test]
    fn job_reports_phases_in_order() {
        let truth = truth();
        let mut rng = Rng(7);
        let shots = make_shots(&truth, 0.0, 0.0, &mut rng);
        let mut job = SolveJob::new(shots.as_slice(), SolveOptions::default()).unwrap();
        let mut seen = Vec::new();
        let result = loop {
            match job.step() {
                Progress::Running { phase, .. } => {
                    if seen.last() != Some(&phase) {
                        seen.push(phase);
                    }
                }
                Progress::Finished(r) => break r,
            }
        };
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(seen, vec![Phase::Pair, Phase::Mag(1), Phase::Mag(2), Phase::Acc(1)]);
        assert_eq!(job.phase(), Phase::Done);
    }
}
