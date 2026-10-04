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

The first boot milestone executes the application's entry and startup until
the TIMER4 register at `0x10800`, where an explicit access fault reports the
missing device. There are no silent MMIO defaults, instruction skips, or host
substitutions for firmware functions. This is application emulation after the
SPL handoff, not ROM or SPL emulation. Initial CPU state is approximate.

Probe encodings come from the Python reference, vendor disassembly, and the
physical comparison. Startup stack arithmetic, immediate masks, and special
register mappings were checked against the Apache-2.0
[Quarkslab pi32v2 reference](https://github.com/quarkslab/ghidra-jieli/tree/e1bd0707874b77b759401555d24839ad43af1267/data/languages).
New CPU/peripheral behavior needs separate hardware validation.
