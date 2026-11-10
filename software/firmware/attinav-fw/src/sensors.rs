//! Sensor set of the board: three RM3100 triads, two SCA3300 accelerometers, the IMU motion gate,
//! the Pt1000 coil thermometers and the `QUIET` line.
//!
//! This module owns the hardware access patterns (power sequencing, data-ready waits, pipelined
//! reads); the shot logic that decides *what* to acquire lives in the engine.

use defmt::{info, warn};
use embassy_futures::join::join;
use embassy_stm32::adc::{Adc, AnyAdcChannel, SampleTime, VrefInt};
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::{Output, OutputOpenDrain};
use embassy_stm32::mode::Async;
use embassy_stm32::peripherals::ADC1;
use embassy_time::{Delay, Duration, Timer, with_timeout};

use attinav_core::calibration::{ACC_COUNT, MAG_COUNT};
use attinav_core::math::Vec3;
use attinav_core::pt1000::{self, Divider};
use attinav_sensors::icm42688::{self, Icm42688};
use attinav_sensors::rm3100::Rm3100;
use attinav_sensors::sca3300::{self, Sca3300};

use crate::board::{QUIET_ASSERTED, SpiDev};

/// Longest wait for a magnetometer data-ready edge. A three-axis measurement at the highest
/// accepted cycle count (4000) takes ~400 ms.
const DRDY_TIMEOUT: Duration = Duration::from_millis(600);
/// VREFINT factory calibration (RM0456): 14-bit code at VDDA = 3.0 V, 30 °C.
const VREFINT_CAL_ADDR: usize = 0x0BFA_07A5;
const VREFINT_CAL_VDDA_MV: f64 = 3000.0;
const ADC_FULL_SCALE: u32 = (1 << 14) - 1;

/// Sensor identity bits for `Info::sensors_ok` ([`attinav_core::calibration::SensorId`] tags).
pub const fn sensor_bit(tag: u8) -> u8 {
    1 << tag
}

