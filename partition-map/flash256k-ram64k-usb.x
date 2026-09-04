/* ---------------------------------------------------------------------------
 * Partition map for the CH32 parts with 256 KiB of application flash (USR_1)
 * and 64 KiB of SRAM that run the USB DFU bootloader: CH32V307RCT6,
 * CH32V307VCT6, CH32V307WCU6.
 *
 * The USB stack makes the bootloader bigger than the UART one, which is paid
 * for by taking 8 KiB away from the active partition.
 *
 *  region            address            size    notes
 *  ----------------  -----------------  ------  --------------------------------
 *  BOOTLOADER        0x0800_0000        32 KiB  the bootloader itself
 *  ACTIVE            0x0800_8000        96 KiB  running application
 *  DFU               0x0802_0000       112 KiB  incoming image
 *  BOOTLOADER_STATE  0x0803_C000        16 KiB  embassy-boot state
 *
 * The erase granularity used by embassy-boot is 8 KiB (see `CoarseFlash`),
 * which gives the following requirements:
 *
 *   PAGE_SIZE                        = 8192
 *   ACTIVE  % PAGE_SIZE == 0         -> 98304 / 8192 = 12 blocks
 *   DFU     % PAGE_SIZE == 0         -> 114688 / 8192 = 14 blocks
 *   DFU - ACTIVE >= PAGE_SIZE        -> 16384 >= 8192 (the swap needs one spare)
 *   2 + 4 * (ACTIVE / PAGE_SIZE) = 50 <= STATE / WRITE_SIZE = 16384 / 256 = 64
 *
 * The state block has to grow with the active partition, which is why these
 * parts keep the 16 KiB of state the UART map uses.
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
  BOOTLOADER       (rx)  : ORIGIN = 0x08000000, LENGTH = 32K
  ACTIVE           (rx)  : ORIGIN = 0x08008000, LENGTH = 96K
  DFU              (rx)  : ORIGIN = 0x08020000, LENGTH = 112K
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
