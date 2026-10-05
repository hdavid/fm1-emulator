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

## Time with both cores running (feat/dual-core-emu)

Both cores run on one clock. A step that executes an instruction on each core
now advances guest time once: `cpu.steps` is guest time, `cpu.core_steps`
counts each core's instructions (`diagnose` prints both, `play_check cores`).
Before, the secondary's instructions also advanced time, so with CPU1 running
every timer, the audio DMA and the firmware's own CPU meter ran twice as fast
per primary instruction. When CPU0 is paused (C0_CON stopped bit) the
secondary alone carries time. Stock FM-1 / Baud Girl therefore do twice the
CPU0 work per unit of guest time than before; their 200M-step counters changed
(they still boot to their home screens), the single-core firmwares' did not.

The SLOOP dual-core prototype (sloop `feat/dual-core`, docs/DUAL-CORE.md)
renders two synth parts on CPU1 per audio block; in this model it cut the
CPU0 audio load by 40-44 % with bit-identical output. The model has no cycle,
cache, wait-state or bus-contention costs, so hardware will gain less.
