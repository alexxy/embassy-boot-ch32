#!/usr/bin/env bash
# Checks that a firmware ELF fits into the partition it is linked against and
# that it starts where the partition map says it should. Optionally writes a raw
# binary image next to it.
#
# The limits are *derived from the ELF itself*, from the `__bootloader_*`
# absolute symbols that the partition map defines, so CI never has to keep a
# second copy of the geometry in sync with `partition-map`:
#
#   role=bootloader   limit = __bootloader_active_start            entry = flash base
#   role=application  limit = __bootloader_active_end
#                               - __bootloader_active_start         entry = flash base
#                                                                     + active_start
#
# The LLVM tools come from the active rustup toolchain (`llvm-tools` component);
# set LLVM_SIZE / LLVM_NM / LLVM_READOBJ / LLVM_OBJDUMP / LLVM_OBJCOPY to use
# some other build.
#
# The entry point is read with whichever of `llvm-readobj` or `llvm-objdump` is
# available: the rustup `llvm-tools` component ships those but *not*
# `llvm-readelf`.
#
# Intended to be used from CI (it emits `::error::` annotations and appends a
# row to $GITHUB_STEP_SUMMARY) but it is a plain script, so it also runs fine
# locally.

set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: check_size.sh --elf <file> --role {bootloader,application}
                     [--label <name>] [--bin <out.bin>]
                     [--limit <bytes>] [--entry <hex-addr>] [--flash-base <hex>]

  --elf          path to the ELF produced by `cargo build`
  --role         bootloader (must fit below the active partition) or
                 application (must fit inside it); selects which symbols are
                 used to derive --limit and --entry
  --label        name used in messages and in the job summary
                 (default: the ELF file name)
  --bin          also write an image in the binary format to this path
  --limit        override the derived flash footprint limit, in bytes
  --entry        override the derived expected entry point, e.g. 0x08004000
  --flash-base   flash origin the map uses (default: 0x08000000)
EOF
    exit 2
}

elf=""
role=""
label=""
limit=""
entry=""
bin=""
flash_base="0x08000000"

while [ $# -gt 0 ]; do
    case "$1" in
        --elf) elf="$2"; shift 2 ;;
        --role) role="$2"; shift 2 ;;
        --label) label="$2"; shift 2 ;;
        --limit) limit="$2"; shift 2 ;;
        --entry) entry="$2"; shift 2 ;;
        --bin) bin="$2"; shift 2 ;;
        --flash-base) flash_base="$2"; shift 2 ;;
        -h | --help) usage ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            ;;
    esac
done

if [ -z "$elf" ] || [ -z "$role" ]; then
    usage
fi

case "$role" in
    bootloader | application) ;;
    *)
        echo "unknown role: $role" >&2
        usage
        ;;
esac

[ -n "$label" ] || label="$(basename "$elf")"

if [ ! -f "$elf" ]; then
    echo "::error::$label: no ELF at $elf (did the build run?)" >&2
    exit 1
fi

# The binutils shipped with the toolchain live next to the linker driver.
llvm_bindir() {
    local sysroot host
    sysroot="$(rustc --print sysroot)"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    printf '%s/lib/rustlib/%s/bin' "$sysroot" "$host"
}

bindir="$(llvm_bindir)"
LLVM_SIZE="${LLVM_SIZE:-$bindir/llvm-size}"
LLVM_NM="${LLVM_NM:-$bindir/llvm-nm}"
LLVM_READOBJ="${LLVM_READOBJ:-$bindir/llvm-readobj}"
LLVM_OBJDUMP="${LLVM_OBJDUMP:-$bindir/llvm-objdump}"
LLVM_OBJCOPY="${LLVM_OBJCOPY:-$bindir/llvm-objcopy}"

for tool in "$LLVM_SIZE" "$LLVM_NM" "$LLVM_OBJCOPY"; do
    if [ ! -x "$tool" ]; then
        echo "::error::$tool not found; run 'rustup component add llvm-tools'" >&2
        exit 1
    fi
done

# Prints the value of a global absolute symbol as bare hex (no 0x), or nothing
# when the symbol is missing.
symbol() {
    "$LLVM_NM" -g --defined-only "$elf" |
        awk -v name="$1" '$3 == name { print $1; exit }'
}

