# bootloader example

A fully blocking (no RTOS) `embassy-boot` bootloader for the WCH CH32 parts
driven by [`ch32-hal`](https://github.com/ch32-rs/ch32-hal). This is a
standalone cargo workspace; the repository-level documentation, the supported
chip matrix and the partition geometry live in the
[top-level README](../../README.md).

## What it does on every reset

1. `BootLoader::try_prepare()` performs a pending `DFU` → `ACTIVE` swap, or
   reverts an image that was swapped but never marked good by the application.
2. If the state partition says `DfuDetach` (the application asked for an
   update), it goes straight into an update session.
3. Otherwise it prints a banner on USART1 (115200 8N1, **PA9** TX / **PA10** RX)
   and waits up to 3 seconds for a key. No key press boots the active partition
   immediately.
4. A key press enters the console (`transport-uart`) or holds the board in a
   DFU session (`transport-usb`).

A `prepare` failure (corrupted state partition) also falls into an update
session rather than hanging.

## Feature selection

Exactly **one chip feature** and exactly **one `transport-*` feature** have to
be enabled. `build.rs` resolves the chip feature to the partition map and the
rust target (see [`../chips.rs`](../chips.rs)), forwards it to the matching
`ch32-hal` part feature, and rejects impossible combinations with a short
message. The default is `["ch32v305rbt6", "transport-uart"]`.

```sh
# default: CH32V305RBT6, serial console
cargo build --release

# serial build of another part: pick its feature *and* its target
cargo build --release --no-default-features --features ch32v208rbt6 \
    --target riscv32imc-unknown-none-elf

# USB DFU bootloader (only the ten `-usb` parts, see the root README)
cargo build --release --no-default-features --features ch32v305rbt6,transport-usb
```

`transport-usb` links against the part's `-usb` partition map, which gives the
bootloader 32 KiB instead of 16 KiB; building `transport-usb` for a part
without a USB driver, or with too little flash, fails the build on purpose.

## Serial console (`transport-uart`, the default)

```text
b / Enter   boot the active partition
i           print the partition map and the bootloader state
d / u       start an update session
? / h       help
```

An update session speaks a trivial line + raw data protocol: the host sends
`f <hex length>` (e.g. `f a800`), then exactly that many raw bytes of the
application image. The bootloader acknowledges **every 256 byte chunk after it
has been written to flash** with a single `.`, and the host must wait for it —
the USART has no FIFO. After the last chunk the image is marked updated, the
bootloader swaps it into `ACTIVE` and boots it.

`tools/serial_update.py` at the repository root is the host side of this
protocol:

```sh
python3 ../../tools/serial_update.py /dev/ttyUSB0 application.bin
```

## USB DFU (`transport-usb`)

With `transport-usb` the USART is still used for the banner, the grace window
and a one-line panic message, but the image is served over **USB DFU 1.1** to
[`dfu-util`](https://dfu-util.sourceforge.net/) instead. The USB controller is
picked per part in [`../chips.rs`](../chips.rs):

| controller | parts | data pins |
| --- | --- | --- |
| `usbd` | CH32V208 CBU6/GBU6/RBT6/WBU6 | PA12 (D+), PA11 (D-) |
| `otg_fs` | CH32V305RBT6, CH32V307 RCT6/VCT6/WCU6 | PA12 (DP), PA11 (DM) |
| `usbhs` | CH32V305FBP6, CH32V305GBU6 | PB7 (DP), PB6 (DM) |

```sh
# while the bootloader waits in DFU mode (state is DfuDetach, or a key was
# pressed during the grace window):
dfu-util -a 0 -D application.bin
```

`embassy-usb-dfu` resets the chip itself once the download has been manifested;
the next boot swaps and boots the new image. The rollback rules are identical
to the serial path.

Worth knowing:

* the session is blocking code driven through `embassy_futures::block_on`, i.e.
  the CPU busy-polls the USB device at 144 MHz for the duration of the flash,
* USB builds run the core at `SYSCLK_FREQ_144MHZ_HSI` (the 48 MHz USB clock
  only derives from a 48/96/144 MHz PLL; HSI so boards without a crystal work),
* the VID:PID is the demo pair `0xc0de:0xcafe` — replace it before shipping,
* there is no interactive console over USB: a key press during the grace window
  simply prints the partition info and enters the DFU session.

## Pairing rules

The bootloader and the application must be linked against the **same**
partition map — they read the partition positions from the same
`__bootloader_*` linker symbols, so a mismatch boots into garbage. Concretely:

* `transport-uart` bootloader ↔ application built without `usb-dfu` (plain map,
  `ACTIVE` at `0x0800_4000`),
* `transport-usb` bootloader ↔ application built against the same `-usb` map;
  only then does the `usb-dfu` runtime interface of the
  [application example](../application) have a bootloader to reset into.

## Checking the size

The bootloader has to fit its partition (`tools/check_size.sh` in the
repository root derives the limit from the `__bootloader_active_start` symbol
in the ELF itself and also verifies the entry point):

```sh
../../tools/check_size.sh --elf target/riscv32imfc-unknown-none-elf/release/bootloader \
    --role bootloader --label ch32v305rbt6-bootloader
```

The serial build sits at ~86 % of its 16 KiB partition and the USB build at
~70 % of its 32 KiB one as of this commit; CI keeps both in check. If it stops
fitting, shrink it or move `ACTIVE`/`DFU` up in the map — do not move only the
bootloader.

## Deliberate simplifications

* The panic handler prints only `bootloader panic`: formatting a `PanicInfo`
  pulls in `core::fmt`'s debug and unicode tables (~12 KiB here).
* The console is polled, not interrupt driven — there is no executor in this
  binary. `ch32-hal`'s `embassy` feature is still enabled because its USB
  modules use `embassy-time` unconditionally.
* `ch32_hal::debug::SDIPrint` is enabled but not used for normal output: it
  spins on a flag only a connected WCH-Link clears.
