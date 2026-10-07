# Emulator performance

Speed is measured as **guest seconds per host second** ("x real time"):
1.0 is real time, below 1.0 the sound breaks up.

## This fork vs upstream

Measured side by side on 2026-10-07 with `examples/speed.rs`, one source file
built against upstream `main` (`81b9ed9`) and against this fork (`bcbe150`),
release builds, each firmware at its own clock. Median of 3 runs, upstream and
fork alternating. Each run is 4 guest seconds after 3 guest seconds of boot.

| Firmware (own clock) | State | Upstream | This fork | Speedup |
| --- | --- | ---: | ---: | ---: |
| Official FM-1_015 | idle | 0.047 | 0.270 | 5.7x |
| Official FM-1_015 | playing | 0.046 | 0.126 | 2.7x |
| Felucca 0.9-beta (360 MHz) | idle | 0.081 | 0.234 | 2.9x |
| Felucca 0.9-beta (360 MHz) | playing | 0.082 | 0.236 | 2.9x |
| Jangada 0.1-alpha | idle / playing | faults (unsupported instruction `0xee53`) | 0.235 / 0.235 | runs only here |
| Felucca 1.0.3 | idle / playing | faults at boot (unsupported instruction `0xeddc`) | 0.200 / 0.197 | runs only here |
| Melodee 0.10 | idle / playing | faults at boot (unsupported instruction `0xedd3`) | 0.231 / 0.230 | runs only here |
| X0X 0.10.1-beta | idle / playing | faults at boot (unsupported instruction `0xec20`) | 0.195 / 0.195 | runs only here |
| SLOOP 2.3 | idle / playing | does not boot (stuck, no interrupt and no audio after 400 M instructions) | 0.228 / 0.228 | runs only here |
| Optimist (`f4d854b`) | idle / playing | does not boot (same) | 2.17 / 1.84 | runs only here |

Upstream runs two of the eight firmwares tested; on those two the fork is 2.7x
to 5.7x faster. On the other six upstream faults on an unsupported instruction
or never boots (I did not investigate why; the fork's CPU additions are the
likely difference).

At a fixed 96 MHz instruction clock (`FM1_CPU_MHZ=96`; upstream has no such
option, so there is no upstream column), fork only, idle / playing:

| Firmware | x real time |
| --- | ---: |
| Official FM-1_015 | 0.323 / 0.206 |
| Felucca 0.9-beta | 0.848 / 0.852 |
| Jangada 0.1-alpha | 0.854 / 0.822 |
| Felucca 1.0.3 | 0.697 / 0.721 |
| Melodee 0.10 | 0.804 / 0.808 |
| X0X 0.10.1-beta | 0.806 / 0.792 |
| SLOOP 2.3 | 0.810 / 0.813 |
| Optimist | 2.37 / 2.07 |

"Playing" holds a three-key chord on the note keys for 0.15 s of every 0.3 s of
guest time. It is the same for every firmware, so it is a light load: for
most synths it changes nothing measurable (the idle and playing columns match).
The official firmware slows down when keys are held. The busy song below is the
realistic load.

### Where the gain comes from

Per-change breakdown on the fork, 2026-10-07, median of 3 runs, each firmware at
its own clock:

| Configuration | FM-1_015 idle | FM-1_015 playing | Felucca 0.9 idle | Felucca 0.9 playing |
| --- | ---: | ---: | ---: | ---: |
| Upstream (block cache and JIT on, the default) | 0.047 | 0.046 | 0.081 | 0.082 |
| Fork, one `step` call at a time (`FM1_STEP=1`): leaner interpreter, event-scheduled devices | 0.065 | 0.065 | 0.171 | 0.175 |
| Fork, `run_steps`, idle skip off | 0.074 | 0.075 | 0.237 | 0.239 |
| Fork, `run_steps`, spin skip off | 0.075 | 0.074 | 0.238 | 0.237 |
| Fork, block cache and JIT on | 0.226 | 0.107 | 0.187 | 0.183 |
| Fork, worker thread at the default class (`FM1_QOS=default`) | 0.262 | 0.127 | 0.231 | 0.234 |
| **Fork, everything as shipped** | **0.270** | **0.126** | **0.234** | **0.236** |

