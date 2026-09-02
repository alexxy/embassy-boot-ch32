/* ---------------------------------------------------------------------------
 * Partition map for CH32V305RBT6 (128 KiB internal flash @ 0x0800_0000,
 * 32 KiB SRAM @ 0x2000_0000).
 *
 * This file is included by `memory/memory.x` of both the bootloader and the
 * application example (via `cargo:rustc-link-search`), so that the two binaries
 * can never disagree about where the partitions are.
 *
 *  region            address            size    notes
 *  ----------------  -----------------  ------  --------------------------------
 *  BOOTLOADER        0x0800_0000        16 KiB  the bootloader itself
 *  ACTIVE            0x0800_4000        48 KiB  running application
 *  DFU               0x0801_0000        56 KiB  incoming image
 *  BOOTLOADER_STATE  0x0801_E000         8 KiB  embassy-boot state
 *
 * The erase granularity used by embassy-boot is 8 KiB (see `CoarseFlash`),
 * which gives the following requirements:
 *
 *   PAGE_SIZE                        = 8192
 *   ACTIVE  % PAGE_SIZE == 0         -> 49152 / 8192 = 6 blocks
 *   DFU     % PAGE_SIZE == 0         -> 57344 / 8192 = 7 blocks
 *   DFU - ACTIVE >= PAGE_SIZE        -> 8192 >= 8192
 *   2 + 4 * (ACTIVE / PAGE_SIZE) = 26 <= STATE / WRITE_SIZE = 8192 / 256 = 32
 *
 * NOTE: ch32-metapac reports `FLASH_SIZE = 480 KiB` for this chip (the size of
 * the largest member of the family) while CH32V305RBT6 only has 128 KiB. The
 * flash driver accepts accesses up to 480 KiB without complaining, so keeping
 * the partitions inside 0x0800_0000..0x0802_0000 is up to us.
 * ------------------------------------------------------------------------- */

MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  BOOTLOADER       (rx)  : ORIGIN = 0x08000000, LENGTH = 16K
  ACTIVE           (rx)  : ORIGIN = 0x08004000, LENGTH = 48K
  DFU              (rx)  : ORIGIN = 0x08010000, LENGTH = 56K
  BOOTLOADER_STATE (rx)  : ORIGIN = 0x0801E000, LENGTH = 8K

  RAM             (rwx) : ORIGIN = 0x20000000, LENGTH = 32K
}

/* embassy-boot expects offsets from the start of the flash array, not bus
 * addresses, hence the `- ORIGIN(BOOTLOADER)` on every symbol. */
__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOTLOADER);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOTLOADER);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOTLOADER);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOTLOADER);

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
