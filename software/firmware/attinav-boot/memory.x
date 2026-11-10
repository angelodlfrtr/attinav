/* Attinav Rev B flash map — STM32U575RG, 1 MiB in two 512 KiB banks of 8 KiB pages.
 *
 * Bank 1 (0x0800_0000):  bootloader | spare | ACTIVE application | spare
 * Bank 2 (0x0808_0000):  DFU staging (ACTIVE + one page) | calibration slots A/B | bootloader state | spare
 *
 * The application runs from bank 1 and every write (staged image, state slots, bootloader state)
 * goes to bank 2, so a flash operation never stalls the executing bank. The same numbers appear
 * in attinav-fw/memory.x; keep them in sync.
 */
MEMORY
{
  FLASH             : ORIGIN = 0x08000000, LENGTH = 32K    /* this bootloader           */
  ACTIVE            : ORIGIN = 0x0800A000, LENGTH = 456K   /* application (bank 1)      */
  DFU               : ORIGIN = 0x08080000, LENGTH = 464K   /* staged image (bank 2)     */
  BOOTLOADER_STATE  : ORIGIN = 0x080F8000, LENGTH = 8K     /* embassy-boot state page   */
  RAM         (rwx) : ORIGIN = 0x20000000, LENGTH = 192K   /* SRAM1 is plenty for a bootloader */
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(FLASH);
__bootloader_state_end   = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(FLASH);

__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(FLASH);
__bootloader_active_end   = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(FLASH);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(FLASH);
__bootloader_dfu_end   = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(FLASH);
