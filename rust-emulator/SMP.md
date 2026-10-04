# Stock firmware secondary CPU handoff

The unchanged stock application at 02059a2e writes its secondary entry
020001b8 to the SPL RAM vector at 01c7fff8, enables C1_CON bit 3 and clears
reset bit 1. It then waits for byte 01c22328 to become nonzero.

The emulator starts a separate register/stack/interrupt context from that
guest-written vector. The secondary executes the actual handoff instructions,
returns from supervisor mode to the copied RAM routine at 01c02428, and writes
the acknowledgement byte itself. No readiness byte is manufactured by the host.
Both supplied packages use this sequence. Interrupt configurations, software
interrupt latches and tick timers have separate banks for each CPU. Their
memory, flash and peripherals share one bus.

Execution alternates cores at instruction boundaries. LOCKSET serializes CPU
bus ownership until LOCKCLR. This is functional scheduling; relative clock
phases, pipeline timing, nested interrupt preemption and reset ROM execution
are not modeled. The secondary begins at the SPL handoff in supervisor context.
The existing public primary register view remains CPU 0; instruction and IRQ
counters include work on both cores. A secondary fault reports its actual PC,
while the diagnostic runner's register dump still shows CPU 0.

Regression checks cover guest handoff selection, CNUM, shared-bus ownership,
reset, and independently masked/acknowledged software interrupts. Existing
single-core raw/ELF and display-package boot checks remain applicable.
