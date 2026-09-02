# embassy-boot-ch32

[![crates.io](https://img.shields.io/crates/v/embassy-boot-ch32.svg)](https://crates.io/crates/embassy-boot-ch32)
[![docs.rs](https://img.shields.io/docs.rs/embassy-boot-ch32/latest)](https://docs.rs/embassy-boot-ch32)
[![CI](https://github.com/alexxy/embassy-boot-ch32/actions/workflows/ci.yml/badge.svg)](https://github.com/alexxy/embassy-boot-ch32/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE-APACHE)

[`embassy-boot`](https://github.com/embassy-rs/embassy) support for WCH CH32
microcontrollers driven by
[`ch32-hal`](https://github.com/ch32-rs/ch32-hal). Tested on the **CH32V305RBT6**
(QingKe V4 core, 128 KiB flash / 32 KiB RAM); the adapter crate itself is not
V305 specific as long as the chip uses the `ch32-hal` flash driver.

```sh
cargo add embassy-boot-ch32 --features log
```

The examples target the
[`nanoCH32V305`](https://github.com/wuxx/nanoCH32V305) board: its status LED is
on **PA3, active low**, and both consoles live on **USART1** (**PA9** TX /
**PA10** RX) at 115200 8N1.

The crate provides the glue that embassy-boot needs on top of `ch32-hal`:

* `CoarseFlash` — wraps the 256 byte page flash driver so embassy-boot sees a
  coarser (and much cheaper) erase granularity,
* partition accessors generated from the linker script symbols
  (`__bootloader_active_start`, `__bootloader_dfu_start`, `__bootloader_state_start`),
* `BootLoader::load()` — jumps from the bootloader into the active partition,
* `system_reset()` — a software reset through the QingKe PFIC.

## Layout

```
.
├── src/lib.rs                  the adapter crate
├── build.rs                    rejects `log` and `defmt` at the same time
├── partition-map/
│   └── ch32v305rbt6.x          single source of truth for the partition map
├── examples/
│   ├── bootloader/             blocking bootloader with a serial console
│   └── application/            embassy application with mark_booted()/mark_dfu()
├── tools/
│   ├── serial_update.py        host side of the serial update protocol
│   └── check_size.sh           partition fit + entry point guard
└── .github/workflows/ci.yml    GitHub Actions CI
```

Both examples are standalone cargo workspaces (they build for a different target
and use a different `memory.x` than the crate), and both `memory/memory.x` files
`INCLUDE` the shared `partition-map/ch32v305rbt6.x`, so the two binaries can
never disagree about where the partitions are.

## Partition map (CH32V305RBT6)

| region             | address      | size   | contents                        |
| ------------------ | ------------ | ------ | ------------------------------- |
| `BOOTLOADER`       | `0x0800_0000` | 16 KiB | the bootloader                  |
| `ACTIVE`           | `0x0800_4000` | 48 KiB | the running application         |
| `DFU`              | `0x0801_0000` | 56 KiB | the incoming image              |
| `BOOTLOADER_STATE` | `0x0801_E000` | 8 KiB  | embassy-boot state              |

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
`assert_partitions`):

* `active_size % 8192 == 0` → 48 KiB,
* `dfu_size % 8192 == 0` → 56 KiB,
* `dfu_size - active_size >= 8192` (the swap needs one spare block),
* the bootloader's copy buffer (1024 bytes) must divide the page size and be a
  multiple of `WRITE_SIZE`.

## Building

Prerequisites: a Rust nightly with `rust-src` (both examples pin it in
`rust-toolchain.toml`; `build-std` is used because `riscv32imfc-unknown-none-elf`
is a JSON target spec), and optionally WCH's `wlink` command line tool (shipped
with MounRiver Studio) for flashing.

```sh
# the adapter crate (host target, only checks that it compiles)
cargo check

# the bootloader
cd examples/bootloader && cargo build --release

# the application
cd ../application && cargo build --release
```

`tools/check_size.sh` checks that a binary still fits its partition *and* that
its entry point is where the partition map expects it (the two things CI checks
for every build):

```sh
$ tools/check_size.sh \
    --elf examples/bootloader/target/riscv32imfc-unknown-none-elf/release/bootloader-ch32v305 \
    --partition BOOTLOADER --limit 16384 --entry 0x08000000 --bin bootloader.bin
BOOTLOADER: flash 13724 bytes (text 13704 + data 20), RAM .bss 224 bytes, limit 16384
BOOTLOADER: 83% of the partition used, 2660 bytes to spare
BOOTLOADER: entry point 0x08000000
wrote bootloader.bin (13724 bytes)
```

It uses the `llvm-tools` of the active toolchain
(`rustup component add llvm-tools`) and can be pointed at another `llvm-size`
with `$LLVM_SIZE`. If the bootloader does not fit anymore, either shrink it or
move the `ACTIVE`/`DFU` regions up in `partition-map/ch32v305rbt6.x`.

## Flashing

The first time, flash both binaries: the bootloader at `0x0800_0000` and the
application at `0x0800_4000` (where it is linked). A blank state partition reads
as `0xFF`, which embassy-boot treats as `State::Boot`, so the bootloader will
simply boot whatever is in `ACTIVE`.

```sh
# with wlink (the `cargo run` runner does the same thing)
wlink flash examples/bootloader/target/riscv32imfc-unknown-none-elf/release/bootloader-ch32v305
wlink flash examples/application/target/riscv32imfc-unknown-none-elf/release/application-ch32v305
```

The nanoCH32V305 can also be programmed over its USB1 port without a debugger,
with [`wchisp`](https://github.com/ch32-rs/wchisp): hold BOOT, press and release
RST, release BOOT, then

```sh
wchisp flash examples/bootloader/target/riscv32imfc-unknown-none-elf/release/bootloader-ch32v305
wchisp flash examples/application/target/riscv32imfc-unknown-none-elf/release/application-ch32v305
```

`wchisp` understands ELF and programs each segment at its linked address, so the
application lands in `ACTIVE` without an intermediate `.bin`.

```sh
# raw binary, if your tool wants a .bin
# (`llvm-objcopy`, or `rust-objcopy` after `rustup component add llvm-tools`)
llvm-objcopy -O binary \
  examples/application/target/riscv32imfc-unknown-none-elf/release/application-ch32v305 \
  application.bin
```

From then on the application can be updated over the serial console without a
debugger.

## Serial console

USART1, 115200 8N1, **PA9 = TX**, **PA10 = RX** (3.3 V levels).

Within 3 seconds after a reset any key press holds the board in the bootloader;
otherwise the active partition is booted immediately.

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
  examples/application/target/riscv32imfc-unknown-none-elf/release/application-ch32v305 \
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

## Caveats

* **`ch32-metapac` reports `FLASH_SIZE = 480 KiB` for `ch32v305rbt6`** (the
  largest member of the family) while the RBT6 only has 128 KiB, and the flash
  driver bounds its checks with that constant. Nothing will stop a partition map
  that runs past `0x0802_0000`; keeping the map inside 128 KiB is our
  responsibility.
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
  `ch32-hal/defmt`.
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

1. add `partition-map/<your chip>.x` (copy the CH32V305 one) with regions that
   satisfy the constraints above,
2. point `memory/memory.x` of both examples at it and pick the region each binary
   lives in via `REGION_ALIAS("FLASH", ...)`,
3. keep `CoarseFlash<_, N>` where `N` is a multiple of the hardware page size and
   divides all three partitions,
4. adjust the LED pin and the UART instance/pins in the examples.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on pushes to `main`,
on pull requests and manually:

| job | toolchains | what it does |
| --- | --- | --- |
| `crate` | stable, nightly | `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, the same for `--features log` and `--features defmt` separately, `cargo doc` with `RUSTDOCFLAGS=-D warnings` |
| `bootloader` / `application` | nightly | `cargo fmt --check`, `cargo clippy --release -D warnings`, `cargo build --release`, `tools/check_size.sh` (partition fit + entry point), uploads the ELF and the `.bin` |

A few things worth knowing:

* `--all-features` is never used, because `log` and `defmt` are mutually
  exclusive: the two back ends are separate permutations. One step of the
  `crate` job does run `cargo check --all-features` and *requires* it to fail
  with the exclusion message from `build.rs`, so the guard cannot rot away.
* Everything runs with `--locked`. The examples depend on `ch32-hal` from git,
  so the committed `Cargo.lock` is what pins the exact upstream revision; update
  it on purpose with `cargo update -p ch32-hal` and commit the new lock.
* The partition limits are repeated in the CI matrix (`limit:`/`entry:` per
  example) because a CI job cannot read them from the linker script. Keep them
  in sync with `partition-map/ch32v305rbt6.x` when you resize partitions.
* The nightly leg exists to catch new lints early, which also means it can go
  red on clippy churn that is not a real problem; pin a known-good nightly in a
  `rust-toolchain.toml` if that bothers you.

The badges above (and the workflow itself) start working with the first push to
GitHub. The docs.rs badge turns green once docs.rs has built the published
version.

## License

MIT OR Apache-2.0.
