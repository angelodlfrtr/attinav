//! Non-volatile state and firmware update staging, both on the internal flash behind one mutex.
//!
//! * **State record** (configuration + calibration): two alternating 8 KiB slots at the end of
//!   bank 2 (`CAL_A`, `CAL_B` in `memory.x`), framed by [`attinav_core::store`] so a reset during
//!   a write never loses the previous record.
//! * **Firmware updates**: chunks are written to the DFU partition through `embassy-boot`'s
//!   [`FirmwareUpdater`]; after the CRC check the image is marked for swap and the MCU resets.

use defmt::{info, warn};
use embassy_boot::FirmwareUpdaterError;
use embassy_boot_stm32::{AlignedBuffer, FirmwareUpdater, FirmwareUpdaterConfig, State};
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_embedded_hal::flash::partition::Partition;
use embassy_futures::yield_now;
use embassy_stm32::flash::{Blocking, Flash, WRITE_SIZE};

use embassy_stm32::peripherals::FLASH;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_storage_async::nor_flash::{NorFlash, ReadNorFlash};
use static_cell::StaticCell;

use attinav_core::calibration::{CALIBRATION_MAX_BYTES, Calibration};
use attinav_core::crc::Crc32;
use attinav_core::protocol::{DeviceConfig, FW_CHUNK_MAX, FwBegin, FwChunk};
use attinav_core::store::{HEADER_LEN, Slot, decode_record, encode_record, newer};
use attinav_core::wire::{Reader, Writer};

use crate::ecc;

/// The flash behind an async mutex.
pub type FlashDev = BlockingAsync<Flash<'static, Blocking>>;
/// Shared flash handle.
pub type FlashMutex = Mutex<NoopRawMutex, FlashDev>;

unsafe extern "C" {
    static __attinav_cal_a_start: u32;
    static __attinav_cal_b_start: u32;
    static __attinav_cal_slot_len: u32;
    static __attinav_active_len: u32;
}

/// Reads a linker-script symbol's *address* as a number (the usual trick for layout constants).
fn linker_value(sym: &u32) -> u32 {
    core::ptr::from_ref(sym) as u32
}

/// Flash-base-relative offset of a state slot.
fn slot_offset(slot: Slot) -> u32 {
    // SAFETY: linker-provided symbols; only their addresses are used.
    unsafe {
        match slot {
            Slot::A => linker_value(&__attinav_cal_a_start),
            Slot::B => linker_value(&__attinav_cal_b_start),
        }
    }
}

fn slot_len() -> u32 {
    // SAFETY: linker-provided symbol; only its address is used.
    unsafe { linker_value(&__attinav_cal_slot_len) }
}

/// Size of the active partition = largest accepted firmware image.
pub fn active_partition_len() -> u32 {
    // SAFETY: linker-provided symbol; only its address is used.
    unsafe { linker_value(&__attinav_active_len) }
}

/// Wraps the flash peripheral once.
pub fn init_flash(p: embassy_stm32::Peri<'static, FLASH>) -> &'static FlashMutex {
    static FLASH_CELL: StaticCell<FlashMutex> = StaticCell::new();
    FLASH_CELL.init(Mutex::new(BlockingAsync::new(Flash::new_blocking(p))))
}

/// Errors of the state store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum StoreError {
    /// Erase, write or read failed.
    Flash,
    /// The record did not fit the slot or did not serialise.
    Encode,
    /// Read-back after write did not match.
    Verify,
}

/// Persistent configuration + calibration.
pub struct Store {
    flash: &'static FlashMutex,
    seq: u32,
    current: Option<Slot>,
    /// Scratch for one encoded record (static: too large for the stack).
    buf: &'static mut [u8; SLOT_BUF],
    payload: &'static mut [u8; STATE_MAX],
    /// Read-back of both slots for [`Self::load`].
    slots: &'static mut [[u8; SLOT_BUF]; 2],
}

/// Payload bound: config + calibration.
const STATE_MAX: usize = DeviceConfig::BYTES + CALIBRATION_MAX_BYTES;
/// Slot buffer: header + payload + write-size padding.
const SLOT_BUF: usize = HEADER_LEN + STATE_MAX + WRITE_SIZE;

impl Store {
    /// Binds to the shared flash. Call [`Self::load`] before anything else. Call once.
    pub fn new(flash: &'static FlashMutex) -> Self {
        static BUF: StaticCell<[u8; SLOT_BUF]> = StaticCell::new();
        static PAYLOAD: StaticCell<[u8; STATE_MAX]> = StaticCell::new();
        static SLOTS: StaticCell<[[u8; SLOT_BUF]; 2]> = StaticCell::new();
        Self {
            flash,
            seq: 0,
            current: None,
            buf: BUF.init([0; SLOT_BUF]),
            payload: PAYLOAD.init([0; STATE_MAX]),
            slots: SLOTS.init([[0; SLOT_BUF]; 2]),
        }
    }

