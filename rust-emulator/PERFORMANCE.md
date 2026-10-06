# Emulator performance TODOs

Implementation is paused here while other work takes priority. Resume with
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
