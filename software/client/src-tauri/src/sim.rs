//! A simulated Attinav behind the real wire protocol.
//!
//! The simulator consumes COBS frames and produces COBS frames exactly like the firmware, so the
//! whole client stack (transport, link, codec, UI) is exercised without hardware. Physics is simple
//! but honest: five sensors with distinct affine "truth" models (offsets, gains, misalignments),
//! Gaussian noise, a controllable pose, an optional nearby-iron gradient and motion. Shots are
//! evaluated and calibrations solved with the **same** `attinav-core` code the firmware runs, so a
//! calibration performed against the simulator recovers the truth models to within noise.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use attinav_core::calibration::{ACC_COUNT, Affine, Calibration, MAG_COUNT, SensorId, rm3100_nominal_gain};
use attinav_core::math::{Mat3, Vec3, deg_to_rad};
use attinav_core::protocol::{
    self, CalInfo, CalSetSummary, CalShotReport, CalSolved, DeviceConfig, DeviceMessage, Event, EventKind,
    FIRMWARE_VERSION, HostMessage, Info, LogLine, MAX_ENCODED, Sample, ShotRaw, ShotRequest, ShotResult, Status,
    StreamRequest, Temps, host_kind, sample_source, shot_flags, stream_sensors,
};
use attinav_core::quality::{self, FieldReference, QualityFlags, ShotInput};
use attinav_core::solver::{self, CalShot, ShotBuffer, SolveOptions};
use attinav_core::stats::SampleStats;
use attinav_core::survey::synthesize;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::transport::Transport;

/// Live knobs of the simulated world, shared with the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SimControl {
    /// Pose of the instrument, degrees.
    pub azimuth_deg: f64,
    /// Degrees, positive above the horizon.
    pub inclination_deg: f64,
    /// Degrees, right side down positive.
    pub roll_deg: f64,
    /// Local field magnitude, µT.
    pub field_ut: f64,
    /// Local dip, degrees.
    pub dip_deg: f64,
    /// Per-sample magnetometer noise, raw counts (1 σ per axis).
    pub mag_noise_counts: f64,
    /// Per-sample accelerometer noise, raw counts.
    pub acc_noise_counts: f64,
    /// Simulated gyro peak during a shot, °/s.
    pub motion_dps: f64,
    /// Ambient temperature, °C (coils and dies follow it with small offsets).
    pub temp_c: f64,
    /// Disturbance added to the field seen by triad 1 only, µT (a nearby iron gradient).
    pub nearby_iron_ut: f64,
    /// `Info::sensors_ok` mask; a cleared bit makes that sensor fail.
    pub sensors_ok: u8,
    /// When `true`, calibration shots take the pose implied by their `(group, roll)` instead of
    /// the pose above, so a full 56-shot procedure can be driven from the UI alone.
    pub auto_pose_for_calibration: bool,
}

impl Default for SimControl {
    fn default() -> Self {
        Self {
            azimuth_deg: 123.4,
            inclination_deg: -12.3,
            roll_deg: 0.0,
            field_ut: 48.0,
            dip_deg: 63.0,
            mag_noise_counts: 12.0,
            acc_noise_counts: 4.0,
            motion_dps: 0.05,
            temp_c: 11.0,
            nearby_iron_ut: 0.0,
            sensors_ok: 0x3F,
            auto_pose_for_calibration: true,
        }
    }
}

/// The models a perfect calibration would recover (`calibrated = matrix · (raw − offset)`).
struct Truth {
    mag: [Affine; MAG_COUNT],
    acc: [Affine; ACC_COUNT],
}

impl Truth {
    fn new(cycle_count: u16, field_ut: f64) -> Self {
        let gain = rm3100_nominal_gain(cycle_count) * field_ut;
        let mag_of = |rz: f64, ry: f64, rx: f64, scale: [f64; 3], offset: [f64; 3]| Affine {
            offset: Vec3::from_array(offset),
            matrix: Mat3::rot_z(deg_to_rad(rz))
                * Mat3::rot_y(deg_to_rad(ry))
                * Mat3::rot_x(deg_to_rad(rx))
                * Mat3::diag(Vec3::new(
                    1.0 / (gain * scale[0]),
                    1.0 / (gain * scale[1]),
                    1.0 / (gain * scale[2]),
                )),
        };
        let acc_scale = attinav_core::calibration::SCA3300_MODE4_LSB_PER_G;
        Self {
            mag: [
                mag_of(1.2, -0.8, 0.5, [1.015, 0.992, 1.004], [320.0, -210.0, 140.0]),
                mag_of(-2.1, 0.4, 1.7, [0.988, 1.011, 0.997], [-95.0, 410.0, 60.0]),
                mag_of(0.6, 2.3, -1.1, [1.003, 1.006, 0.985], [180.0, 75.0, -260.0]),
            ],
            acc: [
                Affine {
                    offset: Vec3::new(12.0, -8.0, 20.0),
                    matrix: Mat3::from_rows(
                        [1.012 / acc_scale, 0.004 / acc_scale, -0.002 / acc_scale],
                        [0.0, 0.991 / acc_scale, 0.003 / acc_scale],
                        [0.0, 0.0, 1.005 / acc_scale],
                    ),
                },
                Affine {
                    offset: Vec3::new(-15.0, 9.0, 4.0),
                    matrix: Mat3::rot_z(deg_to_rad(90.0 + 0.7))
                        * Mat3::rot_x(deg_to_rad(-0.4))
                        * Mat3::diag(Vec3::new(1.02 / acc_scale, 0.98 / acc_scale, 1.0 / acc_scale)),
                },
            ],
        }
    }

