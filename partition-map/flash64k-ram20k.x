/* ---------------------------------------------------------------------------
 * Partition map for the CH32 parts with 64 KiB of application flash (USR_1)
 * and 20 KiB of SRAM: CH32V203C8T6, C8U6, F8P6, F8U6, G8R6, K8T6.
 *
 * Selected by the `build.rs` of both examples from the chip feature, `INCLUDE`d
 * by the
 * `memory.x` generated there, so the bootloader and the application can never
 * disagree about where the partitions are.
 *
 *  region            address            size    notes
 *  ----------------  -----------------  ------  --------------------------------
 *  BOOTLOADER        0x0800_0000        16 KiB  the bootloader itself
 *  ACTIVE            0x0800_4000        16 KiB  running application
 *  DFU               0x0800_8000        24 KiB  incoming image
 *  BOOTLOADER_STATE  0x0800_E000         8 KiB  embassy-boot state
 *
 * This is the tightest layout we support. embassy-boot sees a coarse erase
 * granularity of 8 KiB (see `CoarseFlash`) and requires
 *
 *   PAGE_SIZE                        = 8192
 *   ACTIVE  % PAGE_SIZE == 0         -> 16384 / 8192 = 2 blocks
 *   DFU     % PAGE_SIZE == 0         -> 24576 / 8192 = 3 blocks
 *   DFU - ACTIVE >= PAGE_SIZE        -> 8192 >= 8192 (the swap needs one spare)
 *   2 + 4 * (ACTIVE / PAGE_SIZE) = 10 <= STATE / WRITE_SIZE = 8192 / 256 = 32
 *
 * Parts with only 32 KiB of flash (CH32V203C6T6, F6P6, G6U6, K6T6) cannot hold
 * a bootloader plus two images plus state and are not supported.
 *
 * NOTE: ch32-metapac reports a much larger `FLASH_SIZE` for these chips because
 * it counts the extra on-die flash (`USR_2`) that can be traded against RAM
 * through the ROM/RAM split option bytes. Everything here stays inside the
 * nominal 64 KiB, which is always usable.
 * ------------------------------------------------------------------------- */

MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  BOOTLOADER       (rx)  : ORIGIN = 0x08000000, LENGTH = 16K
  ACTIVE           (rx)  : ORIGIN = 0x08004000, LENGTH = 16K
  DFU              (rx)  : ORIGIN = 0x08008000, LENGTH = 24K
  BOOTLOADER_STATE (rx)  : ORIGIN = 0x0800E000, LENGTH = 8K

  RAM             (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}

/* embassy-boot expects offsets from the start of the flash array, not bus
 * addresses, hence the `- ORIGIN(BOOTLOADER)` on every symbol. */
__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOTLOADER);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOTLOADER);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOTLOADER);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOTLOADER);

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
