#!/usr/bin/env bash
# Builds the bootloader and the application example for every chip in
# `examples/chips.rs` (or a subset of them) and checks each result against the
# partition map it was linked against with `tools/check_size.sh`.
#
# The chip table is parsed from `examples/chips.rs`, so this script cannot drift
# away from what the examples actually support.
#
# Each part is built with the serial transport; parts that have a `-usb`
# partition map in `examples/chips.rs` are additionally built with the USB DFU
# transport (bootloader `transport-usb`, application `usb-dfu`).
#
# Examples:
#
#   tools/build_matrix.sh                       # everything, both transports
#   tools/build_matrix.sh --example bootloader  # only the bootloader
#   tools/build_matrix.sh --transport usb       # only the USB DFU builds
#   tools/build_matrix.sh --chip ch32v203c8t6 --chip ch32v307rct6
#   tools/build_matrix.sh --bin-dir dist        # also write .bin images
#
# Set CARGO_ARGS to pass extra flags to every `cargo build` (for example
# `CARGO_ARGS=--offline`).

set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
chips_rs="$root/examples/chips.rs"

want_bootloader=1
want_application=1
want_all_chips=1
want_transport=all
bin_dir=""
chips=()

usage() {
    cat >&2 <<'EOF'
usage: build_matrix.sh [--example {bootloader,application,all}]
                       [--transport {uart,usb,all}]
                       [--chip <part>] [--list] [--bin-dir <dir>]

  --example      which example to build (default: all)
  --transport    which update transport to build (default: all; parts without
                 a `-usb` partition map only have uart)
  --chip         only build this part; repeatable (default: every part)
  --list         print the chip table and exit
  --bin-dir      write raw binary images to this directory, named
                 <part>-<example>.bin for the serial builds and
                 <part>-<example>-usb.bin for the USB DFU ones
EOF
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --example)
            want_bootloader=0
            want_application=0
            case "$2" in
                bootloader) want_bootloader=1 ;;
                application) want_application=1 ;;
                all)
                    want_bootloader=1
                    want_application=1
                    ;;
                *) usage ;;
            esac
            shift 2
            ;;
        --transport)
            case "$2" in
                uart | usb | all) want_transport="$2" ;;
                *) usage ;;
            esac
            shift 2
            ;;
        --chip) chips+=("$2"); shift 2 ;;
        --list) want_list=1; shift ;;
        --bin-dir) bin_dir="$2"; shift 2 ;;
        -h | --help) usage ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            ;;
    esac
done

if [ ! -f "$chips_rs" ]; then
    echo "no chip table at $chips_rs" >&2
    exit 1
fi

# Turns `examples/chips.rs` into `part target usb-map` lines, one per
# selectable part. The USB map column is empty for parts that cannot host a
# USB bootloader (`map_usb: ""`). `map_usb` is the last field of the struct
# literal, which is where each line is emitted.
chip_table() {
    awk '
        /^const [A-Za-z0-9_]+: &str = "/ {
            name = $2
            sub(/:/, "", name)
            value = $0
            sub(/^.*&str = "/, "", value)
            sub(/".*/, "", value)
            consts[name] = value
        }
        /part: "/ {
            line = $0
            sub(/^.*part: "/, "", line)
            sub(/".*/, "", line)
            part = line
        }
        /target: / && part != "" {
            line = $0
            sub(/^.*target: /, "", line)
            sub(/,.*/, "", line)
            target = (line in consts) ? consts[line] : line
        }
        /map_usb: / && part != "" {
            line = $0
            sub(/^.*map_usb: /, "", line)
            sub(/,.*/, "", line)
            gsub(/[ "]/, "", line)
            usbmap = (line == "") ? "" : ((line in consts) ? consts[line] : line)
            print part, target, usbmap
            part = ""
        }
    ' "$chips_rs"
}

if [ -n "${want_list:-}" ]; then
    printf '%-18s %-31s %s\n' PART TARGET USB_MAP
    chip_table
    exit 0
fi

selected() {
    if [ "${#chips[@]}" -eq 0 ]; then
        return 0
    fi
    local want="$1" chip
    for chip in "${chips[@]}"; do
        [ "$chip" = "$want" ] && return 0
    done
    return 1
}

if [ -n "$bin_dir" ]; then
    mkdir -p "$bin_dir"
fi

failures=()
built=0

while read -r part target usbmap; do
    selected "$part" || continue

    for transport in uart usb; do
        [ "$want_transport" = all ] || [ "$want_transport" = "$transport" ] || continue
        if [ "$transport" = usb ] && [ -z "$usbmap" ]; then
            echo "skipping $part usb: no ch32-hal USB driver or too little flash"
            continue
        fi

    for example in bootloader application; do
        case "$example" in
            bootloader) [ "$want_bootloader" = 1 ] || continue ;;
            application) [ "$want_application" = 1 ] || continue ;;
        esac

        label="$part-$example"
        # The serial builds keep the plain <part>-<example> name; the USB DFU
        # ones are distinguishable by their -usb suffix.
        [ "$transport" = usb ] && label="$label-usb"

        case "$example/$transport" in
            bootloader/uart) features="$part,transport-uart" ;;
            bootloader/usb) features="$part,transport-usb" ;;
            application/uart) features="$part" ;;
            application/usb) features="$part,usb-dfu" ;;
        esac

        echo "====================================================================="
        echo "building $label ($transport) for $target"

        # The CH32V3 line has no built-in rustc target, so its spec is passed as
        # the JSON file that lives next to the example.
        target_arg="$target"
        if [ -f "$root/examples/$example/$target.json" ]; then
            target_arg="$target.json"
        fi

        args=(build --release --no-default-features "--features" "$features" "--target" "$target_arg")
        # shellcheck disable=SC2206
        [ -n "${CARGO_ARGS:-}" ] && args+=(${CARGO_ARGS})

        if ! (cd "$root/examples/$example" && cargo "${args[@]}"); then
            failures+=("$label (build)")
            continue
        fi

        elf="$root/examples/$example/target/$target/release/$example"
        bin=()
        [ -n "$bin_dir" ] && bin=(--bin "$bin_dir/$label.bin")

        if ! "$root/tools/check_size.sh" --elf "$elf" --role "$example" \
            --label "$label" "${bin[@]}"; then
            failures+=("$label (size)")
        fi
        built=$((built + 1))
    done
    done
done < <(chip_table)

echo "====================================================================="
echo "built $built firmware images"

if [ "${#failures[@]}" -gt 0 ]; then
    printf '::error::failed: %s\n' "${failures[*]}"
    exit 1
fi
echo "all builds fit their partitions"