    /// Raw counts a sensor with model `m` reports for the calibrated vector `v`.
    fn raw(m: &Affine, v: Vec3) -> Vec3 {
        m.matrix.inverse().map_or(Vec3::ZERO, |inv| inv * v) + m.offset
    }
}

struct FwSession {
    size: u32,
    crc32: u32,
    version: [u8; 3],
    data: Vec<u8>,
}

enum BusyKind {
    Shot(ShotRequest),
    CalShot { group: u8, roll: u8 },
}

struct Busy {
    seq: u8,
    kind: BusyKind,
    deadline: Instant,
    aborted: bool,
}

/// Device state.
struct Simulator {
    ctl: Arc<Mutex<SimControl>>,
    rng: StdRng,
    boot: Instant,
    firmware: [u8; 3],
    boot_state: u8,
    config: DeviceConfig,
    cal: Calibration,
    saved: bool,
    shots: ShotBuffer,
    pending_set: Option<attinav_core::calibration::SensorSet>,
    cal_session: bool,
    shot_id: u16,
    clock: Option<(u32, Instant)>,
    stream: Option<(StreamRequest, Duration, Instant, u32)>,
    busy: Option<Busy>,
    fw: Option<FwSession>,
    reset_at: Option<(Instant, Option<[u8; 3]>)>,
}

type Reply = (u8, DeviceMessage);

const UID: [u8; 12] = *b"SIMULATED-01";

impl Simulator {
    fn new(ctl: Arc<Mutex<SimControl>>) -> Self {
        let config = DeviceConfig::default();
        Self {
            ctl,
            rng: StdRng::seed_from_u64(0x5EED_A77A),
            boot: Instant::now(),
            firmware: FIRMWARE_VERSION,
            boot_state: 0,
            cal: Calibration::empty(config.mag_cycle_count),
            config,
            saved: false,
            shots: ShotBuffer::new(),
            pending_set: None,
            cal_session: false,
            shot_id: 0,
            clock: None,
            stream: None,
            busy: None,
            fw: None,
            reset_at: None,
        }
    }

