/* ---------------------------------------------------------------------------
 * Partition map for the CH32 parts with 128 KiB of application flash (USR_1)
 * and 64 KiB of SRAM that run the CAN-bus update bootloader: CH32V208CBU6,
 * CH32V208GBU6, CH32V208RBT6, CH32V208WBU6.
 *
 * The CAN transport (driver plus the update protocol codec) does not fit into
 * the 16 KiB bootloader partition the serial map uses, so this map gives the
 * bootloader 32 KiB at the cost of 16 KiB of active partition. See
 * `flash128k-ram32k-can.x` for the same layout on the 32 KiB RAM parts.
 *
 *  region            address            size    notes
 *  ----------------  -----------------  ------  --------------------------------
 *  BOOTLOADER        0x0800_0000        32 KiB  the bootloader itself
 *  ACTIVE            0x0800_8000        32 KiB  running application
 *  DFU               0x0801_0000        56 KiB  incoming image
 *  BOOTLOADER_STATE  0x0801_E000         8 KiB  embassy-boot state
 *
 * The erase granularity used by embassy-boot is 8 KiB (see `CoarseFlash`),
 * which gives the following requirements:
 *
 *   PAGE_SIZE                        = 8192
 *   ACTIVE  % PAGE_SIZE == 0         -> 32768 / 8192 = 4 blocks
 *   DFU     % PAGE_SIZE == 0         -> 57344 / 8192 = 7 blocks
 *   DFU - ACTIVE >= PAGE_SIZE        -> 24576 >= 8192 (the swap needs one spare)
 *   2 + 4 * (ACTIVE / PAGE_SIZE) = 18 <= STATE / WRITE_SIZE = 8192 / 256 = 32
 *
 * 32 + 32 + 56 + 8 = 128 KiB.
 *
 * NOTE: these parts also have extra on-die flash (`USR_2`) that the
 * `memory_x` options of ch32-metapac can trade against RAM through the option
 * bytes. This map assumes the default split with 64 KiB of RAM. If you pick
 * another split, resize `RAM` (and the flash regions) accordingly and make
 * sure `FLASH_OBR.RAM_CODE_MOD` matches, or the chip takes an instruction
 * access fault (`mcause = 0x7`) on the very first code fetch.
 * ------------------------------------------------------------------------- */

MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  BOOTLOADER       (rx)  : ORIGIN = 0x08000000, LENGTH = 32K
  ACTIVE           (rx)  : ORIGIN = 0x08008000, LENGTH = 32K
  DFU              (rx)  : ORIGIN = 0x08010000, LENGTH = 56K
  BOOTLOADER_STATE (rx)  : ORIGIN = 0x0801E000, LENGTH = 8K

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