/// The three magnetometer triads.
pub struct Mags {
    dev: [Rm3100<SpiDev>; MAG_COUNT],
    drdy: [ExtiInput<'static, Async>; MAG_COUNT],
    power: [Output<'static>; MAG_COUNT],
    /// Triads that answered their identity check.
    pub ok: [bool; MAG_COUNT],
    /// Triads that failed during the current acquisition window (see [`Self::begin_window`]).
    fault: [bool; MAG_COUNT],
}

impl Mags {
    /// Wraps the drivers; the triads stay unpowered until [`Self::power_up`].
    pub const fn new(
        dev: [Rm3100<SpiDev>; MAG_COUNT],
        drdy: [ExtiInput<'static, Async>; MAG_COUNT],
        power: [Output<'static>; MAG_COUNT],
    ) -> Self {
        Self {
            dev,
            drdy,
            power,
            ok: [false; MAG_COUNT],
            fault: [false; MAG_COUNT],
        }
    }

    /// Opens an acquisition window (a shot, a stream): every probed triad gets a fresh chance.
    /// A triad that fails inside the window is skipped until the next window, so one dead triad
    /// costs one data-ready timeout per window instead of one per sample.
    pub const fn begin_window(&mut self) {
        self.fault = [false; MAG_COUNT];
    }

    /// Triads that failed in the current window.
    #[must_use]
    pub const fn faults(&self) -> [bool; MAG_COUNT] {
        self.fault
    }

    /// Triads usable right now (probed and not faulted in this window).
    #[must_use]
    pub fn active(&self) -> [bool; MAG_COUNT] {
        let mut a = [false; MAG_COUNT];
        for i in 0..MAG_COUNT {
            a[i] = self.ok[i] && !self.fault[i];
        }
        a
    }

    /// `true` when some working triad is not programmed with `cycle_count`.
    #[must_use]
    pub fn needs_cycle_count(&self, cycle_count: u16) -> bool {
        self.dev
            .iter()
            .zip(self.ok)
            .any(|(d, ok)| ok && d.cycle_count() != cycle_count)
    }

    /// Enables the three AVDD switches, probes the triads and sets the cycle count.
    pub async fn power_up(&mut self, cycle_count: u16) -> [bool; MAG_COUNT] {
        for p in &mut self.power {
            p.set_high();
        }
        // TPS22917 rise + RM3100 power-on reset.
        Timer::after_millis(20).await;
        for (i, dev) in self.dev.iter_mut().enumerate() {
            self.ok[i] = match dev.probe().await {
                Ok(_) => {
                    let _ = dev.stop().await;
                    dev.set_cycle_count(cycle_count).await.is_ok()
                }
                Err(e) => {
                    warn!("mag{}: probe failed {:?}", i + 1, e);
                    false
                }
            };
        }
        info!("mags ok: {:?}", self.ok);
        self.ok
    }

    /// Cuts AVDD to the triads (sleep).
    #[allow(dead_code, reason = "used by the planned low-power mode")]
    pub fn power_down(&mut self) {
        for p in &mut self.power {
            p.set_low();
        }
        self.ok = [false; MAG_COUNT];
    }

    /// Current cycle count (of the first working triad; all are set together).
    #[must_use]
    pub const fn cycle_count(&self) -> u16 {
        self.dev[0].cycle_count()
    }

    /// Sets the cycle count on every working triad.
    pub async fn set_cycle_count(&mut self, cycle_count: u16) -> bool {
        let mut all = true;
        for (i, dev) in self.dev.iter_mut().enumerate() {
            if self.ok[i] && dev.set_cycle_count(cycle_count).await.is_err() {
                warn!("mag{}: cycle count write failed", i + 1);
                all = false;
            }
        }
        all
    }

    /// Triggers one three-axis measurement on every active triad.
    pub async fn start_all(&mut self) {
        let active = self.active();
        for (i, dev) in self.dev.iter_mut().enumerate() {
            if active[i] && dev.start_single().await.is_err() {
                self.fault[i] = true;
                warn!("mag{}: poll write failed", i + 1);
            }
        }
    }

    /// Waits for every active triad's `DRDY`, then reads the three results.
    ///
    /// A triad that times out or fails its read returns `None` for this sample and is marked
    /// faulty for the rest of the window.
    pub async fn read_all(&mut self) -> [Option<Vec3>; MAG_COUNT] {
        let mut out = [None; MAG_COUNT];
        // Wait for all data-ready lines concurrently (they finish within microseconds of each
        // other when the cycle counts are equal).
        let active = self.active();
        let [d0, d1, d2] = &mut self.drdy;
        let (r0, (r1, r2)) = join(
            wait_drdy(active[0], d0),
            join(wait_drdy(active[1], d1), wait_drdy(active[2], d2)),
        )
        .await;
        let ready = [r0, r1, r2];
        for i in 0..MAG_COUNT {
            if !active[i] {
                continue;
            }
            if !ready[i] {
                warn!("mag{}: DRDY timeout", i + 1);
                self.fault[i] = true;
                continue;
            }
            match self.dev[i].read_measurement().await {
                Ok(m) => out[i] = Some(Vec3::new(f64::from(m.x), f64::from(m.y), f64::from(m.z))),
                Err(e) => {
                    warn!("mag{}: read failed {:?}", i + 1, e);
                    self.fault[i] = true;
                }
            }
        }
        out
    }

    /// One complete polled sample of all triads.
    pub async fn sample(&mut self) -> [Option<Vec3>; MAG_COUNT] {
        self.start_all().await;
        self.read_all().await
    }
}

/// One IMU reading reduced to what the engine needs.
#[derive(Clone, Copy, Debug, Default, defmt::Format)]
pub struct ImuReading {
    /// Gyro rate magnitude, °/s.
    pub rate_dps: f32,
    /// Raw gyro counts.
    pub gyro: [i16; 3],
    /// Raw accelerometer counts.
    pub accel: [i16; 3],
    /// Die temperature, °C.
    pub temp_c: f32,
}

/// The two accelerometers and the IMU.
pub struct Accels {
    acc: [Sca3300<SpiDev, Delay>; ACC_COUNT],
    imu: Icm42688<SpiDev, Delay>,
    #[allow(dead_code, reason = "owned so the pin stays configured; interrupts are not used yet")]
    imu_int: ExtiInput<'static, Async>,
    /// Accelerometers that passed start-up.
    pub ok: [bool; ACC_COUNT],
    /// IMU answered its identity check.
    pub imu_ok: bool,
}

impl Accels {
    /// Wraps the drivers.
    pub const fn new(
        acc: [Sca3300<SpiDev, Delay>; ACC_COUNT],
        imu: Icm42688<SpiDev, Delay>,
        imu_int: ExtiInput<'static, Async>,
    ) -> Self {
        Self {
            acc,
            imu,
            imu_int,
            ok: [false; ACC_COUNT],
            imu_ok: false,
        }
    }

    /// Runs the SCA3300 start-up sequence (mode 4) and the IMU reset. Takes ≈ 250 ms.
    pub async fn init(&mut self) -> ([bool; ACC_COUNT], bool) {
        for (i, a) in self.acc.iter_mut().enumerate() {
            self.ok[i] = match a.init(sca3300::Mode::Mode4).await {
                Ok(()) => true,
                Err(e) => {
                    warn!("acc{}: init failed {:?}", i + 1, e);
                    false
                }
            };
        }
        self.imu_ok = match self.imu.init(icm42688::Config::default()).await {
            Ok(()) => true,
            Err(e) => {
                warn!("imu: init failed {:?}", e);
                false
            }
        };
        info!("accels ok: {:?}, imu ok: {}", self.ok, self.imu_ok);
        (self.ok, self.imu_ok)
    }

    /// Reads both accelerometers: raw counts and die temperature (°C).
    pub async fn read(&mut self) -> [Option<(Vec3, f32)>; ACC_COUNT] {
        let mut out = [None; ACC_COUNT];
        for i in 0..ACC_COUNT {
            if !self.ok[i] {
                continue;
            }
            match self.acc[i].read().await {
                Ok(s) => {
                    out[i] = Some((
                        Vec3::new(f64::from(s.x), f64::from(s.y), f64::from(s.z)),
                        sca3300::temp_celsius(s.temp),
                    ));
                }
                Err(e) => warn!("acc{}: read failed {:?}", i + 1, e),
            }
        }
        out
    }

    /// Reads the IMU.
    pub async fn read_imu(&mut self) -> Option<ImuReading> {
        if !self.imu_ok {
            return None;
        }
        match self.imu.read().await {
            Ok(s) => Some(ImuReading {
                rate_dps: s.gyro_rate_dps(self.imu.config().gyro_scale),
                gyro: s.gyro,
                accel: s.accel,
                temp_c: s.temp_celsius(),
            }),
            Err(e) => {
                warn!("imu: read failed {:?}", e);
                None
            }
        }
    }
}

/// Coil thermometers.
pub struct Temps {
    adc: Adc<'static, ADC1>,
    pins: [AnyAdcChannel<'static, ADC1>; MAG_COUNT],
    vref: VrefInt,
    vrefint_cal: u16,
}

/// One temperature reading.
#[derive(Clone, Copy, Debug, Default, defmt::Format)]
pub struct TempReading {
    /// Coil temperatures, °C; `None` when the divider reads open or shorted.
    pub mag_c: [Option<f64>; MAG_COUNT],
    /// Analogue supply from VREFINT, mV.
    pub vdda_mv: u16,
}

impl Temps {
    /// Takes the configured ADC and the three divider pins.
    pub fn new(adc: Adc<'static, ADC1>, pins: [AnyAdcChannel<'static, ADC1>; MAG_COUNT]) -> Self {
        let vref = adc.enable_vrefint();
        // SAFETY: fixed system-memory address of the factory VREFINT calibration (RM0456).
        let vrefint_cal = unsafe { core::ptr::read_volatile(VREFINT_CAL_ADDR as *const u16) };
        Self {
            adc,
            pins,
            vref,
            vrefint_cal,
        }
    }

    /// Reads VDDA and the three coils. Blocking for a few milliseconds (hardware averaging).
    pub fn read(&mut self) -> TempReading {
        let vref_raw = u32::from(self.adc.blocking_read(&mut self.vref, SampleTime::CYCLES160_5));
        let vdda_v = if vref_raw == 0 {
            3.3
        } else {
            VREFINT_CAL_VDDA_MV * f64::from(self.vrefint_cal) / f64::from(vref_raw) / 1000.0
        };
        let mut mag_c = [None; MAG_COUNT];
        for (i, pin) in self.pins.iter_mut().enumerate() {
            let raw = u32::from(self.adc.blocking_read(pin, SampleTime::CYCLES160_5));
            mag_c[i] = pt1000::adc_to_celsius(raw, ADC_FULL_SCALE, vdda_v, &Divider::REV_B);
        }
        TempReading {
            mag_c,
            vdda_mv: (vdda_v * 1000.0) as u16,
        }
    }
}

/// All sensors plus the `QUIET` line.
pub struct Sensors {
    /// Magnetometer triads.
    pub mags: Mags,
    /// Accelerometers and IMU.
    pub accels: Accels,
    /// Coil thermometers.
    pub temps: Temps,
    quiet: OutputOpenDrain<'static>,
}

impl Sensors {
    /// Bundles the parts.
    pub const fn new(mags: Mags, accels: Accels, temps: Temps, quiet: OutputOpenDrain<'static>) -> Self {
        Self {
            mags,
            accels,
            temps,
            quiet,
        }
    }

    /// Powers and probes everything; returns the `Info::sensors_ok` bit mask.
    pub async fn bring_up(&mut self, cycle_count: u16) -> u8 {
        let mags = self.mags.power_up(cycle_count).await;
        let (accs, imu) = self.accels.init().await;
        let mut mask = 0;
        for (i, ok) in mags.iter().enumerate() {
            if *ok {
                mask |= sensor_bit(i as u8);
            }
        }
        for (i, ok) in accs.iter().enumerate() {
            if *ok {
                mask |= sensor_bit(3 + i as u8);
            }
        }
        if imu {
            mask |= sensor_bit(5);
        }
        mask
    }

    /// Drives `QUIET`.
    pub fn set_quiet(&mut self, asserted: bool) {
        if asserted == (QUIET_ASSERTED == embassy_stm32::gpio::Level::Low) {
            self.quiet.set_low();
        } else {
            self.quiet.set_high();
        }
    }
}

/// Waits for one data-ready line (skipped for a faulty triad); `true` when it rose in time.
async fn wait_drdy(ok: bool, drdy: &mut ExtiInput<'static, Async>) -> bool {
    if !ok {
        return false;
    }
    with_timeout(DRDY_TIMEOUT, drdy.wait_for_high()).await.is_ok()
}
