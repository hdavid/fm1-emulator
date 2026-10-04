# Rust FM-1 emulator

Rust 1.91.1 is managed by the root `mise.toml`. The interpreter has no external
Rust dependencies and executes vendor-built pi32v2 machine code.

```sh
mise run rust-test
mise run rust-probe
mise run rust-boot
```

The loader accepts an application `.bin` mapped at `0x02000120`, or an executable
ELF32-pi32v2. ELF flash load addresses reconstruct the exact application `.bin`,
including the initializers for RAM code and data. Startup performs the RAM
copies; the loader does not move them early. Packed `.fwsc` updates are not yet
accepted.

`rust-probe` calls the embedded probe directly from the unchanged full
`build/fm1-diag.elf`. Its twelve results match the saved physical FM-1 capture.
The existing Python interpreter remains an independent reference. The Rust
probe takes 70 instructions; the Python test image includes five additional
startup/call instructions, for 75.

For a raw application, specify the probe entry from that build's symbol table:

```sh
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml --offline -- \
  probe build/fm1-diag.bin --entry 0x02002bc2
```

The address above belongs to the current FM-1_980 build, not arbitrary firmware.
Both commands accept `--limit COUNT` and `--trace PATH` (JSONL).

The separate foundation firmware boots from `_start`, copies `.data` and
`.ram_text`, clears `.bss`, runs the hardware-verified CPU probe, executes RAM
code, observes TIMER4 progressing, and services a TIMER5 interrupt through a
guest vector and handler. The handler runs on SSP, saves/restores registers,
acknowledges the timer, and returns with `rti` to the application stack.

```sh
mise run build-foundation
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml --offline -- \
  boot build/foundation/firmware.elf --until foundation_done \
  --inspect foundation_results:10
```

This ELF/raw application is for emulator tests. It has no updater or recovery
and must not be installed on the FM-1. The physical device retains the working
FM-1_980 diagnostic firmware.

Timers use a deterministic virtual clock of 24 oscillator ticks per guest
instruction (one microsecond), not measured CPU cycle timing. TIMER4 supports
OSC /1 and TIMER5 supports OSC /4; other clock modes fail explicitly. Interrupt
delivery currently covers non-nested TIMER5/IRQ63 with global and per-source
masking. SPL initial state and the interrupt stack handoff are functional
approximations that still need independent physical validation.

The full diagnostic image still stops at its first unsupported instruction,
currently `r0 = r0 & 0xF` in startup. There are no silent MMIO defaults,
instruction skips, or host substitutions for firmware functions. This is
application emulation after the SPL handoff, not ROM or SPL emulation.

Probe encodings come from the Python reference, vendor disassembly, and the
physical comparison. Startup stack arithmetic, immediate masks, and special
register mappings were checked against the Apache-2.0
[Quarkslab pi32v2 reference](https://github.com/quarkslab/ghidra-jieli/tree/e1bd0707874b77b759401555d24839ad43af1267/data/languages).
New CPU/peripheral behavior needs separate hardware validation.
