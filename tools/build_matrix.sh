#!/usr/bin/env bash
# Builds the bootloader and the application example for every chip in
# `examples/chips.rs` (or a subset of them) and checks each result against the
# partition map it was linked against with `tools/check_size.sh`.
#
# The chip table is parsed from `examples/chips.rs`, so this script cannot drift
# away from what the examples actually support.
#
# Examples:
#
#   tools/build_matrix.sh                       # everything, 42 builds
#   tools/build_matrix.sh --example bootloader  # only the bootloader
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
bin_dir=""
chips=()

usage() {
    cat >&2 <<'EOF'
usage: build_matrix.sh [--example {bootloader,application,all}]
                       [--chip <part>] [--list] [--bin-dir <dir>]

  --example      which example to build (default: all)
  --chip         only build this part; repeatable (default: every part)
  --list         print the chip table and exit
  --bin-dir      write raw binary images to this directory, named
                 <part>-<example>.bin
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

# Turns `examples/chips.rs` into `part target` lines, one per selectable part.
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
            print part, target
            part = ""
        }
    ' "$chips_rs"
}

if [ -n "${want_list:-}" ]; then
    printf '%-18s %s\n' PART TARGET
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

while read -r part target; do
    selected "$part" || continue

    for example in bootloader application; do
        case "$example" in
            bootloader) [ "$want_bootloader" = 1 ] || continue ;;
            application) [ "$want_application" = 1 ] || continue ;;
        esac

        label="$part-$example"
        echo "====================================================================="
        echo "building $label for $target"

        # The CH32V3 line has no built-in rustc target, so its spec is passed as
        # the JSON file that lives next to the example.
        target_arg="$target"
        if [ -f "$root/examples/$example/$target.json" ]; then
            target_arg="$target.json"
        fi

        args=(build --release --no-default-features "--features" "$part" "--target" "$target_arg")
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
done < <(chip_table)

echo "====================================================================="
echo "built $built firmware images"

if [ "${#failures[@]}" -gt 0 ]; then
    printf '::error::failed: %s\n' "${failures[*]}"
    exit 1
fi
echo "all builds fit their partitions"
