/* Linker script for the CH32V305RBT6 bootloader example.
 *
 * The partition layout itself (and the `__bootloader_*` symbols consumed by
 * embassy-boot) lives in `partition-map/ch32v305rbt6.x`, which is shared with
 * the application example. This file only picks the region this binary is
 * placed in and sets up the aliases expected by `qingke-rt`'s `link.x`.
 */

INCLUDE ch32v305rbt6.x

REGION_ALIAS("FLASH", BOOTLOADER)
REGION_ALIAS("REGION_TEXT", FLASH)
REGION_ALIAS("REGION_RODATA", FLASH)
REGION_ALIAS("REGION_DATA", RAM)
REGION_ALIAS("REGION_BSS", RAM)
REGION_ALIAS("REGION_HEAP", RAM)
REGION_ALIAS("REGION_STACK", RAM)