    fn ctl(&self) -> SimControl {
        self.ctl
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn gauss(&mut self) -> f64 {
        // Box–Muller.
        let u1: f64 = self.rng.random::<f64>().max(1e-12);
        let u2: f64 = self.rng.random();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    fn noise3(&mut self, sigma: f64) -> Vec3 {
        Vec3::new(self.gauss() * sigma, self.gauss() * sigma, self.gauss() * sigma)
    }

    fn uptime_ms(&self) -> u32 {
        self.boot.elapsed().as_millis() as u32
    }

    fn unix_now(&self) -> u32 {
        self.clock
            .map_or(0, |(t, at)| t.wrapping_add(at.elapsed().as_secs() as u32))
    }

    const fn event(kind: EventKind, detail: u32) -> Reply {
        (0, DeviceMessage::Event(Event { kind, detail }))
    }

    const fn ack(seq: u8, request: u8, status: Status, detail: u16) -> Reply {
        (seq, protocol::ack(request, status, detail))
    }

    /// Earliest moment [`Self::on_deadline`] must run.
    fn next_deadline(&self) -> Option<Instant> {
        let mut d: Option<Instant> = None;
        let mut consider = |t: Instant| d = Some(d.map_or(t, |x| x.min(t)));
        if let Some(b) = &self.busy {
            consider(b.deadline);
        }
        if let Some((_, _, next, _)) = &self.stream {
            consider(*next);
        }
        if let Some((t, _)) = &self.reset_at {
            consider(*t);
        }
        d
    }

    /// Runs whatever became due.
    fn on_deadline(&mut self) -> Vec<Reply> {
        let now = Instant::now();
        let mut out = Vec::new();
        if let Some((t, version)) = self.reset_at
            && t <= now
        {
            self.reset_at = None;
            out.extend(self.reboot(version));
            return out;
        }
        if self.busy.as_ref().is_some_and(|b| b.deadline <= now) {
            let busy = self.busy.take().expect("checked");
            out.extend(self.finish_busy(&busy));
        }
        if let Some((req, period, next, tick)) = &mut self.stream
            && *next <= now
        {
            *next += *period;
            if *next < now {
                *next = now + *period;
            }
            *tick += 1;
            let (req, tick) = (*req, *tick);
            out.extend(self.stream_tick(req, tick));
        }
        out
    }

    fn reboot(&mut self, version: Option<[u8; 3]>) -> Vec<Reply> {
        info!("sim: reboot (new version {version:?})");
        if let Some(v) = version {
            self.firmware = v;
        }
        self.boot = Instant::now();
        self.boot_state = 0;
        self.busy = None;
        self.stream = None;
        self.fw = None;
        self.clock = None;
        self.cal_session = false;
        self.shots.clear();
        self.pending_set = None;
        let ctl = self.ctl();
        let mut out = vec![Self::event(
            EventKind::Boot,
            u32::from(self.boot_state) << 8 | u32::from(ctl.sensors_ok),
        )];
        // The firmware mirrors its state-change logs to the host; do the same so the client's
        // console and toasts can be exercised without hardware.
        out.push((
            0,
            DeviceMessage::Log(LogLine::new(2, "sim: no stored state, using defaults")),
        ));
        for k in 0..6u8 {
            if ctl.sensors_ok & (1 << k) == 0 {
                let name = ["mag1", "mag2", "mag3", "acc1", "acc2", "imu"][usize::from(k)];
                out.push((
                    0,
                    DeviceMessage::Log(LogLine::new(1, &format!("{name}: probe failed BadId(0)"))),
                ));
            }
        }
        out.push(if self.cal.is_empty() {
            Self::event(EventKind::CalibrationMissing, 0)
        } else {
            Self::event(EventKind::CalibrationLoaded, self.cal.id)
        });
        out
    }

    // ------------------------------------------------------------------------------------------
    // Request handling
    // ------------------------------------------------------------------------------------------

    fn handle(&mut self, seq: u8, msg: &HostMessage) -> Vec<Reply> {
        let kind = msg.kind();
        if self.reset_at.is_some() {
            // Resetting: the real device would be silent.
            return Vec::new();
        }
        if let Some(b) = &mut self.busy {
            return match msg {
                HostMessage::Ping => vec![Self::ack(seq, kind, Status::Ok, 0)],
                HostMessage::Abort => {
                    b.aborted = true;
                    b.deadline = Instant::now();
                    vec![Self::ack(seq, kind, Status::Ok, 0)]
                }
                _ => vec![Self::ack(seq, kind, Status::Busy, 0)],
            };
        }
        let ctl = self.ctl();
        match *msg {
            HostMessage::Ping => vec![Self::ack(seq, kind, Status::Ok, 0)],
            HostMessage::GetInfo => vec![(seq, DeviceMessage::Info(self.info(&ctl)))],
            HostMessage::SetTime(t) => {
                self.clock = Some((t, Instant::now()));
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::Reset => {
                self.reset_at = Some((Instant::now() + Duration::from_millis(300), None));
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::Shot(r) => self.start_shot(seq, r, &ctl),
            HostMessage::Abort => {
                let mut out = Vec::new();
                if self.stream.take().is_some() {
                    out.push(Self::event(EventKind::StreamStopped, 0));
                }
                out.push(Self::ack(seq, kind, Status::Ok, 0));
                out
            }
            HostMessage::Stream(r) => {
                if !r.enable {
                    self.stream = None;
                    return vec![Self::ack(seq, kind, Status::Ok, 0)];
                }
                let rate = r.rate_hz.clamp(1, 500);
                let period = Duration::from_micros(1_000_000 / u64::from(rate));
                self.stream = Some((r, period, Instant::now() + period, 0));
                vec![Self::ack(seq, kind, Status::Ok, rate)]
            }
            HostMessage::GetConfig => vec![(seq, DeviceMessage::Config(self.config))],
            HostMessage::SetConfig(c) => {
                if !c.is_valid() {
                    return vec![Self::ack(seq, kind, Status::InvalidArgument, 0)];
                }
                self.config = c;
                self.saved = true;
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::CalBegin => {
                self.shots.clear();
                self.pending_set = None;
                self.cal_session = true;
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::CalEnd => {
                self.cal_session = false;
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::CalShot { group, roll } => {
                if !self.cal_session {
                    return vec![Self::ack(seq, kind, Status::NotCalibrating, 0)];
                }
                if !can_shoot(ctl.sensors_ok) {
                    return vec![Self::ack(seq, kind, Status::SensorError, u16::from(ctl.sensors_ok))];
                }
                let duration = shot_duration(self.config.mag_samples, self.config.mag_cycle_count, &self.config);
                self.busy = Some(Busy {
                    seq,
                    kind: BusyKind::CalShot { group, roll },
                    deadline: Instant::now() + duration,
                    aborted: false,
                });
                Vec::new()
            }
            HostMessage::CalRemove { group, roll } => {
                let status = if self.shots.remove(group, roll) {
                    Status::Ok
                } else {
                    Status::InvalidArgument
                };
                vec![Self::ack(seq, kind, status, self.shots.len() as u16)]
            }
            HostMessage::CalSolve => vec![(seq, DeviceMessage::CalSolved(self.solve()))],
            HostMessage::CalStore { id } => {
                let Some(set) = self.pending_set.take() else {
                    return vec![Self::ack(seq, kind, Status::NoCalibration, 0)];
                };
                if self.cal.mag_cycle_count != self.config.mag_cycle_count {
                    self.cal = Calibration::empty(self.config.mag_cycle_count);
                }
                let idx = self.cal.upsert_set(set, 5.0);
                self.cal.id = id;
                self.cal.date_unix = self.unix_now();
                self.saved = true;
                vec![Self::ack(seq, kind, Status::Ok, idx as u16)]
            }
            HostMessage::CalLoad => {
                if self.saved {
                    vec![Self::ack(seq, kind, Status::Ok, u16::from(self.cal.n_sets))]
                } else {
                    vec![Self::ack(seq, kind, Status::NoCalibration, 0)]
                }
            }
            HostMessage::CalGet => vec![(seq, DeviceMessage::CalData(self.cal))],
            HostMessage::CalSet(c) => {
                self.cal = c;
                self.saved = true;
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::CalClear => {
                self.cal = Calibration::empty(self.config.mag_cycle_count);
                self.saved = true;
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::CalInfo => vec![(seq, DeviceMessage::CalInfo(self.cal_info()))],
            HostMessage::CalGetShots => {
                let n = self.shots.len();
                let mut out: Vec<Reply> = self
                    .shots
                    .as_slice()
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        (
                            seq,
                            DeviceMessage::CalShotData(CalShotReport {
                                index: i as u8,
                                count: n as u8,
                                flags: QualityFlags::default(),
                                shot: *s,
                            }),
                        )
                    })
                    .collect();
                out.push(Self::ack(seq, kind, Status::Ok, n as u16));
                out
            }
            HostMessage::FwBegin(b) => {
                if b.size == 0 || b.size > 456 * 1024 {
                    return vec![Self::ack(seq, kind, Status::InvalidArgument, 0)];
                }
                self.fw = Some(FwSession {
                    size: b.size,
                    crc32: b.crc32,
                    version: b.version,
                    data: Vec::with_capacity(b.size as usize),
                });
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::FwChunk(c) => {
                let Some(s) = self.fw.as_mut() else {
                    return vec![Self::ack(seq, kind, Status::BadImage, 0)];
                };
                let data = c.data();
                if c.offset as usize != s.data.len()
                    || !c.offset.is_multiple_of(16)
                    || c.offset + data.len() as u32 > s.size
                {
                    self.fw = None;
                    return vec![Self::ack(seq, kind, Status::BadImage, 0)];
                }
                s.data.extend_from_slice(data);
                let received = s.data.len();
                // Flash programming time: ~1.5 ms per KiB.
                std::thread::sleep(Duration::from_micros(1500 * data.len() as u64 / 1024));
                vec![Self::ack(seq, kind, Status::Ok, (received >> 10) as u16)]
            }
            HostMessage::FwFinish => {
                let Some(s) = self.fw.take() else {
                    return vec![Self::ack(seq, kind, Status::BadImage, 0)];
                };
                if s.data.len() as u32 != s.size || crc32fast::hash(&s.data) != s.crc32 {
                    return vec![Self::ack(seq, kind, Status::BadImage, 0)];
                }
                self.boot_state = 1;
                self.reset_at = Some((Instant::now() + Duration::from_millis(1500), Some(s.version)));
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
            HostMessage::FwAbort => {
                self.fw = None;
                vec![Self::ack(seq, kind, Status::Ok, 0)]
            }
        }
    }

    fn info(&self, ctl: &SimControl) -> Info {
        Info {
            protocol: attinav_core::PROTOCOL_VERSION,
            firmware: self.firmware,
            hardware_rev: 2,
            uid: UID,
            uptime_ms: self.uptime_ms(),
            calibration_id: self.cal.id,
            calibration_sets: self.cal.n_sets,
            calibration_date: self.cal.date_unix,
            sensors_ok: ctl.sensors_ok,
            time_set: self.clock.is_some(),
            unix_time: self.unix_now(),
            boot_state: self.boot_state,
        }
    }

    fn cal_info(&self) -> CalInfo {
        let mut sets = [CalSetSummary::default(); attinav_core::calibration::MAX_SETS];
        for (i, s) in self.cal.sets().iter().enumerate() {
            sets[i] = CalSetSummary {
                mag_temp_c: s.mag_temp_c.iter().sum::<f32>() / MAG_COUNT as f32,
                acc_temp_c: s.acc_temp_c.iter().sum::<f32>() / ACC_COUNT as f32,
                residual_deg: s.residual_deg,
                shots: s.shots,
                field_ut: s.field_ut as f32,
                dip_deg: s.dip_deg as f32,
            };
        }
        CalInfo {
            id: self.cal.id,
            date_unix: self.cal.date_unix,
            mag_cycle_count: self.cal.mag_cycle_count,
            n_sets: self.cal.n_sets,
            sets,
            buffered_shots: self.shots.len() as u8,
            session_open: self.cal_session,
            pending_set: self.pending_set.is_some(),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Shots
    // ------------------------------------------------------------------------------------------

    fn start_shot(&mut self, seq: u8, r: ShotRequest, ctl: &SimControl) -> Vec<Reply> {
        if !can_shoot(ctl.sensors_ok) {
            return vec![Self::ack(
                seq,
                host_kind::SHOT,
                Status::SensorError,
                u16::from(ctl.sensors_ok),
            )];
        }
        if self.fw.is_some() {
            return vec![Self::ack(seq, host_kind::SHOT, Status::Busy, 0)];
        }
        let mag_samples = if r.mag_samples == 0 {
            self.config.mag_samples
        } else {
            r.mag_samples
        };
        let cycle_count = if r.cycle_count == 0 {
            self.config.mag_cycle_count
        } else {
            r.cycle_count
        };
        if !protocol::cycle_count_valid(cycle_count) || !(1..=4000).contains(&mag_samples) {
            return vec![Self::ack(seq, host_kind::SHOT, Status::InvalidArgument, 0)];
        }
        self.shot_id = self.shot_id.wrapping_add(1);
        let quiet = self.config.quiet_enabled && r.flags & shot_flags::NO_QUIET == 0;
        let mut duration = shot_duration(mag_samples, cycle_count, &self.config);
        if !quiet {
            duration = duration.saturating_sub(Duration::from_millis(u64::from(self.config.quiet_lead_ms)));
        }
        self.busy = Some(Busy {
            seq,
            kind: BusyKind::Shot(r),
            deadline: Instant::now() + duration,
            aborted: false,
        });
        vec![Self::event(EventKind::ShotStarted, u32::from(self.shot_id))]
    }

    /// Synthesises what the sensors would have averaged over one shot at the given pose.
    fn acquire(
        &mut self,
        pose: (f64, f64, f64),
        mag_samples: u16,
        cycle_count: u16,
        quiet: bool,
        ctl: &SimControl,
    ) -> ShotInput {
        let (az, inc, roll) = pose;
        let dip = deg_to_rad(ctl.dip_deg);
        let field_world = Vec3::new(ctl.field_ut * dip.cos(), 0.0, ctl.field_ut * dip.sin());
        let (g, m_ut) = synthesize(az, inc, roll, field_world);
        let truth = Truth::new(cycle_count, ctl.field_ut);
        let n_mag = f64::from(mag_samples);
        let acc_rate = f64::from(self.config.acc_rate_hz.clamp(10, 2000));
        let n_acc = (shot_duration(mag_samples, cycle_count, &self.config).as_secs_f64() * acc_rate)
            .max(1.0)
            .floor();

        let mut input = ShotInput {
            mag_cycle_count: cycle_count,
            quiet_asserted: quiet,
            short: false,
            gyro_peak_dps: (ctl.sensors_ok & (1 << 5) != 0).then(|| ctl.motion_dps + self.gauss().abs() * 0.01),
            ..ShotInput::default()
        };
        for k in 0..MAG_COUNT {
            if ctl.sensors_ok & (1 << k) == 0 {
                continue;
            }
            // Triad 1 sees the nearby-iron disturbance along the body X axis.
            let disturbance = if k == 0 {
                Vec3::new(ctl.nearby_iron_ut, 0.0, 0.0)
            } else {
                Vec3::ZERO
            };
            let unit = (m_ut + disturbance) * (1.0 / ctl.field_ut);
            let raw = Truth::raw(&truth.mag[k], unit);
            let sigma = ctl.mag_noise_counts;
            input.mag[k] = SampleStats {
                count: u32::from(mag_samples),
                mean: raw + self.noise3(sigma / n_mag.sqrt()),
                std_dev: Vec3::new(sigma, sigma, sigma) * (1.0 + 0.05 * self.gauss()),
            };
            input.mag_temp_c[k] = Some(ctl.temp_c + 0.3 * k as f64 + 0.05 * self.gauss());
        }
        for k in 0..ACC_COUNT {
            if ctl.sensors_ok & (1 << (3 + k)) == 0 {
                continue;
            }
            // The accelerometer measures the reaction force: opposite to gravity.
            let raw = Truth::raw(&truth.acc[k], -g);
            let sigma = ctl.acc_noise_counts;
            input.acc[k] = SampleStats {
                count: n_acc as u32,
                mean: raw + self.noise3(sigma / n_acc.sqrt()),
                std_dev: Vec3::new(sigma, sigma, sigma) * (1.0 + 0.05 * self.gauss()),
            };
            input.acc_temp_c[k] = Some(ctl.temp_c + 2.0 + 0.5 * k as f64 + 0.05 * self.gauss());
        }
        input
    }

    fn reference(&self) -> Option<FieldReference> {
        (self.config.ref_b_total_ut > 0.0).then(|| FieldReference {
            b_total_ut: f64::from(self.config.ref_b_total_ut),
            dip_deg: f64::from(self.config.ref_dip_deg),
        })
    }

    fn finish_busy(&mut self, busy: &Busy) -> Vec<Reply> {
        let ctl = self.ctl();
        match busy.kind {
            BusyKind::Shot(r) => {
                let shot_id = self.shot_id;
                if busy.aborted {
                    return vec![Self::ack(busy.seq, host_kind::SHOT, Status::Aborted, shot_id)];
                }
                let mag_samples = if r.mag_samples == 0 {
                    self.config.mag_samples
                } else {
                    r.mag_samples
                };
                let cycle_count = if r.cycle_count == 0 {
                    self.config.mag_cycle_count
                } else {
                    r.cycle_count
                };
                let quiet = self.config.quiet_enabled && r.flags & shot_flags::NO_QUIET == 0;
                let pose = (ctl.azimuth_deg, ctl.inclination_deg, ctl.roll_deg);
                let input = self.acquire(pose, mag_samples, cycle_count, quiet, &ctl);
                let ev = quality::evaluate(&input, &self.cal, &self.config.thresholds, self.reference());
                let duration = shot_duration(mag_samples, cycle_count, &self.config);
                let result = ShotResult {
                    shot_id,
                    flags: ev.flags,
                    unix_time: self.unix_now(),
                    azimuth_deg: ev.attitude.azimuth_deg as f32,
                    inclination_deg: ev.attitude.inclination_deg as f32,
                    roll_deg: ev.attitude.roll_deg as f32,
                    dip_deg: ev.attitude.dip_deg as f32,
                    b_total_ut: ev.b_total_ut as f32,
                    g_norm: ev.g_norm as f32,
                    mag_spread_deg: ev.mag_spread_deg as f32,
                    acc_spread_deg: ev.acc_spread_deg as f32,
                    azimuth_sigma_deg: ev.azimuth_sigma_deg as f32,
                    inclination_sigma_deg: ev.inclination_sigma_deg as f32,
                    gyro_peak_dps: input.gyro_peak_dps.unwrap_or(f64::NAN) as f32,
                    mag_temp_c: ev.mag_temp_c.map(|t| t as f32),
                    acc_temp_c: ev.acc_temp_c.map(|t| t as f32),
                    calibration_id: self.cal.id,
                    mag_samples: input.mag.iter().map(|s| s.count).max().unwrap_or(0) as u16,
                    acc_samples: input.acc.iter().map(|s| s.count).max().unwrap_or(0) as u16,
                    duration_ms: duration.as_millis() as u16,
                };
                let mut out = vec![(busy.seq, DeviceMessage::ShotResult(result))];
                if r.flags & shot_flags::WANT_RAW != 0 {
                    for (k, st) in input.mag.iter().enumerate() {
                        out.push((
                            busy.seq,
                            DeviceMessage::ShotRaw(ShotRaw {
                                shot_id,
                                sensor: k as u8,
                                count: st.count as u16,
                                mean: st.mean.to_f32_array(),
                                std_dev: st.std_dev.to_f32_array(),
                                calibrated: ev.mag_cal[k].to_f32_array(),
                            }),
                        ));
                    }
                    for (k, st) in input.acc.iter().enumerate() {
                        out.push((
                            busy.seq,
                            DeviceMessage::ShotRaw(ShotRaw {
                                shot_id,
                                sensor: SensorId::Acc1.tag() + k as u8,
                                count: st.count as u16,
                                mean: st.mean.to_f32_array(),
                                std_dev: st.std_dev.to_f32_array(),
                                calibrated: ev.acc_cal[k].to_f32_array(),
                            }),
                        ));
                    }
                }
                out.push(Self::event(EventKind::ShotFinished, u32::from(shot_id)));
                out
            }
            BusyKind::CalShot { group, roll } => {
                if busy.aborted {
                    return vec![Self::ack(busy.seq, host_kind::CAL_SHOT, Status::Aborted, 0)];
                }
                let pose = if ctl.auto_pose_for_calibration {
                    calibration_pose(group, roll)
                } else {
                    (ctl.azimuth_deg, ctl.inclination_deg, ctl.roll_deg)
                };
                let input = self.acquire(
                    pose,
                    self.config.mag_samples,
                    self.config.mag_cycle_count,
                    self.config.quiet_enabled,
                    &ctl,
                );
                if input.mag.iter().any(|s| s.count == 0) || input.acc.iter().any(|s| s.count == 0) {
                    return vec![Self::ack(busy.seq, host_kind::CAL_SHOT, Status::SensorError, 0)];
                }
                let shot = CalShot {
                    group,
                    roll,
                    mag: input.mag.map(|s| s.mean),
                    acc: input.acc.map(|s| s.mean),
                    mag_temp_c: input.mag_temp_c.map(|t| t.unwrap_or(f64::NAN) as f32),
                    acc_temp_c: input.acc_temp_c.map(|t| t.unwrap_or(f64::NAN) as f32),
                };
                let Some(index) = self.shots.push(shot) else {
                    return vec![Self::ack(busy.seq, host_kind::CAL_SHOT, Status::InvalidArgument, 0)];
                };
                let ev = quality::evaluate(&input, &self.cal, &self.config.thresholds, self.reference());
                vec![(
                    busy.seq,
                    DeviceMessage::CalShotDone(CalShotReport {
                        index: index as u8,
                        count: self.shots.len() as u8,
                        flags: ev.flags,
                        shot,
                    }),
                )]
            }
        }
    }

    fn solve(&mut self) -> CalSolved {
        let opts = SolveOptions {
            mag_cycle_count: self.config.mag_cycle_count,
            ..SolveOptions::default()
        };
        let started = Instant::now();
        let result = solver::solve(self.shots.as_slice(), &opts);
        debug!("sim: solve took {:?}", started.elapsed());
        match result {
            Ok((set, report)) => {
                self.pending_set = Some(set);
                CalSolved {
                    error: None,
                    report,
                    dip_deg: set.dip_deg as f32,
                    field_ut: set.field_ut as f32,
                }
            }
            Err(e) => CalSolved {
                error: Some(e),
                ..CalSolved::default()
            },
        }
    }

    fn stream_tick(&mut self, req: StreamRequest, tick: u32) -> Vec<Reply> {
        let ctl = self.ctl();
        let t_ms = self.uptime_ms();
        let dip = deg_to_rad(ctl.dip_deg);
        let field_world = Vec3::new(ctl.field_ut * dip.cos(), 0.0, ctl.field_ut * dip.sin());
        let (g, m_ut) = synthesize(ctl.azimuth_deg, ctl.inclination_deg, ctl.roll_deg, field_world);
        let truth = Truth::new(self.config.mag_cycle_count, ctl.field_ut);
        let mut out = Vec::new();
        let to_raw = |v: Vec3| [v.x as i32, v.y as i32, v.z as i32];
        for k in 0..MAG_COUNT {
            if req.sensors & (1 << k) != 0 && ctl.sensors_ok & (1 << k) != 0 {
                let disturbance = if k == 0 {
                    Vec3::new(ctl.nearby_iron_ut, 0.0, 0.0)
                } else {
                    Vec3::ZERO
                };
                let raw = Truth::raw(&truth.mag[k], (m_ut + disturbance) * (1.0 / ctl.field_ut))
                    + self.noise3(ctl.mag_noise_counts);
                out.push((
                    0,
                    DeviceMessage::Sample(Sample {
                        t_ms,
                        sensor: k as u8,
                        raw: to_raw(raw),
                    }),
                ));
            }
        }
        for k in 0..ACC_COUNT {
            if req.sensors & (stream_sensors::ACC1 << k) != 0 && ctl.sensors_ok & (1 << (3 + k)) != 0 {
                let raw = Truth::raw(&truth.acc[k], -g) + self.noise3(ctl.acc_noise_counts);
                out.push((
                    0,
                    DeviceMessage::Sample(Sample {
                        t_ms,
                        sensor: SensorId::Acc1.tag() + k as u8,
                        raw: to_raw(raw),
                    }),
                ));
            }
        }
        if ctl.sensors_ok & (1 << 5) != 0 {
            if req.sensors & stream_sensors::IMU_GYRO != 0 {
                // ±2000 °/s over 16 bits ≈ 16.4 LSB per °/s.
                let raw = self.noise3(0.3 + ctl.motion_dps * 16.4);
                out.push((
                    0,
                    DeviceMessage::Sample(Sample {
                        t_ms,
                        sensor: sample_source::IMU_GYRO,
                        raw: to_raw(raw),
                    }),
                ));
            }
            if req.sensors & stream_sensors::IMU_ACC != 0 {
                // ±16 g over 16 bits = 2048 LSB/g.
                let raw = (-g) * 2048.0 + self.noise3(6.0);
                out.push((
                    0,
                    DeviceMessage::Sample(Sample {
                        t_ms,
                        sensor: sample_source::IMU_ACC,
                        raw: to_raw(raw),
                    }),
                ));
            }
        }
        if req.sensors & stream_sensors::TEMPS != 0 && tick.is_multiple_of(u32::from(req.rate_hz.clamp(1, 500))) {
            let mut mag_temp_c = [f32::NAN; MAG_COUNT];
            for (k, t) in mag_temp_c.iter_mut().enumerate() {
                if ctl.sensors_ok & (1 << k) != 0 {
                    *t = (ctl.temp_c + 0.3 * k as f64 + 0.05 * self.gauss()) as f32;
                }
            }
            let mut acc_temp_c = [f32::NAN; ACC_COUNT];
            for (k, t) in acc_temp_c.iter_mut().enumerate() {
                if ctl.sensors_ok & (1 << (3 + k)) != 0 {
                    *t = (ctl.temp_c + 2.0 + 0.5 * k as f64 + 0.05 * self.gauss()) as f32;
                }
            }
            let imu_temp_c = if ctl.sensors_ok & (1 << 5) != 0 {
                (ctl.temp_c + 3.0) as f32
            } else {
                f32::NAN
            };
            out.push((
                0,
                DeviceMessage::Temps(Temps {
                    t_ms,
                    mag_temp_c,
                    acc_temp_c,
                    imu_temp_c,
                    vdda_mv: 3298 + (self.gauss() * 2.0) as i32 as u16,
                }),
            ));
        }
        out
    }
}

/// A shot needs at least one triad and one accelerometer (firmware rule).
const fn can_shoot(sensors_ok: u8) -> bool {
    (sensors_ok & 0b0_0111) != 0 && (sensors_ok & 0b1_1000) != 0
}

/// Wall-clock duration of a shot: `QUIET` lead plus `n` polled RM3100 measurements.
fn shot_duration(mag_samples: u16, cycle_count: u16, config: &DeviceConfig) -> Duration {
    let rate_hz = 88_000.0 / 3.0 / f64::from(cycle_count.max(1));
    let acquisition = f64::from(mag_samples) / rate_hz;
    let lead = if config.quiet_enabled {
        f64::from(config.quiet_lead_ms) / 1000.0
    } else {
        0.0
    };
    Duration::from_secs_f64(acquisition + lead)
}

/// A well-spread pose for calibration slot `(group, roll)`: 14 directions covering the sphere,
/// four roll angles 90° apart with a little jitter so no two shots are degenerate.
fn calibration_pose(group: u8, roll: u8) -> (f64, f64, f64) {
    let g = f64::from(group);
    // The laser direction must be the same for the four rolls of a group: only the roll varies.
    let azimuth = (g * 77.0).rem_euclid(360.0);
    let inclinations = [-70.0, -40.0, -15.0, 0.0, 15.0, 40.0, 70.0];
    let inclination = inclinations[(group as usize) % inclinations.len()] + if group >= 7 { 5.0 } else { -5.0 };
    let roll_deg = f64::from(roll) * 90.0 + 7.0 * ((g * 1.3).sin());
    (azimuth, inclination, roll_deg)
}

/// Starts a simulated device and returns its transport.
pub fn open_simulator(ctl: Arc<Mutex<SimControl>>) -> Transport {
    let (to_device, mut host_rx) = mpsc::channel::<Vec<u8>>(64);
    let (dev_tx, from_device) = mpsc::channel::<Vec<u8>>(1024);
    let stop = Arc::new(AtomicBool::new(false));
    let transport = Transport::new(to_device, from_device, Arc::clone(&stop));

    tauri::async_runtime::spawn(async move {
        let mut sim = Simulator::new(ctl);
        let mut dec = Box::new(attinav_core::protocol::Decoder::new());
        let mut buf = vec![0u8; MAX_ENCODED];
        // Boot banner, like the firmware.
        let boot = sim.reboot(None);
        if send_all(&dev_tx, &boot, &mut buf).await.is_err() {
            return;
        }
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let deadline = sim.next_deadline();
            let timer = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d.into()).await,
                    None => tokio::time::sleep(Duration::from_millis(200)).await,
                }
            };
            tokio::select! {
                bytes = host_rx.recv() => {
                    let Some(bytes) = bytes else { break };
                    for b in bytes {
                        let Some(frame) = dec.push(b) else { continue };
                        let Ok(h) = frame else { continue };
                        let replies = HostMessage::read_payload(h.kind, dec.payload()).map_or_else(
                            |_| vec![(h.seq, protocol::ack(h.kind, Status::Unsupported, 0))],
                            |m| sim.handle(h.seq, &m),
                        );
                        if send_all(&dev_tx, &replies, &mut buf).await.is_err() {
                            return;
                        }
                    }
                }
                () = timer => {
                    let replies = sim.on_deadline();
                    if send_all(&dev_tx, &replies, &mut buf).await.is_err() {
                        return;
                    }
                }
            }
        }
        info!("sim: stopped");
    });
    transport
}

async fn send_all(tx: &mpsc::Sender<Vec<u8>>, replies: &[Reply], buf: &mut [u8]) -> Result<(), ()> {
    for (seq, msg) in replies {
        let n = msg.encode(*seq, buf).map_err(|_| ())?;
        tx.send(buf[..n].to_vec()).await.map_err(|_| ())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truth_models_are_invertible_and_recoverable() {
        let truth = Truth::new(400, 48.0);
        let v = Vec3::new(0.3, -0.5, 0.81);
        for m in truth.mag.iter().chain(truth.acc.iter()) {
            let raw = Truth::raw(m, v);
            let back = m.apply(raw);
            assert!((back - v).norm() < 1e-9);
        }
    }

    #[test]
    fn calibration_poses_cover_the_sphere() {
        let mut incs = std::collections::BTreeSet::new();
        for g in 0..14u8 {
            for r in 0..4u8 {
                let (az, inc, roll) = calibration_pose(g, r);
                assert!((0.0..360.0).contains(&az));
                assert!(inc.abs() <= 80.0);
                assert!(roll.is_finite());
                incs.insert(inc.round() as i32);
            }
        }
        assert!(incs.len() >= 7);
    }

    /// A full simulated calibration recovers the truth to well under the acceptance threshold.
    #[test]
    fn simulated_calibration_solves() {
        let ctl = Arc::new(Mutex::new(SimControl {
            mag_noise_counts: 2.0,
            acc_noise_counts: 1.0,
            ..SimControl::default()
        }));
        let mut sim = Simulator::new(ctl);
        for g in 0..14u8 {
            for r in 0..4u8 {
                let c = sim.ctl();
                let input = sim.acquire(calibration_pose(g, r), 100, 400, true, &c);
                sim.shots.push(CalShot {
                    group: g,
                    roll: r,
                    mag: input.mag.map(|s| s.mean),
                    acc: input.acc.map(|s| s.mean),
                    mag_temp_c: [11.0; MAG_COUNT],
                    acc_temp_c: [13.0; ACC_COUNT],
                });
            }
        }
        let solved = sim.solve();
        assert!(solved.error.is_none(), "{solved:?}");
        assert!(
            solved.report.residual_deg < 0.1,
            "residual {}",
            solved.report.residual_deg
        );
        assert!((solved.dip_deg - 63.0).abs() < 0.5);
        assert!((solved.field_ut - 48.0).abs() < 1.5);
    }
}
