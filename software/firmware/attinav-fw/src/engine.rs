//! The application: sensor bring-up, shot engine, streaming, calibration sessions, configuration,
//! firmware update staging — everything behind the host protocol.
//!
//! Runs as one cooperative loop in the main task. Long operations (a shot, a solve) check the
//! inbound queue between steps so an `Abort` is honoured and other requests get a `Busy` reply
//! instead of waiting.

use defmt::info;
use embassy_futures::join::join;
use embassy_futures::select::{Either3, select3};
use embassy_futures::yield_now;
use embassy_time::{Duration, Instant, Ticker, Timer};

use attinav_core::calibration::{ACC_COUNT, Calibration, MAG_COUNT, SensorId, SensorSet};
use attinav_core::math::Vec3;
use attinav_core::protocol::{
    self, CalInfo, CalSetSummary, CalShotReport, CalSolved, DeviceConfig, DeviceMessage, Event, EventKind, HostMessage,
    Info, Sample, ShotRaw, ShotRequest, ShotResult, Status, StreamRequest, Temps, host_kind, sample_source, shot_flags,
    stream_sensors,
};
use attinav_core::quality::{self, FieldReference, QualityFlags, ShotInput};
use attinav_core::solver::{CalShot, Progress, ShotBuffer, SolveJob, SolveOptions};
use attinav_core::stats::Accumulator3;
use attinav_core::wire::WireError;

use crate::board;
use crate::link::{self, Inbound, LinkId};
use crate::sensors::Sensors;
use crate::store::{FwUpdate, Store, UpdateError};

/// Bits of `Info::sensors_ok` for the triads and the accelerometers.
const MAG_MASK: u8 = 0b0_0111;
const ACC_MASK: u8 = 0b1_1000;

/// A shot needs at least one triad and one accelerometer.
const fn can_shoot(sensors_ok: u8) -> bool {
    (sensors_ok & MAG_MASK) != 0 && (sensors_ok & ACC_MASK) != 0
}

/// Reply helper.
async fn ack(link: LinkId, seq: u8, request: u8, status: Status, detail: u16) {
    link::send(link, &protocol::ack(request, status, detail), seq).await;
}

/// Wall-clock from the host.
#[derive(Clone, Copy)]
struct Clock {
    unix_at_set: u32,
    set_at: Instant,
}

impl Clock {
    fn now(self) -> u32 {
        self.unix_at_set.wrapping_add(self.set_at.elapsed().as_secs() as u32)
    }
}

/// Result of the acquisition phase of a shot.
struct Acquisition {
    mag: [Accumulator3; MAG_COUNT],
    acc: [Accumulator3; ACC_COUNT],
    acc_temp_c: [Option<f64>; ACC_COUNT],
    gyro_peak_dps: Option<f64>,
    aborted: bool,
    duration: Duration,
}

/// What a long operation found in the inbound queue while running.
enum Interrupt {
    None,
    Abort,
}

/// Engine state.
struct Engine {
    sensors: Sensors,
    store: Store,
    fw: FwUpdate,
    config: DeviceConfig,
    cal: Calibration,
    shots: ShotBuffer,
    pending_set: Option<SensorSet>,
    cal_session: bool,
    shot_id: u16,
    clock: Option<Clock>,
    stream: Option<(StreamRequest, LinkId, Ticker, u32)>,
    sensors_ok: u8,
    boot_state: u8,
    led: embassy_stm32::gpio::Output<'static>,
    watchdog: embassy_stm32::wdg::IndependentWatchdog<'static, embassy_stm32::peripherals::IWDG>,
    boot: Instant,
}