    /// Loads the newest valid record. Returns `None` when no slot holds a valid record (first boot
    /// or erased flash); defaults then apply. May be called again at any time (`CalLoad`).
    pub async fn load(&mut self) -> Option<(DeviceConfig, Calibration)> {
        let bufs = &mut *self.slots;
        let mut infos = [None, None];
        let mut flash = self.flash.lock().await;
        let _ = ecc::take_fault();
        for (i, slot) in [Slot::A, Slot::B].into_iter().enumerate() {
            if flash.read(slot_offset(slot), &mut bufs[i]).await.is_err() {
                warn!("store: read of slot {:?} failed", slot);
                continue;
            }
            if ecc::take_fault() {
                // Torn write: the slot holds a half-programmed quad-word. It is the older slot
                // (the newer one was written completely) and the next `save` erases it.
                warn!("store: ECC fault reading slot {:?}, ignoring it", slot);
                continue;
            }
            infos[i] = decode_record(&bufs[i]).map(|(info, _)| info);
        }
        // Decoding below needs no flash access; release it for the link tasks right away.
        drop(flash);
        let slot = newer(infos[0], infos[1])?;
        let idx = usize::from(slot == Slot::B);
        let (info, payload) = decode_record(&bufs[idx])?;
        let mut r = Reader::new(payload);
        let config = DeviceConfig::read(&mut r).ok()?;
        let cal = Calibration::read(&mut r).ok()?;
        self.seq = info.seq;
        self.current = Some(slot);
        info!(
            "store: loaded record seq {} from slot {:?} (cal id {}, {} sets)",
            info.seq, slot, cal.id, cal.n_sets
        );
        Some((config, cal))
    }

    /// Writes a new record into the other slot and verifies it.
    pub async fn save(&mut self, config: &DeviceConfig, cal: &Calibration) -> Result<(), StoreError> {
        let Self { buf, payload, .. } = self;
        let mut w = Writer::new(&mut payload[..]);
        config.write(&mut w).map_err(|_| StoreError::Encode)?;
        cal.write(&mut w).map_err(|_| StoreError::Encode)?;
        let n = w.len();
        let seq = self.seq.wrapping_add(1);
        let len = encode_record(seq, &payload[..n], WRITE_SIZE, &mut buf[..]).map_err(|_| StoreError::Encode)?;
        if len as u32 > slot_len() {
            return Err(StoreError::Encode);
        }
        let target = self.current.map_or(Slot::A, Slot::other);
        let off = slot_offset(target);
        let mut flash = self.flash.lock().await;
        flash
            .erase(off, off + slot_len())
            .await
            .map_err(|_| StoreError::Flash)?;
        flash.write(off, &buf[..len]).await.map_err(|_| StoreError::Flash)?;
        // Verify.
        let mut back = [0u8; 64];
        let mut pos = 0usize;
        while pos < len {
            let n = (len - pos).min(back.len());
            flash
                .read(off + pos as u32, &mut back[..n])
                .await
                .map_err(|_| StoreError::Flash)?;
            if back[..n] != buf[pos..pos + n] {
                return Err(StoreError::Verify);
            }
            pos += n;
        }
        drop(flash);
        self.seq = seq;
        self.current = Some(target);
        info!("store: saved record seq {} to slot {:?} ({} bytes)", seq, target, len);
        Ok(())
    }
}

// --------------------------------------------------------------------------------------------
// Firmware update
// --------------------------------------------------------------------------------------------

/// Progress of an update session.
#[derive(Clone, Copy, Debug, defmt::Format)]
struct Session {
    size: u32,
    crc32: u32,
    received: u32,
}

/// Errors of the update path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum UpdateError {
    /// No `FwBegin` received, or chunk out of order / misaligned / past the end.
    Protocol,
    /// Image larger than the active partition, or zero.
    Size,
    /// Flash operation failed, or the bootloader state does not allow updates.
    Flash,
    /// The staged image's CRC does not match `FwBegin`.
    Crc,
}

type Updater =
    FirmwareUpdater<'static, Partition<'static, NoopRawMutex, FlashDev>, Partition<'static, NoopRawMutex, FlashDev>>;

/// Stages firmware images for the bootloader.
pub struct FwUpdate {
    updater: Updater,
    /// Second handle on the DFU partition, for the erase in [`Self::begin`].
    dfu: Partition<'static, NoopRawMutex, FlashDev>,
    session: Option<Session>,
}

/// Flash page (erase unit) of the STM32U5, bytes.
const PAGE_SIZE: u32 = 8192;

impl FwUpdate {
    /// Binds to the DFU and state partitions described by the linker script.
    pub fn new(flash: &'static FlashMutex) -> Self {
        static ALIGNED: StaticCell<AlignedBuffer<WRITE_SIZE>> = StaticCell::new();
        let config = FirmwareUpdaterConfig::from_linkerfile(flash, flash);
        let dfu = config.dfu.clone();
        let aligned = ALIGNED.init(AlignedBuffer([0; WRITE_SIZE]));
        Self {
            updater: FirmwareUpdater::new(config, &mut aligned.0),
            dfu,
            session: None,
        }
    }

