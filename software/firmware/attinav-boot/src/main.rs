//! Attinav bootloader.
//!
//! A thin wrapper around `embassy-boot`: on every reset it inspects the state page, swaps a staged
//! image from the DFU partition into the active partition when the application asked for it
//! (`mark_updated`), reverts to the previous image when a trial boot never confirmed itself
//! (`mark_booted` was not called before the next reset), then jumps to the active partition.
//!
//! The partition map lives in `memory.x`. The whole flash is handed to `embassy-boot` as one
//! `NorFlash` so the linker-script offsets (relative to the flash base) apply directly even though
//! the DFU partition sits in the second bank.
#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_boot_stm32::{BootLoader, BootLoaderConfig};
use embassy_stm32::flash::{FLASH_BASE, Flash};
use embassy_sync::blocking_mutex::Mutex;

/// Page buffer for the swap: one flash page (both partitions use 8 KiB pages).
const PAGE_SIZE: usize = 8192;

#[entry]
fn main() -> ! {
    let p = embassy_stm32::init(embassy_stm32::Config::default());

    // Rollback protection starts here, not in the application: a trial image that faults before
    // it configures its own watchdog would otherwise hang until a power cycle. 32 s covers the
    // swap of a full partition (about two seconds) with a wide margin; the application re-arms
    // the same watchdog at 16 s once its sensors are up.
    let mut watchdog = embassy_stm32::wdg::IndependentWatchdog::new(p.IWDG, 32_000_000);
    watchdog.unleash();

    let flash = Flash::new_blocking(p.FLASH);
    let flash = Mutex::new(RefCell::new(flash));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();
    let bl = BootLoader::prepare::<_, _, _, PAGE_SIZE>(config);

    #[cfg(feature = "defmt")]
    defmt::info!(
        "boot state {:?}, jumping to {:#x}",
        bl.state,
        FLASH_BASE as u32 + active_offset
    );

    // SAFETY: the active partition holds a cortex-m-rt image linked at exactly this address
    // (attinav-fw/memory.x); `load` sets VTOR and the stack pointer from its vector table.
    unsafe { bl.load(FLASH_BASE as u32 + active_offset) }
}

/// A flash ECC double error (a quad-word whose programming was cut short by a reset) raises an
/// NMI while the read still returns data. Clear the flag and carry on: embassy-boot validates the
/// state page by its magic bytes, so garbage there means "no update in progress" rather than a
/// reset loop.
#[exception]
unsafe fn NonMaskableInt() {
    let eccr = embassy_stm32::pac::FLASH.eccr().read();
    if eccr.eccd() || eccr.eccc() {
        embassy_stm32::pac::FLASH.eccr().write(|w| {
            w.set_eccd(true);
            w.set_eccc(true);
        });
    }
}

/// A hard fault in the bootloader (corrupt flash, broken swap) resets the MCU: embassy-boot's
/// state machine is idempotent, so retrying is the right move.
#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    cortex_m::peripheral::SCB::sys_reset();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::peripheral::SCB::sys_reset();
}