/// Brings the sensors up and serves requests forever.
// `cal` and `config` are moved into the `Engine`, not copied: one move beats a reference plus a
// clone, and `run` is called exactly once from `main`.
#[allow(clippy::large_types_passed_by_value)]
pub async fn run(
    sensors: Sensors,
    led: embassy_stm32::gpio::Output<'static>,
    watchdog: embassy_stm32::wdg::IndependentWatchdog<'static, embassy_stm32::peripherals::IWDG>,
    store: Store,
    fw: FwUpdate,
    config: DeviceConfig,
    cal: Calibration,
) -> ! {
    let mut e = Engine {
        sensors,
        store,
        fw,
        config,
        cal,
        shots: ShotBuffer::new(),
        pending_set: None,
        cal_session: false,
        shot_id: 0,
        clock: None,
        stream: None,
        sensors_ok: 0,
        boot_state: 255,
        led,
        watchdog,
        boot: Instant::now(),
    };
    e.sensors_ok = e.sensors.bring_up(e.config.mag_cycle_count).await;
    e.watchdog.pet();
    e.boot_state = e.fw.boot_state().await;
    // The image reached the request loop with working peripherals: keep it.
    e.fw.mark_booted().await;
    e.boot_state = e.fw.boot_state().await;
    link::broadcast(&DeviceMessage::Event(Event {
        kind: EventKind::Boot,
        detail: u32::from(e.boot_state) << 8 | u32::from(e.sensors_ok),
    }));
    if e.cal.is_empty() {
        link::broadcast(&DeviceMessage::Event(Event {
            kind: EventKind::CalibrationMissing,
            detail: 0,
        }));
    } else {
        link::broadcast(&DeviceMessage::Event(Event {
            kind: EventKind::CalibrationLoaded,
            detail: e.cal.id,
        }));
    }
    board::led_set(&mut e.led, false);
    info!(
        "engine: ready (sensors {:#04x}, boot state {})",
        e.sensors_ok, e.boot_state
    );

    loop {
        e.watchdog.pet();
        let housekeeping = Timer::after_secs(1);
        match &mut e.stream {
            Some((_, _, ticker, _)) => match select3(link::INBOUND.receive(), ticker.next(), housekeeping).await {
                Either3::First(req) => {
                    e.handle(&req).await;
                    // A request may have taken seconds (a shot); do not replay the missed ticks.
                    if let Some((_, _, ticker, _)) = &mut e.stream {
                        ticker.reset();
                    }
                }
                Either3::Second(()) => e.stream_tick().await,
                Either3::Third(()) => {}
            },
            None => {
                if let Either3::First(req) =
                    select3(link::INBOUND.receive(), core::future::pending::<()>(), housekeeping).await
                {
                    e.handle(&req).await;
                }
            }
        }
    }
}

impl Engine {
    fn unix_now(&self) -> u32 {
        self.clock.map_or(0, Clock::now)
    }

    fn reference(&self) -> Option<FieldReference> {
        (self.config.ref_b_total_ut > 0.0).then(|| FieldReference {
            b_total_ut: f64::from(self.config.ref_b_total_ut),
            dip_deg: f64::from(self.config.ref_dip_deg),
        })
    }

    fn led(&mut self, on: bool) {
        if self.config.led_enabled {
            board::led_set(&mut self.led, on);
        }
    }

    /// Writes configuration and calibration to flash and reports the outcome as a reply status.
    async fn persist(&mut self) -> Status {
        match self.store.save(&self.config, &self.cal).await {
            Ok(()) => Status::Ok,
            Err(e) => {
                host_error!("store: save failed: {:?}", e);
                Status::FlashError
            }
        }
    }

    /// Programs `cycle_count` into the triads when it differs from the current one; `false` when a
    /// working triad refused it (the shot would then be mis-scaled).
    async fn apply_cycle_count(&mut self, cycle_count: u16) -> bool {
        !self.sensors.mags.needs_cycle_count(cycle_count) || self.sensors.mags.set_cycle_count(cycle_count).await
    }
}

