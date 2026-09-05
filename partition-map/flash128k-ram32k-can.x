/* ---------------------------------------------------------------------------
 * Partition map for the CH32 parts with 128 KiB of application flash (USR_1)
 * and 32 KiB of SRAM that run the CAN-bus update bootloader: CH32V303CBT6,
 * CH32V303RBT6, CH32V305GBU6, CH32V305RBT6.
 *
 * The CAN transport (driver plus the update protocol codec) does not fit into
 * the 16 KiB bootloader partition the serial map uses, so this map gives the
 * bootloader 32 KiB at the cost of 16 KiB of active partition - the same
 * trade the `-usb` map makes. See `flash128k-ram64k-can.x` for the 64 KiB RAM
 * parts.
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
 * NOTE: ch32-metapac reports `FLASH_SIZE = 480 KiB` for these chips (the size
 * of the largest member of the family plus its `USR_2` region) while the part
 * only has 128 KiB of nominal flash. The flash driver accepts accesses up to
 * 480 KiB without complaining, so keeping the partitions inside
 * 0x0800_0000..0x0802_0000 is up to us.
 * ------------------------------------------------------------------------- */

MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  BOOTLOADER       (rx)  : ORIGIN = 0x08000000, LENGTH = 32K
  ACTIVE           (rx)  : ORIGIN = 0x08008000, LENGTH = 32K
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