- **The leaner interpreter and event-scheduled devices** (the `step` row): 1.4x
  on the official firmware, 2.1x on Felucca. Boxed faults off the hot path,
  interrupt check skipped while nothing is pending, inlined guest memory
  accesses, a short per-instruction call chain, branch-free bundle merge.
  Devices advance when an event is due instead of on every instruction.
- **Idle skip and spin-loop skip** (`run_steps`): a halted core, or a core
  busy-waiting on a device, jumps to the next device event. Only the official
  firmware benefits: 0.065 to 0.270 idle. With
  either skip off alone it drops to 0.075, so on this firmware the two are
  needed together; I did not investigate why. Felucca never halts (0 slots
  jumped), so it gains only the `run_steps` loop itself, 0.171 to 0.234.
- **Block cache and JIT** are off by default in the fork and on in upstream. On
  these workloads turning them on in the fork is slower on Felucca (0.187 vs
  0.234) and on the official firmware (0.226 vs 0.270, 0.107 vs 0.126).
- **Scheduling class (QoS)**: 1 to 3 % here, within the run-to-run spread, at
  host load 7 to 13. It matters when the host is busy with other threads; this
  measurement does not show it.

### Does it still match?

`examples/speed.rs hash CALLS` runs `CALLS` `Cpu::step` calls and hashes PC,
registers, status registers, interrupt count and all of RAM.

- Felucca 0.9-beta: identical hash after 30 M and after 150 M calls on both builds.
- Jangada 0.1-alpha: identical after 30 M calls (upstream faults before 100 M).
- Official FM-1_015: after 150 M calls both builds have the same interrupt
  count (7283), audio frames (2040) and PC, but the instruction count differs by
  429 of 204,757,552 and so the hash differs. I did not investigate the cause.

## What still does not hold real time

- **A busy song.** `examples/scenarios/busy.steps` (three synth tracks, drums
  and FX on Optimist, 43.9 s of guest time, played by `bench` with
  `FM1_SCENARIO`) took 45.4 to 45.7 s at 96 MHz (0.96x) and 58.4 to 59.0 s at
  Optimist's own 360 MHz clock (0.75x), at host load 10 to 11. The sound breaks
  up at both. Upstream cannot run this firmware at all.
- **Every firmware but Optimist at its own clock.** They run at 0.13x to 0.27x
  (official 0.27 idle, 0.13 playing; Felucca 0.23). Even at a fixed 96 MHz the
  synths reach only 0.70x to 0.85x and the official firmware 0.21x to 0.32x
  (second table). I did not measure lower clocks.
- **Whole-firmware cost is the interpreter.** One native entry of the JIT still
  runs one guest instruction, so it does not pay off (see History).
- Not measured: the GUI's audio path (cpal queue, 70 ms pacing, texture
  uploads). `bench ... gui` covers the worker loop without the window or the
  sound device.

## Method

- **Date, machine:** 2026-10-07, Apple M4 Max (16 cores, 64 GB), macOS 27.0.1,
  rustc 1.99.0, release profile (`lto = "thin"`, `codegen-units = 1`).
- **Host load:** the Mac was not idle. One-minute load average during the
  runs: 5.9 to 10.6 (upstream-versus-fork table), 6.9 to 13.9 (breakdown and
  96 MHz), 10.0 to 11.2 (busy song). Each run is single-threaded; the host CPU
  time column of the same runs agreed with wall time within 1 to 3 %, and the
  three runs of each cell agreed within 6 %, so load did not distort the table. Quieter hosts will show larger absolute numbers.