/// Checks the inbound queue without waiting: acknowledges `Abort` and `Ping`, answers `Busy` to
/// anything else. A free function so it can run while a solve job borrows engine fields.
async fn poll_interrupt() -> Interrupt {
    let Ok(req) = link::INBOUND.try_receive() else {
        return Interrupt::None;
    };
    let kind = req.header.kind;
    match kind {
        host_kind::ABORT => {
            ack(req.link, req.header.seq, kind, Status::Ok, 0).await;
            Interrupt::Abort
        }
        host_kind::PING => {
            ack(req.link, req.header.seq, kind, Status::Ok, 0).await;
            Interrupt::None
        }
        _ => {
            ack(req.link, req.header.seq, kind, Status::Busy, 0).await;
            Interrupt::None
        }
    }
}

impl Engine {
    // ------------------------------------------------------------------------------------------
    // Request dispatch
    // ------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_lines, reason = "one match arm per request kind")]
    async fn handle(&mut self, req: &Inbound) {
        let link = req.link;
        let seq = req.header.seq;
        let kind = req.header.kind;
        let msg = match HostMessage::read_payload(kind, req.payload()) {
            Ok(m) => m,
            Err(WireError::Invalid) if !host_kind_known(kind) => {
                host_warn!("unknown request kind {:#04x}", kind);
                return ack(link, seq, kind, Status::Unsupported, 0).await;
            }
            Err(e) => {
                host_warn!("bad payload for {:#04x}: {:?}", kind, e);
                return ack(link, seq, kind, Status::BadPayload, 0).await;
            }
        };
        match msg {
            HostMessage::Ping => ack(link, seq, kind, Status::Ok, 0).await,
            HostMessage::GetInfo => {
                let info = self.info();
                link::send(link, &DeviceMessage::Info(info), seq).await;
            }
            HostMessage::SetTime(t) => {
                self.clock = Some(Clock {
                    unix_at_set: t,
                    set_at: Instant::now(),
                });
                ack(link, seq, kind, Status::Ok, 0).await;
            }
            HostMessage::Reset => {
                ack(link, seq, kind, Status::Ok, 0).await;
                Timer::after_millis(100).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
            HostMessage::Shot(r) => self.shot(link, seq, r).await,
            HostMessage::Abort => {
                if self.stream.take().is_some() {
                    link::broadcast(&DeviceMessage::Event(Event {
                        kind: EventKind::StreamStopped,
                        detail: 0,
                    }));
                }
                ack(link, seq, kind, Status::Ok, 0).await;
            }
            HostMessage::Stream(r) => self.set_stream(link, seq, r).await,
            HostMessage::GetConfig => link::send(link, &DeviceMessage::Config(self.config), seq).await,
            HostMessage::SetConfig(c) => self.set_config(link, seq, c).await,
            HostMessage::CalBegin => {
                self.shots.clear();
                self.pending_set = None;
                self.cal_session = true;
                ack(link, seq, kind, Status::Ok, 0).await;
            }
            HostMessage::CalEnd => {
                self.cal_session = false;
                ack(link, seq, kind, Status::Ok, 0).await;
            }
            HostMessage::CalShot { group, roll } => self.cal_shot(link, seq, group, roll).await,
            HostMessage::CalRemove { group, roll } => {
                let status = if self.shots.remove(group, roll) {
                    Status::Ok
                } else {
                    Status::InvalidArgument
                };
                ack(link, seq, kind, status, self.shots.len() as u16).await;
            }
            HostMessage::CalSolve => self.cal_solve(link, seq).await,
            HostMessage::CalStore { id } => self.cal_store(link, seq, id).await,
            HostMessage::CalLoad => match self.store.load().await {
                Some((config, cal)) => {
                    if config.is_valid() {
                        self.config = config;
                    }
                    self.cal = cal;
                    ack(link, seq, kind, Status::Ok, self.cal.n_sets.into()).await;
                }
                None => ack(link, seq, kind, Status::NoCalibration, 0).await,
            },
            HostMessage::CalGet => link::send(link, &DeviceMessage::CalData(self.cal), seq).await,
            HostMessage::CalSet(cal) => {
                self.cal = cal;
                let status = self.persist().await;
                ack(link, seq, kind, status, 0).await;
            }
            HostMessage::CalClear => {
                self.cal = Calibration::empty(self.config.mag_cycle_count);
                let status = self.persist().await;
                ack(link, seq, kind, status, 0).await;
            }
            HostMessage::CalInfo => {
                let info = self.cal_info();
                link::send(link, &DeviceMessage::CalInfo(info), seq).await;
            }
            HostMessage::CalGetShots => {
                let n = self.shots.len();
                for (i, s) in self.shots.as_slice().iter().enumerate() {
                    let rep = CalShotReport {
                        index: i as u8,
                        count: n as u8,
                        flags: QualityFlags::default(),
                        shot: *s,
                    };
                    link::send(link, &DeviceMessage::CalShotData(rep), seq).await;
                }
                ack(link, seq, kind, Status::Ok, n as u16).await;
            }
            HostMessage::FwBegin(b) => {
                let status = match self.fw.begin(&b).await {
                    Ok(()) => Status::Ok,
                    Err(UpdateError::Size) => Status::InvalidArgument,
                    Err(_) => Status::BadImage,
                };
                ack(link, seq, kind, status, 0).await;
            }
            HostMessage::FwChunk(c) => {
                self.led(true);
                let (status, detail) = match self.fw.chunk(&c).await {
                    Ok(end) => (Status::Ok, (end >> 10) as u16),
                    Err(UpdateError::Flash) => (Status::FlashError, 0),
                    Err(_) => (Status::BadImage, 0),
                };
                self.led(false);
                ack(link, seq, kind, status, detail).await;
            }
            HostMessage::FwFinish => {
                let status = match self.fw.finish().await {
                    Ok(()) => Status::Ok,
                    Err(UpdateError::Flash) => Status::FlashError,
                    Err(_) => Status::BadImage,
                };
                ack(link, seq, kind, status, 0).await;
                if status == Status::Ok {
                    host_info!("fw: rebooting into the new image");
                    Timer::after_millis(200).await;
                    cortex_m::peripheral::SCB::sys_reset();
                }
            }
            HostMessage::FwAbort => {
                self.fw.abort();
                ack(link, seq, kind, Status::Ok, 0).await;
            }
        }
    }

    fn info(&self) -> Info {
        Info {
            protocol: attinav_core::PROTOCOL_VERSION,
            firmware: protocol::FIRMWARE_VERSION,
            hardware_rev: board::HARDWARE_REV,
            uid: embassy_stm32::uid::uid(),
            uptime_ms: self.boot.elapsed().as_millis() as u32,
            calibration_id: self.cal.id,
            calibration_sets: self.cal.n_sets,
            calibration_date: self.cal.date_unix,
            sensors_ok: self.sensors_ok,
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

    async fn set_config(&mut self, link: LinkId, seq: u8, c: DeviceConfig) {
        if !c.is_valid() {
            return ack(link, seq, host_kind::SET_CONFIG, Status::InvalidArgument, 0).await;
        }
        if c.mag_cycle_count != self.sensors.mags.cycle_count()
            && !self.sensors.mags.set_cycle_count(c.mag_cycle_count).await
        {
            return ack(link, seq, host_kind::SET_CONFIG, Status::SensorError, 0).await;
        }
        self.config = c;
        let status = self.persist().await;
        ack(link, seq, host_kind::SET_CONFIG, status, 0).await;
    }

    // ------------------------------------------------------------------------------------------
    // Streaming
    // ------------------------------------------------------------------------------------------

    async fn set_stream(&mut self, link: LinkId, seq: u8, r: StreamRequest) {
        if !r.enable {
            if self.stream.take().is_some() {
                link::broadcast(&DeviceMessage::Event(Event {
                    kind: EventKind::StreamStopped,
                    detail: 0,
                }));
            }
            return ack(link, seq, host_kind::STREAM, Status::Ok, 0).await;
        }
        // A polled RM3100 measurement bounds the rate when triads are selected; a faster ticker
        // would only run permanently behind.
        let mut rate = r.rate_hz.clamp(1, 500);
        if r.sensors & (stream_sensors::MAG1 | stream_sensors::MAG2 | stream_sensors::MAG3) != 0 {
            let max_hz = attinav_sensors::rm3100::max_sample_rate_hz(self.sensors.mags.cycle_count()) * 0.9;
            rate = rate.min((max_hz as u16).max(1));
        }
        self.sensors.mags.begin_window();
        let ticker = Ticker::every(Duration::from_micros(1_000_000 / u64::from(rate)));
        self.stream = Some((r, link, ticker, 0));
        ack(link, seq, host_kind::STREAM, Status::Ok, rate).await;
    }

    async fn stream_tick(&mut self) {
        let Some((req, _link, _, tick)) = &mut self.stream else {
            return;
        };
        let req = *req;
        *tick += 1;
        let tick = *tick;
        let t_ms = self.boot.elapsed().as_millis() as u32;
        let sensors = req.sensors;
        let want_mag = sensors & (stream_sensors::MAG1 | stream_sensors::MAG2 | stream_sensors::MAG3) != 0;
        let want_acc = sensors & (stream_sensors::ACC1 | stream_sensors::ACC2) != 0;
        let want_imu = sensors & (stream_sensors::IMU_GYRO | stream_sensors::IMU_ACC) != 0;

        if want_mag {
            let m = self.sensors.mags.sample().await;
            for (k, v) in m.iter().enumerate() {
                if sensors & (1 << k) != 0
                    && let Some(v) = v
                {
                    link::broadcast(&DeviceMessage::Sample(Sample {
                        t_ms,
                        sensor: k as u8,
                        raw: [v.x as i32, v.y as i32, v.z as i32],
                    }));
                }
            }
        }
        if want_acc {
            let a = self.sensors.accels.read().await;
            for (k, v) in a.iter().enumerate() {
                if sensors & (stream_sensors::ACC1 << k) != 0
                    && let Some((v, _t)) = v
                {
                    link::broadcast(&DeviceMessage::Sample(Sample {
                        t_ms,
                        sensor: SensorId::Acc1.tag() + k as u8,
                        raw: [v.x as i32, v.y as i32, v.z as i32],
                    }));
                }
            }
        }
        if want_imu && let Some(r) = self.sensors.accels.read_imu().await {
            if sensors & stream_sensors::IMU_GYRO != 0 {
                link::broadcast(&DeviceMessage::Sample(Sample {
                    t_ms,
                    sensor: sample_source::IMU_GYRO,
                    raw: r.gyro.map(i32::from),
                }));
            }
            if sensors & stream_sensors::IMU_ACC != 0 {
                link::broadcast(&DeviceMessage::Sample(Sample {
                    t_ms,
                    sensor: sample_source::IMU_ACC,
                    raw: r.accel.map(i32::from),
                }));
            }
        }
        // Temperatures once a second regardless of the stream rate.
        if sensors & stream_sensors::TEMPS != 0 && tick.is_multiple_of(u32::from(req.rate_hz.clamp(1, 500))) {
            let t = self.sensors.temps.read();
            let acc = self.sensors.accels.read().await;
            let imu = self.sensors.accels.read_imu().await;
            link::broadcast(&DeviceMessage::Temps(Temps {
                t_ms,
                mag_temp_c: t.mag_c.map(|c| c.map_or(f32::NAN, |v| v as f32)),
                acc_temp_c: acc.map(|a| a.map_or(f32::NAN, |(_, t)| t)),
                imu_temp_c: imu.map_or(f32::NAN, |r| r.temp_c),
                vdda_mv: t.vdda_mv,
            }));
        }
    }

    // ------------------------------------------------------------------------------------------
    // Shots
    // ------------------------------------------------------------------------------------------

    /// Acquires one shot's worth of samples: `n_mag` polled magnetometer samples while the
    /// accelerometers and the IMU are read at `acc_rate_hz`.
    async fn acquire(&mut self, n_mag: u16, acc_rate_hz: u16) -> Acquisition {
        let start = Instant::now();
        let mut mag = [Accumulator3::new(); MAG_COUNT];
        let mut acc = [Accumulator3::new(); ACC_COUNT];
        let mut acc_temp = [Accumulator3::new(); ACC_COUNT];
        let mut gyro_peak: Option<f64> = None;
        let mut aborted = false;

        let mags = &mut self.sensors.mags;
        let accels = &mut self.sensors.accels;
        let watchdog = &mut self.watchdog;
        // Set by the magnetometer loop when it is done; the accelerometer loop then exits at its
        // next tick instead of being cancelled in the middle of an SPI frame.
        let done = core::cell::Cell::new(false);

        mags.begin_window();
        let mag_fut = async {
            for _ in 0..n_mag {
                // A shot may legitimately run for minutes (4000 samples at a high cycle count).
                watchdog.pet();
                let s = mags.sample().await;
                for (k, v) in s.iter().enumerate() {
                    if let Some(v) = v {
                        mag[k].push(*v);
                    }
                }
                if mags.active().iter().all(|a| !a) {
                    break;
                }
                if let Ok(req) = link::INBOUND.try_receive() {
                    let kind = req.header.kind;
                    let status = if kind == host_kind::ABORT || kind == host_kind::PING {
                        Status::Ok
                    } else {
                        Status::Busy
                    };
                    ack(req.link, req.header.seq, kind, status, 0).await;
                    if kind == host_kind::ABORT {
                        aborted = true;
                        break;
                    }
                }
            }
            done.set(true);
        };
        let acc_fut = async {
            let mut ticker = Ticker::every(Duration::from_micros(
                1_000_000 / u64::from(acc_rate_hz.clamp(10, 2000)),
            ));
            loop {
                ticker.next().await;
                if done.get() {
                    break;
                }
                let a = accels.read().await;
                for (k, v) in a.iter().enumerate() {
                    if let Some((v, t)) = v {
                        acc[k].push(*v);
                        acc_temp[k].push(Vec3::new(f64::from(*t), 0.0, 0.0));
                    }
                }
                if let Some(r) = accels.read_imu().await {
                    let rate = f64::from(r.rate_dps);
                    gyro_peak = Some(gyro_peak.map_or(rate, |p| p.max(rate)));
                }
            }
        };
        join(mag_fut, acc_fut).await;
        for (k, faulted) in mags.faults().iter().enumerate() {
            if *faulted {
                link::broadcast(&DeviceMessage::Event(Event {
                    kind: EventKind::SensorFault,
                    detail: k as u32,
                }));
            }
        }

        Acquisition {
            mag,
            acc,
            acc_temp_c: acc_temp.map(|a| (a.count() > 0).then(|| a.mean().x)),
            gyro_peak_dps: if accels.imu_ok { gyro_peak } else { None },
            aborted,
            duration: start.elapsed(),
        }
    }

    /// Runs the full shot sequence; returns the evaluated input and the raw acquisition.
    ///
    /// `cycle_count` must have been validated ([`protocol::cycle_count_valid`]) and programmed
    /// into the triads by the caller.
    async fn take_shot(
        &mut self,
        mag_samples: u16,
        cycle_count: u16,
        quiet: bool,
    ) -> (ShotInput, Acquisition, crate::sensors::TempReading) {
        let temps = self.sensors.temps.read();
        if quiet {
            self.sensors.set_quiet(true);
            Timer::after_millis(u64::from(self.config.quiet_lead_ms)).await;
        }
        self.led(true);
        let acq = self.acquire(mag_samples, self.config.acc_rate_hz).await;
        self.led(false);
        if quiet {
            self.sensors.set_quiet(false);
        }
        let input = ShotInput {
            mag: acq.mag.map(|a| a.summary()),
            acc: acq.acc.map(|a| a.summary()),
            gyro_peak_dps: acq.gyro_peak_dps,
            mag_temp_c: temps.mag_c,
            acc_temp_c: acq.acc_temp_c,
            mag_cycle_count: cycle_count,
            quiet_asserted: quiet,
            short: acq.aborted || acq.mag.iter().any(|a| a.count() < u32::from(mag_samples)),
        };
        (input, acq, temps)
    }

    async fn shot(&mut self, link: LinkId, seq: u8, r: ShotRequest) {
        if !can_shoot(self.sensors_ok) {
            return ack(
                link,
                seq,
                host_kind::SHOT,
                Status::SensorError,
                u16::from(self.sensors_ok),
            )
            .await;
        }
        if self.fw.in_progress() {
            return ack(link, seq, host_kind::SHOT, Status::Busy, 0).await;
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
            return ack(link, seq, host_kind::SHOT, Status::InvalidArgument, 0).await;
        }
        if !self.apply_cycle_count(cycle_count).await {
            return ack(link, seq, host_kind::SHOT, Status::SensorError, 0).await;
        }
        let quiet = self.config.quiet_enabled && r.flags & shot_flags::NO_QUIET == 0;
        self.shot_id = self.shot_id.wrapping_add(1);
        let shot_id = self.shot_id;
        link::broadcast(&DeviceMessage::Event(Event {
            kind: EventKind::ShotStarted,
            detail: u32::from(shot_id),
        }));

        let (input, acq, _temps) = self.take_shot(mag_samples, cycle_count, quiet).await;
        if acq.aborted {
            ack(link, seq, host_kind::SHOT, Status::Aborted, shot_id).await;
            return link::broadcast(&DeviceMessage::Event(Event {
                kind: EventKind::ShotFinished,
                detail: u32::from(shot_id),
            }));
        }
        let ev = quality::evaluate(&input, &self.cal, &self.config.thresholds, self.reference());
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
            duration_ms: acq.duration.as_millis() as u16,
        };
        link::send(link, &DeviceMessage::ShotResult(result), seq).await;
        if r.flags & shot_flags::WANT_RAW != 0 {
            for (k, st) in input.mag.iter().enumerate() {
                let raw = ShotRaw {
                    shot_id,
                    sensor: k as u8,
                    count: st.count as u16,
                    mean: st.mean.to_f32_array(),
                    std_dev: st.std_dev.to_f32_array(),
                    calibrated: ev.mag_cal[k].to_f32_array(),
                };
                link::send(link, &DeviceMessage::ShotRaw(raw), seq).await;
            }
            for (k, st) in input.acc.iter().enumerate() {
                let raw = ShotRaw {
                    shot_id,
                    sensor: SensorId::Acc1.tag() + k as u8,
                    count: st.count as u16,
                    mean: st.mean.to_f32_array(),
                    std_dev: st.std_dev.to_f32_array(),
                    calibrated: ev.acc_cal[k].to_f32_array(),
                };
                link::send(link, &DeviceMessage::ShotRaw(raw), seq).await;
            }
        }
        link::broadcast(&DeviceMessage::Event(Event {
            kind: EventKind::ShotFinished,
            detail: u32::from(shot_id),
        }));
    }

    // ------------------------------------------------------------------------------------------
    // Calibration
    // ------------------------------------------------------------------------------------------

    async fn cal_shot(&mut self, link: LinkId, seq: u8, group: u8, roll: u8) {
        if !self.cal_session {
            return ack(link, seq, host_kind::CAL_SHOT, Status::NotCalibrating, 0).await;
        }
        if !can_shoot(self.sensors_ok) {
            return ack(
                link,
                seq,
                host_kind::CAL_SHOT,
                Status::SensorError,
                u16::from(self.sensors_ok),
            )
            .await;
        }
        if !self.apply_cycle_count(self.config.mag_cycle_count).await {
            return ack(link, seq, host_kind::CAL_SHOT, Status::SensorError, 0).await;
        }
        let quiet = self.config.quiet_enabled;
        let (input, acq, _temps) = self
            .take_shot(self.config.mag_samples, self.config.mag_cycle_count, quiet)
            .await;
        if acq.aborted {
            return ack(link, seq, host_kind::CAL_SHOT, Status::Aborted, 0).await;
        }
        // Every sensor must have delivered data: a calibration shot with a hole is useless.
        if input.mag.iter().any(|s| s.count == 0) || input.acc.iter().any(|s| s.count == 0) {
            return ack(link, seq, host_kind::CAL_SHOT, Status::SensorError, 0).await;
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
            return ack(link, seq, host_kind::CAL_SHOT, Status::InvalidArgument, 0).await;
        };
        let ev = quality::evaluate(&input, &self.cal, &self.config.thresholds, self.reference());
        let rep = CalShotReport {
            index: index as u8,
            count: self.shots.len() as u8,
            flags: ev.flags,
            shot,
        };
        link::send(link, &DeviceMessage::CalShotDone(rep), seq).await;
    }

    async fn cal_solve(&mut self, link: LinkId, seq: u8) {
        let opts = SolveOptions {
            mag_cycle_count: self.config.mag_cycle_count,
            ..SolveOptions::default()
        };
        let started = Instant::now();
        let mut job = match SolveJob::new(self.shots.as_slice(), opts) {
            Ok(j) => j,
            Err(e) => {
                return link::send(
                    link,
                    &DeviceMessage::CalSolved(CalSolved {
                        error: Some(e),
                        ..CalSolved::default()
                    }),
                    seq,
                )
                .await;
            }
        };
        let mut steps = 0u32;
        let result = loop {
            match job.step() {
                Progress::Running { .. } => {
                    steps += 1;
                    // Field-level borrows: `job` holds `self.shots`, so no `&mut self` methods here.
                    self.watchdog.pet();
                    if steps.is_multiple_of(4) && self.config.led_enabled {
                        board::led_set(&mut self.led, steps.is_multiple_of(8));
                    }
                    if matches!(poll_interrupt().await, Interrupt::Abort) {
                        break Err(attinav_core::solver::SolveError::NotConverged);
                    }
                    yield_now().await;
                }
                Progress::Finished(r) => break r,
            }
        };
        self.led(false);
        host_info!("solve: {} steps in {} ms", steps, started.elapsed().as_millis());
        let msg = match result {
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
        };
        link::send(link, &DeviceMessage::CalSolved(msg), seq).await;
    }

    async fn cal_store(&mut self, link: LinkId, seq: u8, id: u32) {
        let Some(set) = self.pending_set.take() else {
            return ack(link, seq, host_kind::CAL_STORE, Status::NoCalibration, 0).await;
        };
        if self.cal.mag_cycle_count != self.config.mag_cycle_count {
            // Sets at different cycle counts cannot be mixed: start a fresh calibration.
            self.cal = Calibration::empty(self.config.mag_cycle_count);
        }
        let idx = self.cal.upsert_set(set, 5.0);
        self.cal.id = id;
        self.cal.date_unix = self.unix_now();
        let status = self.persist().await;
        ack(link, seq, host_kind::CAL_STORE, status, idx as u16).await;
    }
}

/// `true` for every request kind the protocol defines (to tell `Unsupported` from `BadPayload`).
const fn host_kind_known(kind: u8) -> bool {
    #[allow(clippy::wildcard_imports, reason = "every kind constant is listed below")]
    use host_kind::*;
    matches!(
        kind,
        PING | GET_INFO
            | SET_TIME
            | RESET
            | SHOT
            | ABORT
            | STREAM
            | GET_CONFIG
            | SET_CONFIG
            | CAL_BEGIN
            | CAL_SHOT
            | CAL_REMOVE
            | CAL_SOLVE
            | CAL_STORE
            | CAL_LOAD
            | CAL_GET
            | CAL_SET
            | CAL_CLEAR
            | CAL_INFO
            | CAL_GET_SHOTS
            | CAL_END
            | FW_BEGIN
            | FW_CHUNK
            | FW_FINISH
            | FW_ABORT
    )
}
