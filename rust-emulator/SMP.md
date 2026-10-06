# Stock firmware secondary CPU handoff

The unchanged stock application at 02059a2e writes its secondary entry
020001b8 to the SPL RAM vector at 01c7fff8, enables C1_CON bit 3 and clears
reset bit 1. It then waits for byte 01c22328 to become nonzero.

The emulator starts a separate register/stack/interrupt context from that
guest-written vector. The secondary executes the actual handoff instructions,
returns from supervisor mode to the copied RAM routine at 01c02428, and writes
the acknowledgement byte itself. No readiness byte is manufactured by the host.
Both supplied packages use this sequence. Interrupt configurations and tick
timers have separate banks for each CPU. Software interrupt latches are shared:
the stock caller sets bit 4 via CPU 0's ILAT_SET, the secondary handles source
124, and its guest handler clears the same latch through CPU 0's ILAT_CLR.
Pending reads expose software sources enabled in that core's mask. Their
memory, flash and peripherals share one bus.

Execution alternates cores at instruction boundaries. LOCKSET serializes CPU
bus ownership until LOCKCLR. This is functional scheduling; relative clock
phases, pipeline timing, nested interrupt preemption and reset ROM execution
are not modeled. The secondary begins at the SPL handoff in supervisor context.
The existing public primary register view remains CPU 0; instruction and IRQ
counters include work on both cores. A secondary fault reports its actual PC,
while the diagnostic runner's register dump still shows CPU 0.

Regression checks cover guest handoff selection, CNUM, shared-bus ownership,
reset, shared software latch acknowledgement, and independent interrupt masks. Existing
single-core raw/ELF and display-package boot checks remain applicable.

The stock flash exclusion routine pauses the other core with Cx_CON bit 2,
waits for stopped bit 4, and resumes with bit 3. The functional model stops
instruction execution while preserving the context and completes these
commands at bundle boundaries. This behavior is inferred from the unchanged
stock caller and handler, rather than measured stop latency. It lets the
guest complete its own mailbox acknowledgement and enter SPI flash operations.

Hardware evidence from the compatible diagnostic: with software sources
disabled, pending reads return zero after ILAT_SET. With all eight enabled
on CPU 0, setting each bit 0..7 exposes IPND3 bits 24..31 respectively; other
pending words stay zero. CPU 1 remained in reset and its registers returned
zero, so that capture does not validate cross-core visibility. A later probe
attempting to start CPU 1 reset before returning results and was discarded;
the USB updater recovered and FM-1_981 display firmware was restored.

## Spin loops

Stock FM-1 rarely lets the idle skip engage: while CPU0 sleeps, CPU1 polls
its job mailbox (0x01c0247e.., with a call to 0x01c01c22 that resets TIMER5's
counter and reads it back). `src/spin.rs` records such a loop (the core's
state before every instruction and its data accesses) and accepts it only as
a fixed point: a pass returns to the head state, reads and stores the same
memory values, and touches no device other than TIMER4/5 counter writes and
reads. `Cpu::skip_idle_calls` then also jumps spans in which every issuing
core is halted or in such a loop: the timer accesses are replayed at the
oscillator tick each would run at (the fractional instruction clock), a
value no recorded pass read or a timer reaching its period ends the span,
and the core is left in the recorded state it reaches. `spin_skip = false`
(`FM1_SPIN_SKIP=0` in bench, diagnose and play_check) turns it off;
`FM1_SPIN_LOG=1` lists the loops found and rejected. Final state, SRAM,
audio and LCD hashes are identical either way.
