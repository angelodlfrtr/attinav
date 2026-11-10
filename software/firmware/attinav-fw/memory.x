/* Attinav Rev B application memory map — must agree with attinav-boot/memory.x.
 *
 * STM32U575RG: 1 MiB flash in two 512 KiB banks of 8 KiB pages, 768 KiB SRAM (SRAM1..3 contiguous).
 *
 *   0x0800_0000  BOOTLOADER        32 K   embassy-boot
 *   0x0800_8000  (spare)            8 K
 *   0x0800_A000  FLASH (ACTIVE)   456 K   this application
 *   0x0807_C000  (spare)           16 K
 *   0x0808_0000  DFU              464 K   staged update image (bank 2)
 *   0x080F_4000  CAL_A              8 K   non-volatile state slot A (config + calibration)
 *   0x080F_6000  CAL_B              8 K   non-volatile state slot B
 *   0x080F_8000  BOOTLOADER_STATE   8 K   swap / trial-boot state page
 *   0x080F_A000  (spare)           24 K
 *
 * Everything the application writes (DFU, state slots, bootloader state) is in bank 2, so no
 * erase or program ever stalls the bank the code executes from.
 *
 * RAM is limited to SRAM1 + SRAM2 (256 K): SRAM3 can be powered off in Stop modes and is not
 * needed. flip-link places the stack below .data/.bss so a stack overflow faults instead of
 * corrupting state.
 */
MEMORY
{
  BOOTLOADER        : ORIGIN = 0x08000000, LENGTH = 32K
  FLASH             : ORIGIN = 0x0800A000, LENGTH = 456K
  DFU               : ORIGIN = 0x08080000, LENGTH = 464K
  CAL_A             : ORIGIN = 0x080F4000, LENGTH = 8K
  CAL_B             : ORIGIN = 0x080F6000, LENGTH = 8K
  BOOTLOADER_STATE  : ORIGIN = 0x080F8000, LENGTH = 8K
  RAM         (rwx) : ORIGIN = 0x20000000, LENGTH = 256K
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
__bootloader_state_end   = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOTLOADER);
__bootloader_dfu_end   = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOTLOADER);

/* Exposed to the firmware so the flash map has a single source of truth. */
__attinav_cal_a_start = ORIGIN(CAL_A) - ORIGIN(BOOTLOADER);
__attinav_cal_b_start = ORIGIN(CAL_B) - ORIGIN(BOOTLOADER);
__attinav_cal_slot_len = LENGTH(CAL_A);
__attinav_active_len   = LENGTH(FLASH);
