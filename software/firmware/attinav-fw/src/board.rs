//! Board definition for Attinav Rev B: clock tree, interrupt bindings, pin map and the construction
//! of every peripheral driver. Nothing else in the firmware names a pin.
//!
//! Pin plan (hardware/kicad/ROADMAP.md, phase 4):
//!
//! | function                         | pins                                   |
//! |----------------------------------|----------------------------------------|
//! | SPI1 → 3× RM3100                 | PA5 SCK, PA7 MOSI, PA6 MISO            |
//! | CS_MAG1..3 / DRDY_MAG1..3        | PC6 PC7 PC8 / PC9 PA8 PC10 (EXTI 9 8 10)|
//! | AVDD_EN1..3 (TPS22917, high = on)| PC11 PC12 PD2                          |
//! | TEMP_MAG1..3 (Pt1000 dividers)   | PC0 PC1 PC2 = ADC1_IN1..3              |
//! | SPI2 → 2× SCA3300 + ICM-42688-P  | PB13 SCK, PB15 MOSI, PB14 MISO         |
//! | CS_ACC1 CS_ACC2 CS_IMU / IMU_INT | PB4 PB5 PB6 / PB7 (EXTI 7)             |
//! | LPUART1 to the carrier           | PA2 TX, PA3 RX                         |
//! | QUIET (open drain, active low)   | PA15                                   |
//! | USB FS                           | PA11 DM, PA12 DP                       |
//! | status LED                       | PA0                                    |

use embassy_executor::Spawner;
use embassy_stm32::adc::{Adc, AdcChannel, AdcConfig, Averaging, Resolution};
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::{Level, Output, OutputOpenDrain, Pull, Speed};
use embassy_stm32::mode::Async;
use embassy_stm32::spi::{self, Spi};
use embassy_stm32::time::Hertz;
use embassy_stm32::usart::{self, Uart};
use embassy_stm32::wdg::IndependentWatchdog;
use embassy_stm32::{Peripherals, bind_interrupts, dma, exti, interrupt, peripherals, usb};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Delay;
use static_cell::StaticCell;

use attinav_sensors::icm42688::Icm42688;
use attinav_sensors::rm3100::Rm3100;
use attinav_sensors::sca3300::Sca3300;

use crate::link;
use crate::sensors::{Accels, Mags, Sensors, Temps};

bind_interrupts!(pub struct Irqs {
    OTG_FS => usb::InterruptHandler<peripherals::USB_OTG_FS>;
    LPUART1 => usart::InterruptHandler<peripherals::LPUART1>;
    GPDMA1_CHANNEL0 => dma::InterruptHandler<peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => dma::InterruptHandler<peripherals::GPDMA1_CH1>;
    GPDMA1_CHANNEL2 => dma::InterruptHandler<peripherals::GPDMA1_CH2>;
    GPDMA1_CHANNEL3 => dma::InterruptHandler<peripherals::GPDMA1_CH3>;
    GPDMA1_CHANNEL4 => dma::InterruptHandler<peripherals::GPDMA1_CH4>;
    GPDMA1_CHANNEL5 => dma::InterruptHandler<peripherals::GPDMA1_CH5>;
    EXTI7 => exti::InterruptHandler<interrupt::typelevel::EXTI7>;
    EXTI8 => exti::InterruptHandler<interrupt::typelevel::EXTI8>;
    EXTI9 => exti::InterruptHandler<interrupt::typelevel::EXTI9>;
    EXTI10 => exti::InterruptHandler<interrupt::typelevel::EXTI10>;
});

/// A shared SPI bus.
pub type SpiBus = Mutex<NoopRawMutex, Spi<'static, Async, spi::mode::Master>>;
/// One chip-selected device on a shared bus.
pub type SpiDev = embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice<
    'static,
    NoopRawMutex,
    Spi<'static, Async, spi::mode::Master>,
    Output<'static>,
>;
/// The USB driver.
pub type UsbDrv = usb::Driver<'static, peripherals::USB_OTG_FS>;

/// Hardware revision reported in `Info`.
pub const HARDWARE_REV: u8 = 2;