status=0

active_start="$(symbol __bootloader_active_start)"
active_end="$(symbol __bootloader_active_end)"

if [ -z "$active_start" ] || [ -z "$active_end" ]; then
    echo "::error::$label: __bootloader_active_start/__bootloader_active_end are missing; the linker did not pick up the partition map (is it on the linker search path?)" >&2
    exit 1
fi

if [ -z "$limit" ] || [ -z "$entry" ]; then
    if [ "$role" = bootloader ]; then
        derived_limit="$((16#$active_start))"
        derived_entry="$(printf '0x%08x' "$((16#${flash_base#0x}))")"
    else
        derived_limit="$((16#$active_end - 16#$active_start))"
        derived_entry="$(printf '0x%08x' "$((16#${flash_base#0x} + 16#$active_start))")"
    fi
    [ -n "$limit" ] || limit="$derived_limit"
    [ -n "$entry" ] || entry="$derived_entry"
fi

active_addr="$(printf '0x%08x' "$((16#${flash_base#0x} + 16#$active_start))")"
if [ "$role" = bootloader ]; then
    what="the bootloader partition at the start of flash, below $active_addr"
else
    what="the active partition at $active_addr"
fi

# Prints the ELF entry point as bare hex (no 0x), or nothing when no ELF reader
# is available or the output cannot be parsed.
elf_entry_point() {
    if [ -x "$LLVM_READOBJ" ]; then
        "$LLVM_READOBJ" --file-headers "$elf" |
            sed -n 's/.*Entry:[[:space:]]*0x\([0-9a-fA-F]*\).*/\1/p'
    elif [ -x "$LLVM_OBJDUMP" ]; then
        # llvm-objdump spells it "start address", not "Entry".
        "$LLVM_OBJDUMP" -f "$elf" |
            sed -n 's/.*start address:[[:space:]]*0x\([0-9a-fA-F]*\).*/\1/p'
    fi
}

# Berkeley format: "text data bss dec hex filename". `text` covers everything
# allocated in flash except the initialized data (`.data`), so the footprint a
# partition has to hold is text + data. `.bss` is RAM only.
size_line="$("$LLVM_SIZE" -B "$elf" | awk 'NR==2')"
text="$(printf '%s\n' "$size_line" | awk '{print $1}')"
data="$(printf '%s\n' "$size_line" | awk '{print $2}')"
bss="$(printf '%s\n' "$size_line" | awk '{print $3}')"
flash=$((text + data))

echo "$label ($role): must fit $what, limit $limit bytes"
echo "$label: flash $flash bytes (text $text + data $data), RAM .bss $bss bytes"

if [ "$flash" -gt "$limit" ]; then
    echo "::error::$label overflowed $what by $((flash - limit)) bytes ($flash > $limit)" >&2
    status=1
else
    printf '%s: %d%% used, %d bytes to spare\n' \
        "$label" "$((100 * flash / limit))" "$((limit - flash))"
fi

actual="$(elf_entry_point)"
if [ -z "$actual" ]; then
    echo "::error::$label: could not read the entry point of $elf (tried llvm-readobj and llvm-objdump)" >&2
    status=1
elif [ "$((16#$actual))" -ne "$((16#${entry#0x}))" ]; then
    # A wrong entry point almost always means memory.x was not picked up.
    echo "::error::$label: entry point is 0x$actual, expected $entry (is the partition map on the linker search path?)" >&2
    status=1
else
    echo "$label: entry point $entry"
fi

if [ -n "$bin" ]; then
    "$LLVM_OBJCOPY" -O binary "$elf" "$bin"
    echo "wrote $bin ($(wc -c <"$bin") bytes)"
fi

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        printf '| firmware | role | flash | .bss | limit | entry |\n'
        printf '| --- | --- | --- | --- | --- | --- |\n'
        printf '| %s | %s | %d | %d | %d | %s |\n' \
            "$label" "$role" "$flash" "$bss" "$limit" "$entry"
    } >>"${GITHUB_STEP_SUMMARY}"
fi

exit "$status"
