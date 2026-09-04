# embassy-boot-ch32

[![crates.io](https://img.shields.io/crates/v/embassy-boot-ch32.svg)](https://crates.io/crates/embassy-boot-ch32)
[![docs.rs](https://img.shields.io/docsrs/embassy-boot-ch32/latest)](https://docs.rs/embassy-boot-ch32)
[![CI](https://github.com/alexxy/embassy-boot-ch32/actions/workflows/ci.yml/badge.svg)](https://github.com/alexxy/embassy-boot-ch32/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE-APACHE)

[`embassy-boot`](https://github.com/embassy-rs/embassy) support for WCH CH32
microcontrollers driven by
[`ch32-hal`](https://github.com/ch32-rs/ch32-hal): a `NorFlash` adapter, a
linker-script driven partition map, and a pair of
[bootloader](examples/bootloader) / [application](examples/application) examples
that build for 20 parts across 5 chip families. The update transport is
selectable: a serial console on every part, USB DFU 1.1 on the ten parts that
have a working `ch32-hal` USB controller. Tested on real hardware on the
**CH32V305RBT6**.

```sh
cargo add embassy-boot-ch32 --features log
```

The examples default to the
[`nanoCH32V305`](https://github.com/wuxx/nanoCH32V305) board: its status LED is
on **PA3, active low**, and both consoles live on **USART1** (**PA9** TX /
**PA10** RX) at 115200 8N1. Both are one-line changes in `src/main.rs` for
another board.

The crate provides the glue that embassy-boot needs on top of `ch32-hal`:

* `CoarseFlash` — wraps the 256 byte page flash driver so embassy-boot sees a
  coarser (and much cheaper) erase granularity,
* partition accessors generated from the linker script symbols
  (`__bootloader_active_start`, `__bootloader_dfu_start`, `__bootloader_state_start`),
* `BootLoader::load()` — jumps from the bootloader into the active partition,
* `system_reset()` — a software reset through the QingKe PFIC.

## Supported chips

One cargo feature per part (the lower case part number, same spelling as in
`ch32-hal`); `build.rs` of each example resolves it to a partition map and to
the target the part needs.

| family | core | target | parts | partition map |
| --- | --- | --- | --- | --- |
| CH32V203 | QingKe V4B, no FPU | `riscv32imc-unknown-none-elf` | C8T6, C8U6, F8P6, F8U6, G8R6, K8T6 | `flash64k-ram20k.x` |
| CH32V208 | QingKe V4C + Bluetooth, no FPU | `riscv32imc-unknown-none-elf` | CBU6, GBU6, RBT6, WBU6 | `flash128k-ram64k.x`, `-usb` |
| CH32V303 | QingKe V4F | `riscv32imfc-unknown-none-elf` | CBT6, RBT6 (128 KiB), RCT6, VCT6 (256 KiB) | `flash128k-ram32k.x`, `flash256k-ram64k.x` |
| CH32V305 | QingKe V4F, USB OTG HS | `riscv32imfc-unknown-none-elf` | FBP6, GBU6, RBT6 | `flash128k-ram32k.x`, `-usb` |
| CH32V307 | QingKe V4F, Ethernet | `riscv32imfc-unknown-none-elf` | RCT6, VCT6, WCU6 | `flash256k-ram64k.x`, `-usb` |

The canonical list lives in [`examples/chips.rs`](examples/chips.rs), which both
`build.rs` files `include!`, so the two binaries can never disagree about the
geometry. `tools/build_matrix.sh --list` prints it.

The ten parts marked `-usb` above can also serve updates over **USB DFU**
(see [USB DFU](#usb-dfu)): the four CH32V208 parts (USBD), the CH32V305 FBP6 /
GBU6 (USBHS) and RBT6 (OTG_FS), and the three CH32V307 parts (OTG_FS). The
CH32V203 parts that have a USBD controller only have 64 KiB of flash, too little
for the 32 KiB USB bootloader, and the `usb/v2fs` block of the CH32V303 family
has no driver in `ch32-hal`, so both families stay serial-only.

### Not (yet) supported

A runtime bootloader needs a working flash *driver*, and `ch32-hal` only has one
for the `flash` IP version `v3` — the CH32V2 and CH32V3 lines. For every other
version `ch32-hal` selects a stub (`src/flash/mod.rs` picks `v3.rs` under
`cfg(flash_v3)` and `other.rs`, where every operation is `unimplemented!()`,
otherwise):

| family | `flash` IP version | state |
| --- | --- | --- |
| CH32V103 | `v1` | no driver upstream |
| CH32V003, CH641 | `v0` | no driver upstream (16 KiB flash / 2 KiB RAM, too small anyway) |
| CH32X033, CH32X035 | `x0` | no driver upstream |
| CH32L103 | `l1` | no driver upstream |

Two further exclusions:

* **CH32V203RBT6** is left out even though its flash IP is `v3`, because
  `ch32-hal` does not compile for it at all (upstream bug, verified on `0766331`).
  It is the only CH32V203 part with a 32-bit general purpose timer (TIM5,
  declared as `GPTM32` in `ch32-metapac`), and in `ch32-hal/src/timer/mod.rs`
  the `foreach_interrupt!` arm for `timer, GPTM32, UP` expands to
  `TimerBits::Bits32` unconditionally, while the `Bits32` variant of the enum is
  `#[cfg(any(ch32l1, ch32v208))]`. Building *any* crate with
  `ch32-hal/ch32v203rbt6` therefore fails with
  `error[E0599]: no variant ... named Bits32 found for enum timer::TimerBits`
  before a single line of this repository is compiled.
* **32 KiB flash parts** (CH32V203C6T6/F6P6/G6U6/K6T6 and friends) are too small
  for a 16 KiB bootloader plus two images plus a state partition.

## Layout

```
.
├── src/lib.rs                  the adapter crate
├── build.rs                    rejects `log` and `defmt` at the same time
├── partition-map/
│   ├── flash64k-ram20k.x       one map per (flash, RAM) geometry
│   ├── flash128k-ram32k.x
│   ├── flash128k-ram64k.x
│   ├── flash256k-ram64k.x
│   └── *-usb.x                 bigger-bootloader variants for USB DFU
├── examples/
│   ├── chips.rs                chip -> map -> target table, included by both build.rs
│   ├── bootloader/             blocking bootloader, serial console or USB DFU
│   └── application/            embassy application with mark_booted()/mark_dfu()
├── tools/
│   ├── serial_update.py        host side of the serial update protocol
│   ├── check_size.sh           partition fit + entry point guard
│   ├── check_partitions.py     validates the partition maps statically
│   └── build_matrix.sh         builds (and checks) every supported chip
└── .github/workflows/ci.yml    GitHub Actions CI
```

Both examples are standalone cargo workspaces (they build for a different target
and use a different `memory.x` than the crate). Neither carries a `memory.x` in
the repository: `build.rs` generates one that `INCLUDE`s the partition map
selected by the chip feature and aliases `FLASH` to `BOOTLOADER` (bootloader) or
`ACTIVE` (application), so the two binaries can never disagree about where the
partitions are.

Each example has its own README with the details of its feature selection,
console and pairing rules:
[examples/bootloader/README.md](examples/bootloader/README.md) and
[examples/application/README.md](examples/application/README.md).

## Partition maps

Maps are named after the nominal application flash and the SRAM they are written
for, because several parts share one geometry. All sizes are multiples of the
8 KiB erase granularity embassy-boot sees (see below), and the `DFU` partition is
always at least one 8 KiB block larger than `ACTIVE`, as the swap requires a
spare block.

| map | flash | RAM | `BOOTLOADER` | `ACTIVE` | `DFU` | `BOOTLOADER_STATE` |
| --- | --- | --- | --- | --- | --- | --- |
| `flash64k-ram20k.x` | 64 KiB | 20 KiB | 16 KiB `0x0800_0000` | 16 KiB `0x0800_4000` | 24 KiB `0x0800_8000` | 8 KiB `0x0800_E000` |
| `flash128k-ram32k.x` | 128 KiB | 32 KiB | 16 KiB `0x0800_0000` | 48 KiB `0x0800_4000` | 56 KiB `0x0801_0000` | 8 KiB `0x0801_E000` |
| `flash128k-ram64k.x` | 128 KiB | 64 KiB | 16 KiB `0x0800_0000` | 48 KiB `0x0800_4000` | 56 KiB `0x0801_0000` | 8 KiB `0x0801_E000` |
| `flash256k-ram64k.x` | 256 KiB | 64 KiB | 16 KiB `0x0800_0000` | 104 KiB `0x0800_4000` | 120 KiB `0x0801_E000` | 16 KiB `0x0803_C000` |
| `flash128k-ram32k-usb.x` | 128 KiB | 32 KiB | 32 KiB `0x0800_0000` | 32 KiB `0x0800_8000` | 56 KiB `0x0801_0000` | 8 KiB `0x0801_E000` |
| `flash128k-ram64k-usb.x` | 128 KiB | 64 KiB | 32 KiB `0x0800_0000` | 32 KiB `0x0800_8000` | 56 KiB `0x0801_0000` | 8 KiB `0x0801_E000` |
| `flash256k-ram64k-usb.x` | 256 KiB | 64 KiB | 32 KiB `0x0800_0000` | 96 KiB `0x0800_8000` | 112 KiB `0x0802_0000` | 16 KiB `0x0803_C000` |

The `-usb` maps give the bootloader 32 KiB instead of 16 KiB (embassy-usb plus
embassy-usb-dfu does not fit into the serial bootloader's partition) and pay for
it out of `ACTIVE`. A board has to be flashed with a matching set: a bootloader
and an application linked against the *same* map, because they disagree about
where `ACTIVE` starts otherwise. `build.rs` of both examples picks the `-usb`
map automatically for `transport-usb` / `usb-dfu` builds and rejects the feature
on parts whose `map_usb` is empty.

`tools/check_partitions.py` validates every map in `partition-map/` (contiguity,
page alignment, the embassy-boot state capacity inequality, total size, RAM size
taken from the file name) and runs in CI.

### Why `CoarseFlash` and why an 8 KiB state partition

`ch32-hal` exposes the internal flash with `WRITE_SIZE = ERASE_SIZE = 256`
bytes. embassy-boot's `assert_partitions` requires

```text
2 + 4 * (active_size / PAGE_SIZE) <= state_capacity / STATE::WRITE_SIZE
```

With a 256 byte page size and a 48 KiB active partition that would need
`2 + 4 * 192 = 770` slots of 256 bytes, i.e. ~193 KiB of state on a 128 KiB
chip. Raising the erase granularity that embassy-boot sees to 8 KiB brings the
requirement down to `2 + 4 * 6 = 26` slots, i.e. 6.5 KiB, which fits into a
single 8 KiB block.

`CoarseFlash<Flash, 8192>` implements `NorFlash` with `ERASE_SIZE = 8192` by
erasing 32 hardware pages per `erase()` call. The cost is that a swap erases
whole 8 KiB blocks instead of only the 256 byte pages it overwrites; the benefit
is that the swap can be interrupted by a power loss at any time and still be
resumed or reverted.

Other constraints that shaped the table above (all checked at compile time by
`assert_partitions`, and statically by `tools/check_partitions.py`):

* `active_size % 8192 == 0`,
* `dfu_size % 8192 == 0`,
* `dfu_size - active_size >= 8192` (the swap needs one spare block),
* the bootloader's copy buffer (1024 bytes) must divide the page size and be a
  multiple of `WRITE_SIZE`.

## Building

Prerequisites: a Rust nightly with `rust-src` (both examples pin it in
`rust-toolchain.toml`; `build-std` is used because
`riscv32imfc-unknown-none-elf` is a JSON target spec), and optionally WCH's
`wlink` command line tool (shipped with MounRiver Studio) for flashing.

```sh
# the adapter crate (host target, only checks that it compiles)
cargo check

# the bootloader and the application for the default chip (CH32V305RBT6)
cd examples/bootloader && cargo build --release
cd ../application && cargo build --release
```

For another part, pick its feature *and* the target it needs (the table above,
or `tools/build_matrix.sh --list`):

```sh
cd examples/bootloader
cargo build --release --no-default-features --features ch32v208rbt6 \
    --target riscv32imc-unknown-none-elf
```

The bootloader additionally needs exactly one `transport-*` feature
(`transport-uart` by default); the application gets an optional `usb-dfu`
feature. The USB variants of both are covered in [USB DFU](#usb-dfu).

`build.rs` refuses to continue if the feature and the target disagree, so a
`--target` mistake is a short error message instead of a link failure. The
output binaries are called `bootloader` and `application`.

To build (and size check) everything:

```sh
tools/build_matrix.sh                      # every chip, both examples, both transports
tools/build_matrix.sh --example bootloader # one example
tools/build_matrix.sh --transport usb      # only the USB DFU builds
tools/build_matrix.sh --chip ch32v307rct6 --bin-dir dist
```

`tools/check_size.sh` checks that a binary still fits its partition *and* that
its entry point is where the partition map expects it. Both the limit and the
expected entry point are derived from the `__bootloader_active_start` /
`__bootloader_active_end` symbols in the ELF itself, so the numbers are never
duplicated outside the map:

```sh
$ tools/check_size.sh --elf examples/bootloader/target/riscv32imfc-unknown-none-elf/release/bootloader \
    --role bootloader --label ch32v305rbt6-bootloader
ch32v305rbt6-bootloader (bootloader): must fit the bootloader partition at the start of flash, below 0x08004000, limit 16384 bytes
ch32v305rbt6-bootloader: flash 14160 bytes (text 14140 + data 20), RAM .bss 224 bytes
ch32v305rbt6-bootloader: 86% used, 2224 bytes to spare
ch32v305rbt6-bootloader: entry point 0x08000000
```

`--role application` uses the `ACTIVE` partition instead. The script uses the
`llvm-tools` of the active toolchain (`rustup component add llvm-tools`) and can
be pointed at another tool with `$LLVM_SIZE`, `$LLVM_NM`, … If the bootloader
does not fit anymore, either shrink it or move `ACTIVE`/`DFU` up in the
partition map it uses.

## Flashing

The first time, flash both binaries: the bootloader at `0x0800_0000` and the
application at `0x0800_4000` (where it is linked; `0x0800_8000` for a `-usb`
map, see [USB DFU](#usb-dfu)). A blank state partition reads
as `0xFF`, which embassy-boot treats as `State::Boot`, so the bootloader will
simply boot whatever is in `ACTIVE`.

```sh
# with wlink (the `cargo run` runner does the same thing)
wlink flash examples/bootloader/target/riscv32imfc-unknown-none-elf/release/bootloader
wlink flash examples/application/target/riscv32imfc-unknown-none-elf/release/application
```

A nanoCH32V305 can also be programmed over its USB1 port without a debugger,
with [`wchisp`](https://github.com/ch32-rs/wchisp): hold BOOT, press and release
RST, release BOOT, then

```sh
wchisp flash examples/bootloader/target/riscv32imfc-unknown-none-elf/release/bootloader
wchisp flash examples/application/target/riscv32imfc-unknown-none-elf/release/application
```

`wchisp` understands ELF and programs each segment at its linked address, so the
application lands in `ACTIVE` without an intermediate `.bin`.

```sh
# raw binary, if your tool wants a .bin
# (`llvm-objcopy`, or `rust-objcopy` after `rustup component add llvm-tools`)
llvm-objcopy -O binary \
  examples/application/target/riscv32imfc-unknown-none-elf/release/application \
  application.bin
```

From then on the application can be updated over the serial console without a
debugger.

## Serial console

USART1, 115200 8N1, **PA9 = TX**, **PA10 = RX** (3.3 V levels).

Within 3 seconds after a reset any key press holds the board in the bootloader;
otherwise the active partition is booted immediately. The
[bootloader example README](examples/bootloader/README.md) describes the console,
its variants and the size budget in detail.

```text
b / Enter   boot the active partition
i           print the partition map and the bootloader state
d / u       start an update session
? / h       help
```

### Update protocol

An update session speaks a trivial line + raw data protocol:

1. the host sends `f <hex length>` followed by CR or LF, e.g. `f a800`,
2. the host sends exactly that many raw bytes of the application image,
3. the bootloader acknowledges **every 256 byte chunk after it has been written
   to flash** with a single `.` (with a `N / M bytes` progress line every
   16 KiB), and the host **must wait for that acknowledgment** before sending
   the next chunk: the USART has no FIFO and flash programming takes
   milliseconds, so a host that just streams will overrun the receiver,
4. after the last chunk the image is marked updated, the session ends, the
   bootloader swaps `DFU` into `ACTIVE` and boots it.

The tail of the last chunk is padded with `0xFF`, which is what erased flash
reads as.

The image is a **raw binary**, not an ELF: the bootloader copies the bytes
straight into the `DFU` partition, which is linked to run at `0x0800_4000`.

A host script (Python + [`pyserial`](https://pypi.org/project/pyserial/)) is
provided:

```sh
llvm-objcopy -O binary \
  examples/application/target/riscv32imfc-unknown-none-elf/release/application \
  application.bin

python3 tools/serial_update.py /dev/ttyUSB0 application.bin
```

It resets the board over RTS (wire RTS to RESET, otherwise reset it by hand),
holds the bootloader during the grace window, and then stops and waits for the
`.` acknowledgment after every 256 byte chunk.

## Rollback

The application is only trusted once it calls
[`mark_booted()`](examples/application/src/main.rs). Until then the state
partition keeps the `Swap` magic and the bootloader reverts the image on the next
reset, so a bad image cannot brick the board:

1. `mark_updated()` writes the `Swap` magic,
2. the bootloader swaps and boots the new image, state stays `Swap`,
3. the new application runs its self-check and calls `mark_booted()`,
4. if it resets or hangs before that, step 2 is undone and the previous image
   boots again.

An application can also ask to stay in the bootloader by calling `mark_dfu()`
and resetting (`d` in the example application). `mark_dfu()` does not touch the
active image; it only makes the bootloader wait for a new one.

## USB DFU

The ten `-usb` parts (see [Supported chips](#supported-chips)) can take updates
over USB DFU 1.1 through
[`embassy-usb-dfu`](https://crates.io/crates/embassy-usb-dfu) instead of the
serial console. There are two independent halves, each behind its own feature:

* a **`transport-usb` [bootloader](examples/bootloader)** is a DFU 1.1
  *download* device: `dfu-util` writes the application image into the `DFU`
  partition, the bootloader marks it updated and resets; the next boot swaps
  `DFU` into `ACTIVE` and boots it,
* a **`usb-dfu` [application](examples/application)** exposes only the DFU
  *runtime* interface: a `dfu-util -e` (DFU_DETACH) makes it write the DFU magic
  into the state partition and reset, which lands in the USB bootloader.

Both halves must be built against the same `-usb` partition map, and the
bootloader has to be a `transport-usb` one for the runtime detach to have
somewhere to land.

```sh
cd examples/bootloader
cargo build --release --no-default-features --features ch32v305rbt6,transport-usb \
    --target riscv32imfc-unknown-none-elf

cd ../application
cargo build --release --no-default-features --features ch32v305rbt6,usb-dfu \
    --target riscv32imfc-unknown-none-elf
```

The first time, flash the bootloader at `0x0800_0000` and the application at
`0x0800_8000` (the `ACTIVE` of the `-usb` maps); `wchisp flash` programs an ELF
segment by segment, so both ELFs go in as they are. From then on neither a
debugger nor a serial cable is needed:

```sh
# while the application runs: ask the board to reset into the USB bootloader
dfu-util -e

# while the bootloader waits in DFU mode: send a raw binary and reboot
llvm-objcopy -O binary \
  examples/application/target/riscv32imfc-unknown-none-elf/release/application \
  application.bin
dfu-util -a 0 -D application.bin
```

`embassy-usb-dfu` resets the chip by itself once the download has been
manifested, and the same rollback rules as for a serial update apply
(`mark_booted()` in the application, revert on failure).

### USB controllers

The controller is selected per part in `examples/chips.rs`:

| controller | parts | data pins | interrupt | speed |
| --- | --- | --- | --- | --- |
| `usbd` | CH32V208 CBU6, GBU6, RBT6, WBU6 | PA12 (D+), PA11 (D-) | `USB_LP_CAN1_RX0` | full speed |
| `otg_fs` | CH32V305RBT6, CH32V307 RCT6/VCT6/WCU6 | PA12 (DP), PA11 (DM) | `OTG_FS` | full speed |
| `usbhs` | CH32V305FBP6, CH32V305GBU6 | PB7 (DP), PB6 (DM) | `USBHS` (+ `USBHS_WKUP`) | high speed |

On the [`nanoCH32V305`](https://github.com/wuxx/nanoCH32V305) the USB-C socket
is wired to **OTG_FS**, the same controller the on-chip ISP monitor uses, so
`wchisp`, DFU and the running application all share one port — which is why
`ch32v305rbt6` prefers `otg_fs` over the `usbhs` block it also has.

### USB caveats

* **48 MHz clock.** The USB block needs a 48 MHz clock derived from a PLL at
  48, 96 or 144 MHz, so both USB examples run the core at
  `SYSCLK_FREQ_144MHZ_HSI` (HSI rather than HSE so boards without a crystal
  work too). Serial builds keep the `ch32-hal` default clock.
* **Demo VID/PID.** Both examples use `0xc0de:0xcafe`; replace it with your own
  allocation before shipping anything.
* **The DFU bootloader busy-polls.** It is still blocking code driven through
  `embassy_futures::block_on`, which just polls the USB device in a loop. Fine
  for flashing an image, but there is no waker-driven driver and no idle path.
* **A `-usb` build shrinks the application.** 32 KiB of bootloader instead of 16
  takes 8–16 KiB away from `ACTIVE`; `tools/check_size.sh` and CI keep both
  variants inside their maps.

## Caveats

* **Only the CH32V2/V3 flash controller is implemented upstream.** Everything
  above the line "Not (yet) supported" is a limitation of `ch32-hal`, not of this
  crate: the adapter itself is chip agnostic.
* **`ch32-metapac` over-reports flash size.** It reports `FLASH_SIZE` as the
  size of the largest member of the family plus its extra `USR_2` region (480 KiB
  for a 128 KiB CH32V305RBT6), and the flash driver bounds its checks with that
  constant. Nothing will stop a partition map that runs past the physical flash;
  keeping the map inside the part's nominal flash is our responsibility, which is
  what `tools/check_partitions.py` checks.
* **Option bytes.** This example assumes the factory configuration: the chip
  boots through the on-chip monitor ROM into the user flash at `0x0800_0000`. If
  you have used WCHISPTool or `wlink` to change the boot configuration (boot
  from RAM, read protection, etc.), restore the defaults before testing, or the
  reset vector will not land in the bootloader.
* **`ch32-hal` needs its `embassy` feature** to build today: its USB modules call
  `embassy_time` unconditionally. The bootloader example is plain blocking code
  and does not use an executor or a time driver, but it still has to enable the
  feature.
* **No `defmt` for now.** `ch32-hal` pins `defmt` 0.3 while `embassy-boot` uses
  `defmt` 1.x, so the `defmt` feature of this crate cannot be combined with
  `ch32-hal/defmt`. The examples therefore use `log`.
* **`log` and `defmt` are exclusive.** Cargo cannot express that in
  `[features]`, so `build.rs` turns the combination into a build error
  (`cargo::error=`, Rust 1.84+) instead of letting it fail deep inside
  `embassy-boot`.
* **`SDIPrint` can hang without a debugger.** `ch32_hal::debug::SDIPrint` spins
  on a busy flag that only a connected WCH-Link clears, so all normal output in
  both examples goes to USART1.
* The bootloader's panic handler deliberately does not print the panic message
  or location: formatting a `PanicInfo` pulls in `core::fmt`'s debug and unicode
  tables, which cost ~12 KiB here. Rebuild with a debug profile if you need the
  details.

## Porting to another chip

If the part has the `v3` flash IP, adding it is three steps:

1. pick the geometry: reuse a map in `partition-map/`, or add one for the new
   `(flash, RAM)` pair and make sure `tools/check_partitions.py` is happy with
   it,
2. add a `Chip { part, map, target, usb, map_usb }` entry to `examples/chips.rs`
   and a `ch32XXXX = ["ch32-hal/ch32XXXX"]` feature to both examples'
   `Cargo.toml`; leave `usb` and `map_usb` empty unless `ch32-hal` has a driver
   for the part's USB controller *and* the flash can host a `-usb` map with a
   32 KiB bootloader,
3. adjust the LED pin and the UART instance/pins in the examples if the board
   differs from a nanoCH32V305.

`tools/build_matrix.sh --chip <part>` then builds and size checks both binaries,
and CI picks the part up automatically (its matrix is generated from
`examples/chips.rs`).

For a part with a different flash controller the real work is upstream: a driver
in `ch32-hal` for that `flash` IP version.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on pushes to `main`,
on pull requests and manually:

| job | toolchains | what it does |
| --- | --- | --- |
| `crate` | stable, nightly | `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, the same for `--features log` and `--features defmt` separately, `cargo doc` with `RUSTDOCFLAGS=-D warnings`, `python3 tools/check_partitions.py` |
| `chips` | — | turns `examples/chips.rs` into the chip matrix for the next job |
| `firmware` (one job per part) | nightly | `tools/build_matrix.sh --chip <part>`: builds both transports of the bootloader and the application for the target that part needs (the USB ones only where a `-usb` map exists), checks every binary with `tools/check_size.sh`, uploads `dist/<part>/*.bin` as `firmware-<part>` |
| `examples-lint` | nightly | `cargo fmt --check` and `cargo clippy --release -D warnings` for both examples, in the default (serial) and the USB variant (default part) |

A few things worth knowing:

* `--all-features` is never used, because `log` and `defmt` are mutually
  exclusive: the two back ends are separate permutations. One step of the
  `crate` job does run `cargo check --all-features` and *requires* it to fail
  with the exclusion message from `build.rs`, so the guard cannot rot away.
* Everything runs with `--locked`. The examples depend on `ch32-hal` from git,
  so the committed `Cargo.lock` is what pins the exact upstream revision; update
  it on purpose with `cargo update -p ch32-hal` and commit the new lock.
* No partition sizes or entry points are duplicated in the workflow: the limits
  come from the ELF symbols and the chip list from `examples/chips.rs`.
* The `firmware` jobs cache only the downloaded registry and git checkouts
  (`cache-targets: false`): every part is a different feature set, so caching
  `target/` would store 20 near-identical trees.
* The nightly leg exists to catch new lints early, which also means it can go
  red on clippy churn that is not a real problem; pin a known-good nightly in a
  `rust-toolchain.toml` if that bothers you.

The badges above (and the workflow itself) start working with the first push to
GitHub. The docs.rs badge turns green once docs.rs has built the published
version.

## Roadmap

* **CAN-bus updates.** A draft protocol specification for flashing one specific
  node on a multi-node CAN bus (7-bit NodeID filtering plus verification against
  the factory 96-bit unique device ID) lives in
  [docs/can-update-protocol.md](docs/can-update-protocol.md). No implementation
  has started; feedback on the draft is very welcome.
* Re-enable **CH32V203RBT6** once the upstream `ch32-hal` timer-variant bug is
  fixed (see [Not (yet) supported](#not-yet-supported)).

## License

MIT OR Apache-2.0.