/// Rev B drives the status LED from PA0 through a series resistor to ground: **high = on**.
const LED_ON: Level = Level::High;
/// `QUIET` is an open-drain output pulled up on the carrier; asserted = driven low.
pub const QUIET_ASSERTED: Level = Level::Low;

/// Clock tree: no crystal on the board. HSI (16 MHz) × 10 through PLL1 → 160 MHz system clock at
/// voltage range 1; HSI48 trimmed by the CRS from USB start-of-frame feeds the USB and is the
/// `ICLK` source. The ADC runs from the untouched 16 MHz HSI (its limit is 55 MHz).
pub fn clock_config() -> embassy_stm32::Config {
    #[allow(clippy::wildcard_imports, reason = "the RCC configuration names a dozen enums")]
    use embassy_stm32::rcc::*;
    let mut config = embassy_stm32::Config::default();
    config.rcc.hsi = true;
    config.rcc.pll1 = Some(Pll {
        source: PllSource::HSI,
        prediv: PllPreDiv::DIV1,
        mul: PllMul::MUL10,
        divp: None,
        divq: None,
        divr: Some(PllDiv::DIV1),
    });
    config.rcc.sys = Sysclk::PLL1_R;
    config.rcc.voltage_range = VoltageScale::RANGE1;
    config.rcc.hsi48 = Some(Hsi48Config { sync_from_usb: true });
    config.rcc.mux.iclksel = mux::Iclksel::HSI48;
    config.rcc.mux.adcdacsel = mux::Adcdacsel::HSI;
    config
}

/// Peripherals of the carrier UART, kept aside until the stored baud rate is known.
pub struct UartParts {
    lpuart: embassy_stm32::Peri<'static, peripherals::LPUART1>,
    rx: embassy_stm32::Peri<'static, peripherals::PA3>,
    tx: embassy_stm32::Peri<'static, peripherals::PA2>,
    tx_dma: embassy_stm32::Peri<'static, peripherals::GPDMA1_CH4>,
    rx_dma: embassy_stm32::Peri<'static, peripherals::GPDMA1_CH5>,
}

/// Builds the carrier UART at `baud` and spawns its task.
pub fn start_uart(parts: UartParts, baud: u32, spawner: Spawner) {
    let mut uart_cfg = usart::Config::default();
    uart_cfg.baudrate = baud;
    let uart = Uart::new(
        parts.lpuart,
        parts.rx,
        parts.tx,
        parts.tx_dma,
        parts.rx_dma,
        Irqs,
        uart_cfg,
    )
    .expect("LPUART1 configuration");
    link::spawn_uart(spawner, uart);
}

/// Everything `main` needs after bring-up.
pub struct Board {
    /// Flash peripheral for the state store and the firmware updater.
    pub flash: embassy_stm32::Peri<'static, peripherals::FLASH>,
    /// Carrier UART, started by `main` once the configuration is loaded.
    pub uart: UartParts,
    /// All sensors, powered and probed by [`Sensors::bring_up`] (called by the engine).
    pub sensors: Sensors,
    /// Status LED (`high` = on).
    pub led: Output<'static>,
    /// Independent watchdog, already running.
    pub watchdog: IndependentWatchdog<'static, peripherals::IWDG>,
}

