# application example

A plain [`embassy`](https://github.com/embassy-rs/embassy) application that is
linked into the `ACTIVE` partition of a
[`ch32-hal`](https://github.com/ch32-rs/ch32-hal) part and booted by the [bootloader example](../bootloader). This is a standalone
cargo workspace; the repository-level documentation, the supported chip matrix
and the partition geometry live in the
[top-level README](../../README.md).

It shows the two things an application has to do to take part in the update
protocol:

* call [`BlockingFirmwareUpdater::mark_booted()`](https://docs.rs/embassy-boot-ch32)
  once the image has proven itself — until that succeeds the state partition
  keeps the `Swap` magic and the bootloader reverts the image on the next reset,
* optionally call `mark_dfu()` and reset to ask the bootloader to stay in its
  update session.

Besides that it is a blinky: the status LED on **PA3** (active low on the
[nanoCH32V305](https://github.com/wuxx/nanoCH32V305), adjust the polarity for
other boards) toggles every 500 ms from an embassy task.

## What it does on every boot

1. Prints a banner on USART1 (115200 8N1, **PA9** TX / **PA10** RX) with the
   chip name and the partition map it was linked against.
2. Reads the boot state, then calls `mark_booted()`. When the state was `Swap`
   it prints `this was a fresh image, it is now marked as booted`.
3. Spawns the blink task.
4. Runs the console command loop — or, in a `usb-dfu` build, the DFU runtime
   interface — while the blink task keeps running through the executor. A
   `can-runtime` build keeps the console loop and polls the CAN bus from it.

## Feature selection

Exactly **one chip feature** has to be enabled (the default is
`ch32v305rbt6`); `build.rs` resolves it to the partition map and the rust target
(see [`../chips.rs`](../chips.rs)). The optional `usb-dfu` feature adds the DFU
1.1 runtime interface and is only available on the ten parts that have a working
`ch32-hal` USB controller; the optional `can-runtime` feature adds the CAN bus
update listener and is only available on the thirteen parts that have a CAN
controller (see the root README). On any other part the build fails on
purpose, and `usb-dfu` and `can-runtime` cannot be combined.

```sh
# default: CH32V305RBT6, serial console
cargo build --release

# another part: pick its feature *and* its target
cargo build --release --no-default-features --features ch32v208rbt6 \
    --target riscv32imc-unknown-none-elf

# DFU runtime interface (only the ten `-usb` parts)
cargo build --release --no-default-features --features ch32v305rbt6,usb-dfu

# CAN update listener (only the thirteen parts with CAN)
cargo build --release --no-default-features --features ch32v305rbt6,can-runtime
```

A `usb-dfu` build is linked against the part's `-usb` partition map and runs the
core at `SYSCLK_FREQ_144MHZ_HSI` (the 48 MHz USB clock only derives from a
48/96/144 MHz PLL; HSI so boards without a crystal work), exactly like the
`transport-usb` bootloader.

## Serial console (plain build)

```text
i   print the partition map and the boot state
d   mark the state partition for DFU and reset into the bootloader
```

`d` writes the `DfuDetach` mark and performs a plain system reset — not a jump
back into the bootloader, because the application has enabled peripheral
interrupts whose vectors the bootloader does not handle. The bootloader picks
the mark up on the next boot and stays in its update session.

## USB DFU runtime (`usb-dfu`)

Instead of the console loop the binary serves the DFU 1.1 *runtime* interface
over the part's USB controller (PA12/PA11 for USBD and OTG_FS, PB7/PB6 for
USBHS — see [`../chips.rs`](../chips.rs)):

```sh
dfu-util -l   # list: the device answers GET_STATUS even in application mode
dfu-util -e   # detach: mark the state partition for DFU and reset
```

The detach lands the board in the `transport-usb` bootloader, which then serves
the image (`dfu-util -a 0 -D application.bin`). No flash programming code lives
in the application itself; see [`src/usb_runtime.rs`](src/usb_runtime.rs).

Worth knowing:

* the VID:PID is the demo pair `0xc0de:0xcafe`, identical to the bootloader's,
  so `dfu-util -d 0xc0de:0xcafe` finds the device in both modes — replace it
  before shipping,
* there are no MSOS descriptors, so Windows needs the DFU driver assigned by
  hand,
* the blink task keeps running while the interface is active; a USB cable that
  carries only power simply means no host ever detaches it.

## CAN update listener (`can-runtime`)

A `can-runtime` build keeps the serial console loop and additionally polls
CAN1 from it — **PA11 (RX) / PA12 (TX)**, or **PB8 / PB9** with the
`can-pb8-pb9` feature, which must match the [bootloader build](../bootloader);
see its README for the transceiver and termination. On the bus
([docs/can-update-protocol.md](../../docs/can-update-protocol.md)) it:

* answers a targeted `PING` and any `GET_INFO` — including the functional
  discovery broadcast, answered after a per-node delay derived from the unique
  device ID so a bus full of nodes does not collide — reporting state `APP`,
* on a targeted `ENTER_UPDATE` whose embedded UID prefix matches its own chip,
  answers OK, then does exactly what the `d` key does: `mark_dfu()` and a
  reset, ~5 ms later, into the `transport-can` bootloader,
* ignores everything else; the flash transfer only ever happens in the
  bootloader. See [`src/can_runtime.rs`](src/can_runtime.rs).

So updating a running board is a single command from the host:

```sh
python3 ../../tools/can_update.py --interface socketcan --channel can0 \
    --bitrate 1000000 --node 1 application.bin
```

`NODE_ID` and `CAN_BITRATE` at the top of `src/can_runtime.rs` must match the
bootloader build and the host tool. Unlike `usb-dfu`, a `can-runtime` build
needs no clock changes and coexists happily with the serial console: `d` still
works, it just lands in the same CAN bootloader.

## First flash and pairing rules

The application is linked into `ACTIVE`: at `0x0800_4000` for the plain maps and
at `0x0800_8000` for the `-usb` and `-can` maps. For the very first flash it has to be
flashed over ISP together with the bootloader (`wchisp`/`wlink` take the ELF and
use its link addresses):

```sh
wchisp flash target/riscv32imfc-unknown-none-elf/release/application
```

After that, updates go through the bootloader. The bootloader and the
application must be linked against the **same** partition map, so the pair is:

* plain application ↔ `transport-uart` bootloader,
* `usb-dfu` application ↔ `transport-usb` bootloader,
* `can-runtime` application ↔ `transport-can` bootloader (same `-can` map,
  `can-pb8-pb9` choice, `NODE_ID` and bit rate).

A mismatch boots into garbage: both binaries read the partition positions from
the same `__bootloader_*` linker symbols.

## Checking the size

The image has to fit its `ACTIVE` partition (`tools/check_size.sh` in the
repository root derives the limit and the expected entry point from the
`__bootloader_active_start`/`__bootloader_active_end` symbols in the ELF
itself):

```sh
../../tools/check_size.sh --elf target/riscv32imfc-unknown-none-elf/release/application \
    --role application --label ch32v305rbt6-application
```

The plain build sits at ~24 % of its 48 KiB partition, the `usb-dfu` build at
roughly 60 % and the `can-runtime` build at ~44 % of their 32 KiB ones as of
this commit; CI keeps every permutation of the chip matrix in check.
