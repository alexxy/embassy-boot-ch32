/* ---------------------------------------------------------------------------
 * Partition map for the CH32 parts with 256 KiB of application flash (USR_1)
 * and 64 KiB of SRAM: CH32V303RCT6, CH32V303VCT6, CH32V307RCT6, CH32V307VCT6,
 * CH32V307WCU6.
 *
 * Selected by the `build.rs` of both examples from the chip feature, `INCLUDE`d
 * by the
 * `memory.x` generated there, so the bootloader and the application can never
 * disagree about where the partitions are.
 *
 *  region            address            size    notes
 *  ----------------  -----------------  ------  --------------------------------
 *  BOOTLOADER        0x0800_0000        16 KiB  the bootloader itself
 *  ACTIVE            0x0800_4000       104 KiB  running application
 *  DFU               0x0801_E000       120 KiB  incoming image
 *  BOOTLOADER_STATE  0x0803_C000        16 KiB  embassy-boot state
 *
 * The erase granularity used by embassy-boot is 8 KiB (see `CoarseFlash`),
 * which gives the following requirements:
 *
 *   PAGE_SIZE                        = 8192
 *   ACTIVE  % PAGE_SIZE == 0         -> 106496 / 8192 = 13 blocks
 *   DFU     % PAGE_SIZE == 0         -> 122880 / 8192 = 15 blocks
 *   DFU - ACTIVE >= PAGE_SIZE        -> 16384 >= 8192 (the swap needs one spare)
 *   2 + 4 * (ACTIVE / PAGE_SIZE) = 54 <= STATE / WRITE_SIZE = 16384 / 256 = 64
 *
 * The state block has to grow with the active partition, which is why 128 KiB
 * chips get away with 8 KiB of state and these need 16 KiB.
 *
 * NOTE: these parts also have 224 KiB of extra on-die flash (`USR_2`) that the
 * `memory_x` options of ch32-metapac (`c256_r64`, `c288_r32`, `c224_r96`, ...)
 * can trade against RAM through the option bytes. This map assumes the default
 * `c256_r64` split; if you change it, resize `RAM` and the flash regions
 * together and make sure `FLASH_OBR.RAM_CODE_MOD` matches, or the chip takes an
 * instruction access fault (`mcause = 0x7`) on the very first code fetch.
 * ------------------------------------------------------------------------- */

MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  BOOTLOADER       (rx)  : ORIGIN = 0x08000000, LENGTH = 16K
  ACTIVE           (rx)  : ORIGIN = 0x08004000, LENGTH = 104K
  DFU              (rx)  : ORIGIN = 0x0801E000, LENGTH = 120K
  BOOTLOADER_STATE (rx)  : ORIGIN = 0x0803C000, LENGTH = 16K

  RAM             (rwx) : ORIGIN = 0x20000000, LENGTH = 64K
}

/* embassy-boot expects offsets from the start of the flash array, not bus
 * addresses, hence the `- ORIGIN(BOOTLOADER)` on every symbol. */
__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOTLOADER);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOTLOADER);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOTLOADER);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOTLOADER);

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOTLOADER);
