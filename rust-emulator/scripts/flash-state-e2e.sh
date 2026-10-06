#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# End-to-end check of the flash state (src/flash_state.rs) with a real firmware:
#   scripts/flash-state-e2e.sh FIRMWARE.fwsc [OUT_DIR]
# Session 1 boots from the package alone (--fresh), turns KNOB1 five detents,
# then waits 30 s of guest time (autosaving firmware saves when stopped, quiet
# and idle; Optimist and SLOOP at most every 20 s) and saves the state. Session 2
# starts from that state. The screens at the end of session 1 and after the
# restore are compared, and both against a boot without the state.
# FM1_CPU_MHZ (play_check) sets the clock; 96 keeps it quick.
set -eu
firmware=${1:?usage: flash-state-e2e.sh FIRMWARE.fwsc [OUT_DIR]}
out=${2:-$(mktemp -d)}
here=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$out"
cargo build -q --release --example play_check --manifest-path "$here/Cargo.toml"
play="$here/target/release/examples/play_check"
export FM1_CPU_MHZ="${FM1_CPU_MHZ:-96}"
echo "== session 1: change, wait past the autosave, quit"
"$play" --state "$out/state/" --fresh "$firmware" run:3 png:"$out/boot.png" turn:KNOB1:5 run:30 png:"$out/session1.png" | grep "flash state"
echo "== session 2: start from the saved state"
"$play" --state "$out/state/" "$firmware" run:3 png:"$out/restored.png" | grep "flash state"
cat "$out/state/"*.index
if cmp -s "$out/session1.png" "$out/restored.png"; then
    echo "PASS: the restored screen is the screen session 1 ended on ($out)"
elif cmp -s "$out/boot.png" "$out/restored.png"; then
    echo "FAIL: the restored screen is a fresh boot's ($out)"
    exit 1
else
    echo "DIFFERENT: the restored screen is neither session 1's nor a fresh boot's: compare $out/*.png"
    exit 2
fi
