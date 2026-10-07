#!/bin/bash
# Upstream-versus-fork speed comparison with examples/speed.rs.
#   scripts/speed-compare.sh UPSTREAM_CHECKOUT FIRMWARE.fwsc [REPS] [GUEST_SECONDS]
# UPSTREAM_CHECKOUT is a worktree of upstream's main (its rust-emulator/ is
# used); this script's own checkout is the fork. The same examples/speed.rs
# is copied into the upstream checkout and built in release mode for both,
# the fork with --cfg fm1_fork. Prints guest seconds per host second for idle
# and playing, alternating upstream and fork. Check `uptime` first: the
# numbers are only comparable when the host is not busy.
set -e
HERE=$(cd "$(dirname "$0")/.." && pwd)
UP=$(cd "$1/rust-emulator" && pwd)
FW=$2
REPS=${3:-3}
SECS=${4:-4}
cp "$HERE/examples/speed.rs" "$UP/examples/speed.rs"
(cd "$UP" && cargo build --release --example speed)
(cd "$HERE" && RUSTFLAGS="--cfg fm1_fork" cargo build --release --example speed)
uptime
for rep in $(seq 1 "$REPS"); do
  for mode in idle play; do
    echo "upstream $mode: $("$UP/target/release/examples/speed" "$FW" "$mode" "$SECS" 2>&1)"
    echo "fork     $mode: $("$HERE/target/release/examples/speed" "$FW" "$mode" "$SECS" 2>&1)"
  done
done