    /// Bootloader state as an `Info::boot_state` code.
    pub async fn boot_state(&mut self) -> u8 {
        match self.updater.get_state().await {
            Ok(State::Boot) => 0,
            Ok(State::Swap) => 1,
            Ok(State::Revert) => 2,
            Ok(State::DfuDetach) => 3,
            Err(_) => 255,
        }
    }

    /// Confirms the running image so the bootloader keeps it. Idempotent.
    pub async fn mark_booted(&mut self) {
        match self.updater.get_state().await {
            Ok(State::Swap) => match self.updater.mark_booted().await {
                Ok(()) => info!("fw: trial image confirmed"),
                Err(e) => warn!("fw: mark_booted failed: {:?}", defmt::Debug2Format(&e)),
            },
            Ok(s) => info!("fw: boot state {:?}", defmt::Debug2Format(&s)),
            Err(e) => warn!("fw: cannot read boot state: {:?}", defmt::Debug2Format(&e)),
        }
    }

    /// Starts a session. Erases the first DFU page now; the others are erased lazily as chunks
    /// arrive.
    ///
    /// The explicit erase matters for a retry: `embassy-boot` remembers the last page it erased,
    /// and a session that was abandoned inside page 0 would otherwise make the next session
    /// program a non-erased page (a flash error on every retry until reset).
    pub async fn begin(&mut self, b: &FwBegin) -> Result<(), UpdateError> {
        if b.size == 0 || b.size > active_partition_len() {
            return Err(UpdateError::Size);
        }
        self.session = None;
        self.dfu.erase(0, PAGE_SIZE).await.map_err(|_| UpdateError::Flash)?;
        self.session = Some(Session {
            size: b.size,
            crc32: b.crc32,
            received: 0,
        });
        info!(
            "fw: update begin, {} bytes, crc {:#010x}, version {:?}",
            b.size, b.crc32, b.version
        );
        Ok(())
    }

    /// Writes one chunk; chunks must arrive in order and aligned to the flash write size.
    pub async fn chunk(&mut self, c: &FwChunk) -> Result<u32, UpdateError> {
        let Some(s) = self.session.as_mut() else {
            return Err(UpdateError::Protocol);
        };
        let data = c.data();
        if c.offset != s.received || data.is_empty() || !c.offset.is_multiple_of(WRITE_SIZE as u32) {
            return Err(UpdateError::Protocol);
        }
        let end = c.offset + data.len() as u32;
        if end > s.size {
            return Err(UpdateError::Protocol);
        }
        // Pad the final partial chunk with 0xFF to the write granularity.
        let mut padded = [0xFFu8; FW_CHUNK_MAX];
        padded[..data.len()].copy_from_slice(data);
        let len = data.len().div_ceil(WRITE_SIZE) * WRITE_SIZE;
        self.updater
            .write_firmware(c.offset as usize, &padded[..len])
            .await
            .map_err(|e| map_updater_error(&e))?;
        s.received = end;
        Ok(end)
    }

    /// Verifies the staged image against the announced CRC and marks it for swap at next reset.
    pub async fn finish(&mut self) -> Result<(), UpdateError> {
        let Some(s) = self.session.take() else {
            return Err(UpdateError::Protocol);
        };
        if s.received != s.size {
            return Err(UpdateError::Protocol);
        }
        // CRC over the staged bytes, read back from flash in 1 KiB pieces, yielding in between.
        let mut crc = Crc32::new();
        let mut buf = [0u8; 1024];
        let mut off = 0u32;
        let _ = ecc::take_fault();
        while off < s.size {
            let n = ((s.size - off) as usize).min(buf.len());
            self.updater
                .read_dfu(off, &mut buf[..n])
                .await
                .map_err(|e| map_updater_error(&e))?;
            crc.update(&buf[..n]);
            off += n as u32;
            yield_now().await;
        }
        let crc = crc.finish();
        if ecc::take_fault() {
            warn!("fw: ECC fault while reading back the staged image");
            return Err(UpdateError::Crc);
        }
        if crc != s.crc32 {
            warn!("fw: crc mismatch {:#010x} != {:#010x}", crc, s.crc32);
            return Err(UpdateError::Crc);
        }
        self.updater.mark_updated().await.map_err(|e| map_updater_error(&e))?;
        info!("fw: image verified and marked for swap");
        Ok(())
    }

    /// Drops the session (staged bytes are simply overwritten by the next one).
    pub fn abort(&mut self) {
        if self.session.take().is_some() {
            info!("fw: update aborted");
        }
    }

    /// `true` while a session is open.
    #[must_use]
    pub const fn in_progress(&self) -> bool {
        self.session.is_some()
    }
}

/// Every updater failure (flash I/O, bootloader state, signature) is a "flash" error for the
/// host: the staged image cannot be trusted and the session has to start over.
const fn map_updater_error(_: &FirmwareUpdaterError) -> UpdateError {
    UpdateError::Flash
}
