# Emulator performance TODOs

Native block batching (below) is paused while other work takes priority; the
interpreter itself has been sped up since (see "Since then"). Resume with
native block batching, then use measurements to choose further JIT work.

## Current checkpoint

Prepared basic blocks are cached for both guest cores. Hot register operations
compile to ARM64 and x86-64 machine code, but each native call still executes
only one guest instruction before returning to the scheduler. This preserves
instruction boundaries but has not produced a substantial firmware speedup.

Local macOS ARM64 measurements for 400 million outer `Cpu::step()` calls:

| Firmware | Before block caching/JIT | Current JIT |
| --- | ---: | ---: |
| Official firmware | 20.133 s | 20.682 s |
| Felucca | 12.524 s | 12.450 s |

These are single-run throughput samples, not input-to-screen latency
measurements. Diagnostic output matched between each pair. The existing full
boot regressions passed for local Felucca, published Felucca and official
firmware, with unchanged execution and peripheral counts.

The block cache and JIT are now off by default (`Cpu::set_block_cache`,
`FM1_BLOCK_CACHE=1` in `examples/bench`). `bench FIRMWARE 200000000 batch`
at `FM1_CPU_MHZ=96`, median of three runs, guest seconds per host second:

| Firmware | Block cache + JIT | Off |
| --- | ---: | ---: |
| Felucca 0.9-beta | 0.465 | 0.500 |
| SLOOP drum kit build | 1.195 | 1.236 |
| Official firmware | 0.188 | 0.200 |

`bench ... batch` and `bench ... hash` print the same state, SRAM, audio and
LCD hashes either way.

## Since then: the fork's interpreter and scheduling work

Commits on this branch (see `git log`): event-scheduled devices and idle skip,
boxed faults (`bffa4f5`), interrupt check skipped while nothing is pending
(`c0fdbbd`), inlined guest memory accesses (`449319c`), a short per-instruction
call chain (`ceba3fe`), branch-free bundle merge (`fb54dfa`), spin-loop skip
(`6a6d075`), the GUI worker at the main thread's scheduling class (`0f356ba`).

`bench FIRMWARE 200000000 batch` at `FM1_CPU_MHZ=96`, guest seconds per host
second, block cache off. Copied from the maintainer's notes of 2026-10-06
(their upstream-PR notes, section 10), measured on an
Apple-silicon Mac under load; not re-run for this documentation pass:

| Firmware | At the block-cache-off commit (`014a4a8`) | After `ceba3fe` |
| --- | ---: | ---: |
| Felucca 0.9-beta | 0.500 | 0.83 |
| SLOOP drum kit build | 1.236 | 2.08 |
| Official firmware | 0.200 | 0.273 |

Stock firmware 0.27 -> 0.35 after the spin-loop skip. A busy song
(`examples/scenarios/busy.steps` drives three synth tracks, drums and FX) on a
Mac at host load about 6 was reported at 0.6-0.98x of real time, so such songs
still do not play in real time; this is the maintainer's measurement, recorded
here without a re-run.

Not yet measured: the GUI's audio path (cpal queue, 70 ms pacing, texture
uploads). `bench ... gui` covers the worker loop (run_steps batches, audio
drain, LCD copy) without the window or the sound device.

## Next steps, in order

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
  clock phase, interrupts, code mutations and fault boundaries. Run the unchanged
  firmware regressions through the batched path as well.
- [ ] Measure both firmware throughput and button-to-LCD latency. Compare with
  the current engine under the same workload before claiming a speedup.
- [ ] Profile the remaining hot paths, broaden native translation where it
  helps, and then investigate batching with both guest cores active while
  preserving their scheduling and shared-device behavior.
