//! Flash ECC fault handling.
//!
//! The STM32U5 flash is ECC-protected per quad-word. A read of a quad-word whose programming was
//! interrupted (reset or power loss mid-write) does not return a bus error: the garbage data is
//! delivered *and* a non-maskable interrupt fires with `ECCD` set. Without a handler the NMI ends
//! in the default handler and the MCU would reset on every boot that reads the torn slot.
//!
//! The handler here clears the flag and records the fault; code that reads flash it did not write
//! itself (the state store, the staged-image verification) calls [`take_fault`] around the read
//! and treats the data as invalid when a fault happened.

use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m_rt::exception;
use embassy_stm32::pac;

static FAULT: AtomicBool = AtomicBool::new(false);

#[exception]
unsafe fn NonMaskableInt() {
    let eccr = pac::FLASH.eccr().read();
    if eccr.eccd() || eccr.eccc() {
        // Both flags are "write 1 to clear"; the NMI line stays asserted until ECCD is cleared.
        pac::FLASH.eccr().write(|w| {
            w.set_eccd(true);
            w.set_eccc(true);
        });
        FAULT.store(true, Ordering::SeqCst);
    }
    // No other NMI source is enabled (no clock security system, no SRAM parity).
}

/// Returns `true` when an ECC fault happened since the previous call, and forgets it.
pub fn take_fault() -> bool {
    FAULT.swap(false, Ordering::SeqCst)
}
