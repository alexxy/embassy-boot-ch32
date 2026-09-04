#!/usr/bin/env python3
"""Checks the linker partition maps in `partition-map/`.

Every `*.x` file in there is used by both examples, so a map that does not
satisfy embassy-boot's `assert_partitions()` is a bug that would otherwise only
show up as a panic on the board. The file name carries the geometry of the chip
it was written for (`flash<app flash>-ram<sram>[-usb].x`, the `-usb` maps
belong to the USB DFU bootloader), which is what the total partition sizes
are checked against.

Usage:

    tools/check_partitions.py            # check every map, print a summary
    tools/check_partitions.py --markdown # also print the region tables
"""

import os
import re
import sys

FLASH_BASE = 0x08_0000_00
RAM_BASE = 0x20_00_0000

# The granularity `CoarseFlash` presents to embassy-boot and the write/erase
# size of the flash IP version `ch32-hal` implements (256 byte pages).
PAGE_SIZE = 8192
HW_PAGE_SIZE = 256

REGIONS = ("BOOTLOADER", "ACTIVE", "DFU", "BOOTLOADER_STATE")

REGION_RE = re.compile(
    r"^\s*(\w+)\s*\(\w+\)\s*:\s*ORIGIN\s*=\s*(0x[0-9a-fA-F]+)\s*,\s*LENGTH\s*=\s*(\d+)([KkMm]?)\s*$"
)
# A `-usb` suffix marks the maps used by the (bigger) USB DFU bootloader; the
# geometry the name carries is unchanged.
NAME_RE = re.compile(r"^flash(\d+)([KkMm])-ram(\d+)([KkMm])(-usb)?\.x$")


def scaled(value, suffix):
    return int(value) * {"": 1, "k": 1024, "m": 1024 * 1024}[suffix.lower()]


def read(path):
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def check_comments(text):
    """Reports unbalanced C style comments, by line number.

    Linker scripts have no nesting and no way to report this usefully: writing a
    glob such as `examples/*/build.rs` inside a comment silently ends it, and the
    linker then tries to parse the rest of the prose as instructions.
    """
    problems = []
    in_comment = False
    opened_at = 0
    line = 1
    index = 0

    while index < len(text):
        pair = text[index : index + 2]
        if text[index] == "\n":
            line += 1
        elif not in_comment and pair == "/*":
            in_comment = True
            opened_at = line
            index += 1
        elif in_comment and pair == "*/":
            in_comment = False
            index += 1
        elif not in_comment and pair == "*/":
            problems.append(
                f"line {line}: comment terminator without an opener. A `*/` inside a comment "
                "(the glob `examples/*/build.rs` is a classic) silently ends it and the linker "
                "then parses the rest of the prose as instructions."
            )
            index += 1
        index += 1

    if in_comment:
        problems.append(f"line {opened_at}: this comment is never closed")
    return problems


def parse(text):
    regions = {}
    for line in text.splitlines():
        match = REGION_RE.match(line)
        if match:
            name, origin, length, suffix = match.groups()
            regions[name] = (int(origin, 16), scaled(length, suffix))
    return regions


def check(path):
    """Returns (problems, regions, geometry) for one map file."""
    problems = []
    match = NAME_RE.match(os.path.basename(path))
    if not match:
        return (
            [
                "file name must be flash<n><K|M>-ram<n><K|M>[-usb].x so the geometry can be "
                "checked"
            ],
            {},
            None,
        )
    flash = scaled(match.group(1), match.group(2))
    ram = scaled(match.group(3), match.group(4))

    text = read(path)
    problems.extend(check_comments(text))
    regions = parse(text)
    missing = [name for name in REGIONS + ("RAM",) if name not in regions]
    if missing:
        problems.append(f"missing regions: {', '.join(missing)}")
        return (problems, regions, (flash, ram))

    if PAGE_SIZE % HW_PAGE_SIZE:
        problems.append("the coarse page size must be a multiple of the hardware page size")

    origin, length = regions["BOOTLOADER"]
    if origin != FLASH_BASE:
        problems.append(f"BOOTLOADER must start at {FLASH_BASE:#010x}, found {origin:#010x}")

    # The partitions have to tile the flash without gaps: embassy-boot's swap
    # works on the whole active/dfu range and a gap would silently end up in
    # nobody's partition.
    previous_end = None
    for name in REGIONS:
        origin, length = regions[name]
        if previous_end is not None and origin != previous_end:
            problems.append(
                f"{name} starts at {origin:#010x}, previous partition ends at {previous_end:#010x}"
            )
        previous_end = origin + length

    for name in ("ACTIVE", "DFU", "BOOTLOADER_STATE"):
        origin, length = regions[name]
        if (origin - FLASH_BASE) % PAGE_SIZE:
            problems.append(
                f"{name} origin is not a multiple of the {PAGE_SIZE} byte page embassy-boot uses"
            )
        if length % PAGE_SIZE and name != "BOOTLOADER_STATE":
            problems.append(f"{name} size {length} is not a multiple of {PAGE_SIZE}")

    active = regions["ACTIVE"][1]
    dfu = regions["DFU"][1]
    state = regions["BOOTLOADER_STATE"]

    if dfu - active < PAGE_SIZE:
        problems.append(
            f"the swap needs one spare block: dfu ({dfu}) - active ({active}) < {PAGE_SIZE}"
        )

    slots = state[1] // HW_PAGE_SIZE
    needed = 2 + 4 * (active // PAGE_SIZE)
    if needed > slots:
        problems.append(
            f"state partition too small: needs {needed} slots of {HW_PAGE_SIZE} bytes, has {slots}"
        )

    used = previous_end - FLASH_BASE
    if used > flash:
        problems.append(f"partitions use {used} bytes, the chip only has {flash}")

    ram_origin, ram_length = regions["RAM"]
    if ram_origin != RAM_BASE:
        problems.append(f"RAM must start at {RAM_BASE:#010x}, found {ram_origin:#010x}")
    if ram_length != ram:
        problems.append(f"RAM region is {ram_length} bytes, the file name promises {ram}")

    if active < PAGE_SIZE:
        problems.append(f"the active partition must hold at least one {PAGE_SIZE} byte block")

    return (problems, regions, (flash, ram))


def main():
    directory = os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir, "partition-map")
    markdown = "--markdown" in sys.argv
    maps = sorted(name for name in os.listdir(directory) if name.endswith(".x"))

    if not maps:
        print(f"no partition maps found in {directory}", file=sys.stderr)
        return 1

    failed = False
    for name in maps:
        problems, regions, geometry = check(os.path.join(directory, name))
        status = "FAIL" if problems else "ok  "
        if geometry:
            flash, ram = geometry
            summary = f"{flash // 1024} KiB flash / {ram // 1024} KiB RAM"
        else:
            summary = "unparsable name"
        print(f"[{status}] {name:26} {summary}")
        for problem in problems:
            failed = True
            print(f"        {problem}")

        if markdown and regions:
            print()
            print("| region | address | size |")
            print("| --- | --- | --- |")
            for region in REGIONS + ("RAM",):
                origin, length = regions[region]
                base = f"{origin:#010x}" if region != "RAM" else f"{origin:#010x}"
                print(f"| `{region}` | `{base}` | {length // 1024} KiB |")
            print()

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