- **Harness:** `examples/speed.rs`, the same source for both builds (the fork
  with `--cfg fm1_fork`). Guest time is counted in audio frames (44.1 kHz), a
  counter both builds have. Each build is driven as its own GUI worker drives
  it: upstream with a loop of `Cpu::step`, the fork with `Cpu::run_steps`.
  Upstream has no `bench` or `play_check`, so neither of those was used for the
  upstream comparison. The old `bench ... batch` figures are not comparable with
  these (they ran at 96 MHz and only the interpreter loop).
- **Firmwares:** official FM-1.fwsc (FM-1_015, sha256 `db1642b2...`),
  `felucca-0.9-beta`, `jangada-0.1-alpha` from `fm1-firmware`; Felucca 1.0.3,
  Melodee 0.10, X0X 0.10.1-beta, SLOOP 2.3 from `sloop/firmwares`; the Optimist
  build `optimist-0.1-dev-f4d854b-modified.fwsc`.
- **Reproduce:**
  - `scripts/speed-compare.sh UPSTREAM_CHECKOUT FIRMWARE.fwsc 3 4` for one firmware, both builds.
  - Fork only: `FM1_CPU_MHZ=96 target/release/examples/speed FIRMWARE idle 3`
    (built with `RUSTFLAGS="--cfg fm1_fork" cargo build --release --example speed`);
    breakdown switches `FM1_STEP=1`, `FM1_IDLE_SKIP=0`, `FM1_SPIN_SKIP=0`,
    `FM1_BLOCK_CACHE=1`, `FM1_QOS=default`.
  - Busy song: `FM1_CPU_MHZ=96 FM1_SCENARIO=examples/scenarios/busy.steps target/release/examples/bench OPTIMIST.fwsc 1000 batch`.
  - Hash: `speed FIRMWARE hash 150000000` on each build.
  - "Does not boot" was checked with `speed FIRMWARE hash 400000000`: upstream
    ends with 0 interrupts and 0 audio frames, the fork with more than 9000
    interrupts and 42,000 frames.

## Next steps

- [ ] Real time on the busy song and on every firmware at its own clock: the
  interpreter has to get faster. Profile the remaining hot paths (`FM1_HOT`).
- [ ] Batch multiple native guest instructions through the existing bounded
  `Cpu::run` loop. Start with single-core sequences of pure register operations
  that fit before the next device clock tick.
- [ ] End batches before memory/device accesses, control flow, interrupt
  delivery, idle, predicate/repeat boundaries or changes to core scheduling.
  Validate instruction words through the bus and stop before changed or
  unreadable code; preserve the original fault timing and XIP permissions.
- [ ] Preserve instruction limits, stop-PC checks, tracing and single-step
  behavior. Keep ARM64 and AMD64 calling conventions, executable-page ownership
  and interpreter fallback intact.
- [ ] Use bounded batches in the GUI worker while retaining frequent input and
  command polling, pause/restart behavior and USB serial output.
- [ ] Compare batched execution against single stepping: registers, PSR, PC,
  clock phase, interrupts, code mutations and fault boundaries.
- [ ] Measure button-to-LCD latency and the GUI audio path, not only throughput.
- [ ] Investigate the 429-instruction difference from upstream on the official
  firmware.

## History

An earlier experiment cached prepared basic blocks for both guest cores and
compiled hot register operations to ARM64 and x86-64 machine code, but each
native call still executed only one guest instruction before returning to the
scheduler. It did not produce a substantial speedup: for 400 million outer
`Cpu::step()` calls on a local Apple-silicon Mac, official firmware 20.133 s
before and 20.682 s with the JIT, Felucca 12.524 s and 12.450 s (single runs,
not re-measured; diagnostic output matched). The cache and JIT are therefore
off by default in the fork (`Cpu::set_block_cache`, `FM1_BLOCK_CACHE=1` in
`examples/bench`); upstream still enables them. Copied from the earlier notes,
not re-run: at `FM1_CPU_MHZ=96`, `bench ... batch`, block cache on and off,
Felucca 0.465 and 0.500, SLOOP drum kit 1.195 and 1.236, official firmware
0.188 and 0.200 (`bench ... batch` and `... hash` print the same hashes either
way).
