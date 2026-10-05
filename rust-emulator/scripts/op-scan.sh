#!/usr/bin/env bash
# Scan firmware for pi32v2 instructions the interpreter does not decode
# (examples/op_scan.rs), with JieLi's own objdump as the reference.
#
#   scripts/op-scan.sh [--out DIR] FIRMWARE...     .elf, .fwsc or raw app image
#
# ELFs are disassembled as they are (functions from STT_FUNC symbols); other
# inputs are extracted (examples/extract) and the application image is
# disassembled whole, wrapped in an object at offset 0 (op_scan adds the
# 0x02000120 load address). Reports go to stdout; with --out, the listing,
# report (.md) and per-instruction findings (.tsv) are kept in DIR.
# Needs the JieLi toolchain (JIELI_TOOLCHAIN, as scripts/pi32-objdump.sh) and
# Docker (Rancher Desktop).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
TC="${JIELI_TOOLCHAIN:-$(ls -d "$HOME"/.jieli/jieli-linux-toolchains-* 2>/dev/null | sort | tail -1)}"
[ -x "$TC/common/bin/objdump" ] || { echo "op-scan: no JieLi toolchain at '$TC'" >&2; exit 1; }
out=""
if [ "${1:-}" = "--out" ]; then
    out="$2"
    shift 2
    mkdir -p "$out"
fi
[ $# -gt 0 ] || { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
(cd "$here" && cargo build --release --quiet --example op_scan --example extract)
bin="$here/target/release/examples"
work="$(mktemp -d "${TMPDIR:-/tmp}/opscan.XXXXXX")"
trap 'rm -rf "$work"' EXIT

objdump_in_docker() { # DIR COMMAND
    docker run --rm --platform linux/amd64 -v "$1:/work" -v "$TC:/opt/jieli:ro" -w /work \
        debian:bookworm-slim sh -c "$2"
}

for firmware in "$@"; do
    name="$(basename "$firmware")"
    name="${name%.*}"
    mkdir "$work/$name"
    dir="$work/$name"
    if [ "$(head -c 4 "$firmware" | od -An -c | tr -d ' ')" = "177ELF" ]; then
        cp "$firmware" "$dir/in.elf"
        objdump_in_docker "$dir" "/opt/jieli/common/bin/objdump -d in.elf > listing.dis"
        base=0
    else
        "$bin/extract" "$firmware" "$dir/app.bin" 2>/dev/null
        printf '\t.text\n\t.globl image\nimage:\n\t.incbin "app.bin"\n' >"$dir/image.s"
        objdump_in_docker "$dir" "/opt/jieli/common/bin/clang -target pi32v2 -c image.s -o image.o && /opt/jieli/common/bin/objdump -d image.o > listing.dis"
        base=02000120
    fi
    if [ -n "$out" ]; then
        cp "$dir/listing.dis" "$out/$name.dis"
        "$bin/op_scan" "$firmware" "$dir/listing.dis" --base "$base" --tsv "$out/$name.tsv" | tee "$out/$name.md"
    else
        "$bin/op_scan" "$firmware" "$dir/listing.dis" --base "$base"
    fi
    echo
done
