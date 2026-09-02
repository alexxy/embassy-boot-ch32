#!/usr/bin/env bash
# Checks that a firmware ELF fits into the partition it is linked against, that
# it starts where `partition-map/ch32v305rbt6.x` says it should, and optionally
# writes a raw binary image next to it.
#
# The LLVM tools come from the active rustup toolchain (`llvm-tools` component);
# set LLVM_SIZE / LLVM_READELF / LLVM_OBJCOPY to use some other build.
#
# Intended to be used from CI (it emits `::error::` annotations and appends a
# row to $GITHUB_STEP_SUMMARY) but it is a plain script, so it also runs fine
# locally.

set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: check_size.sh --elf <file> --partition <name> --limit <bytes>
                     [--entry <hex-addr>] [--bin <out.bin>]

  --elf        path to the ELF produced by `cargo build`
  --partition  partition name, only used in messages
  --limit      largest allowed flash footprint (text + data), in bytes
  --entry      expected ELF entry point, e.g. 0x08004000
  --bin        also write an image in the binary format to this path
EOF
    exit 2
}

elf=""
partition=""
limit=""
entry=""
bin=""

while [ $# -gt 0 ]; do
    case "$1" in
        --elf) elf="$2"; shift 2 ;;
        --partition) partition="$2"; shift 2 ;;
        --limit) limit="$2"; shift 2 ;;
        --entry) entry="$2"; shift 2 ;;
        --bin) bin="$2"; shift 2 ;;
        -h | --help) usage ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            ;;
    esac
done

if [ -z "$elf" ] || [ -z "$partition" ] || [ -z "$limit" ]; then
    usage
fi

if [ ! -f "$elf" ]; then
    echo "::error::$partition: no ELF at $elf (did the build run?)" >&2
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
LLVM_READELF="${LLVM_READELF:-$bindir/llvm-readelf}"
LLVM_OBJCOPY="${LLVM_OBJCOPY:-$bindir/llvm-objcopy}"

for tool in "$LLVM_SIZE" "$LLVM_READELF" "$LLVM_OBJCOPY"; do
    if [ ! -x "$tool" ]; then
        echo "::error::$tool not found; run 'rustup component add llvm-tools'" >&2
        exit 1
    fi
done

# Berkeley format: "text data bss dec hex filename". `text` covers everything
# allocated in flash except the initialized data (`.data`), so the footprint a
# partition has to hold is text + data. `.bss` is RAM only.
size_line="$("$LLVM_SIZE" -B "$elf" | awk 'NR==2')"
text="$(printf '%s\n' "$size_line" | awk '{print $1}')"
data="$(printf '%s\n' "$size_line" | awk '{print $2}')"
bss="$(printf '%s\n' "$size_line" | awk '{print $3}')"
flash=$((text + data))

echo "$partition: flash $flash bytes (text $text + data $data), RAM .bss $bss bytes, limit $limit"

status=0

if [ "$flash" -gt "$limit" ]; then
    echo "::error::$partition overflowed by $((flash - limit)) bytes ($flash > $limit)" >&2
    status=1
else
    printf '%s: %d%% of the partition used, %d bytes to spare\n' \
        "$partition" "$((100 * flash / limit))" "$((limit - flash))"
fi

if [ -n "$entry" ]; then
    actual="$("$LLVM_READELF" -h "$elf" |
        sed -n 's/.*Entry point address:[[:space:]]*0x\([0-9a-fA-F]*\).*/\1/p')"
    if [ -z "$actual" ]; then
        echo "::error::$partition: could not read the entry point of $elf" >&2
        status=1
    elif [ "$((16#$actual))" -ne "$((16#${entry#0x}))" ]; then
        # A wrong entry point almost always means memory.x was not picked up.
        echo "::error::$partition: entry point is 0x$actual, expected $entry (is the linker script on the search path?)" >&2
        status=1
    else
        echo "$partition: entry point $entry"
    fi
fi

if [ -n "$bin" ]; then
    "$LLVM_OBJCOPY" -O binary "$elf" "$bin"
    echo "wrote $bin ($(wc -c <"$bin") bytes)"
fi

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        printf '| example | partition | flash | .bss | limit | entry |\n'
        printf '| --- | --- | --- | --- | --- | --- |\n'
        printf '| %s | %s | %d | %d | %d | %s |\n' \
            "${example:-$partition}" "$partition" "$flash" "$bss" "$limit" "${entry:-n/a}"
    } >>"${GITHUB_STEP_SUMMARY}"
fi

exit "$status"