/// Builds every driver and spawns the USB task. The sensors are configured by the engine; the UART
/// is started by `main` after the stored baud rate is known.
#[allow(
    clippy::items_after_statements,
    reason = "each static sits next to the driver it backs"
)]
pub fn init(p: Peripherals, spawner: Spawner) -> Board {
    // --- Watchdog: 16 s, petted by the engine loop (incl. inside the solver loop). ---
    let mut watchdog = IndependentWatchdog::new(p.IWDG, 16_000_000);
    watchdog.unleash();

    // --- SPI1: three RM3100 triads, ≤ 1 MHz, mode 0. ---
    static SPI1_BUS: StaticCell<SpiBus> = StaticCell::new();
    let mut spi1_cfg = spi::Config::default();
    spi1_cfg.frequency = Hertz(1_000_000);
    spi1_cfg.mode = spi::MODE_0;
    let spi1 = Spi::new(p.SPI1, p.PA5, p.PA7, p.PA6, p.GPDMA1_CH0, p.GPDMA1_CH1, Irqs, spi1_cfg);
    let spi1 = SPI1_BUS.init(Mutex::new(spi1));
    fn cs(pin: embassy_stm32::Peri<'static, embassy_stm32::gpio::AnyPin>) -> Output<'static> {
        Output::new(pin, Level::High, Speed::VeryHigh)
    }
    let mags = Mags::new(
        [
            Rm3100::new(SpiDev::new(spi1, cs(p.PC6.into()))),
            Rm3100::new(SpiDev::new(spi1, cs(p.PC7.into()))),
            Rm3100::new(SpiDev::new(spi1, cs(p.PC8.into()))),
        ],
        [
            ExtiInput::new(p.PC9, p.EXTI9, Pull::Down, Irqs),
            ExtiInput::new(p.PA8, p.EXTI8, Pull::Down, Irqs),
            ExtiInput::new(p.PC10, p.EXTI10, Pull::Down, Irqs),
        ],
        [
            Output::new(p.PC11, Level::Low, Speed::Low),
            Output::new(p.PC12, Level::Low, Speed::Low),
            Output::new(p.PD2, Level::Low, Speed::Low),
        ],
    );

    // --- SPI2: two SCA3300 and the IMU, 4 MHz, mode 0. ---
    static SPI2_BUS: StaticCell<SpiBus> = StaticCell::new();
    let mut spi2_cfg = spi::Config::default();
    spi2_cfg.frequency = Hertz(4_000_000);
    spi2_cfg.mode = spi::MODE_0;
    let spi2 = Spi::new(
        p.SPI2,
        p.PB13,
        p.PB15,
        p.PB14,
        p.GPDMA1_CH2,
        p.GPDMA1_CH3,
        Irqs,
        spi2_cfg,
    );
    let spi2 = SPI2_BUS.init(Mutex::new(spi2));
    let accels = Accels::new(
        [
            Sca3300::new(SpiDev::new(spi2, cs(p.PB4.into())), Delay),
            Sca3300::new(SpiDev::new(spi2, cs(p.PB5.into())), Delay),
        ],
        Icm42688::new(SpiDev::new(spi2, cs(p.PB6.into())), Delay),
        ExtiInput::new(p.PB7, p.EXTI7, Pull::None, Irqs),
    );

    // --- ADC1: Pt1000 dividers, 14 bit, 64× hardware averaging, VREFINT for the VDDA reading. ---
    let adc = Adc::new_with_config(
        p.ADC1,
        AdcConfig {
            resolution: Some(Resolution::BITS14),
            averaging: Some(Averaging::Samples64),
        },
    );
    let temps = Temps::new(adc, [p.PC0.degrade_adc(), p.PC1.degrade_adc(), p.PC2.degrade_adc()]);

    let quiet = OutputOpenDrain::new(p.PA15, Level::High, Speed::Low);
    let sensors = Sensors::new(mags, accels, temps, quiet);

    // --- Links. ---
    static EP_OUT: StaticCell<[u8; 256]> = StaticCell::new();
    let mut usb_cfg = usb::Config::default();
    // Bus powered through VBUS only; no VBUS sensing pin on the board.
    usb_cfg.vbus_detection = false;
    let usb_driver = usb::Driver::new_fs(p.USB_OTG_FS, Irqs, p.PA12, p.PA11, EP_OUT.init([0; 256]), usb_cfg);
    link::spawn_usb(spawner, usb_driver);

    let uart = UartParts {
        lpuart: p.LPUART1,
        rx: p.PA3,
        tx: p.PA2,
        tx_dma: p.GPDMA1_CH4,
        rx_dma: p.GPDMA1_CH5,
    };

    let led = Output::new(p.PA0, LED_ON, Speed::Low);

    Board {
        flash: p.FLASH,
        uart,
        sensors,
        led,
        watchdog,
    }
}

/// Turns the status LED on or off.
pub fn led_set(led: &mut Output<'static>, on: bool) {
    if on == (LED_ON == Level::High) {
        led.set_high();
    } else {
        led.set_low();
    }
}
