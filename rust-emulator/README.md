# Rust FM-1 emulator

Rust 1.91.1 is managed by the root `mise.toml`. The interpreter executes vendor-built pi32v2 machine code without external
Rust dependencies. The optional `gui` feature uses eframe/egui 0.31.1 for a
native OpenGL window; the version and transitive dependencies are locked.

On a fresh checkout, fetch the locked dependency metadata before offline tests:

```sh
mise exec -- cargo fetch --manifest-path rust-emulator/Cargo.toml --locked
mise run rust-test
mise run rust-probe
mise run build-foundation
mise run rust-foundation
```

## Native device window

From the project root:

```sh
./emulator build/display/firmware.elf
mise run build-display       # optional: rebuild guest with the vendor compiler
mise run rust-gui-test       # core and GUI input integration tests
```

The launcher compiles a release `fm1-ui` executable using mise. On macOS it
creates a local application bundle under `target/release`; on Linux it runs the
binary directly (the host needs the normal eframe OpenGL/windowing development
libraries). macOS is visually verified; Linux is not tested here. The existing
`fm1-emu probe|boot` CLI is unchanged and does not require a display server.

The panel is an original vector illustration drawn in Rust, using the device's
[front-panel photograph](https://m.media-amazon.com/images/I/71fgwUHLJKL._AC_SL1500_.jpg)
as a layout reference. No vendor product photo is bundled. Button identities
follow the [pinned Felucca panel defaults](https://github.com/hugelton/Felucca/blob/1e838e17e170b20ff09b9660c9a7171aadfc5dca/firmware/src/panel.c)
and the local `fm1_input.h` wiring. All fourteen buttons and twenty-seven note
keys feed matrix contacts; rotary controls are currently decorative. Short
clicks/keystrokes are held for at least 100 ms so a guest scan can observe them.
Losing window focus releases contacts. Pause stops guest execution; Restart
reloads the selected image and resets CPU, RAM, peripherals, and input state.

The UI reads only the panel's 240×240 framebuffer, gated by display enable,
sleep, and active-low PA2 backlight. There are no symbol-specific drawing hooks
or substituted application functions. SPI1 commands and SRAM DMA implement
software reset, sleep/display enable, RGB565 format, unrotated RGB/BGR, column/
row windows, and pixel writes. Unsupported commands, rotations, and invalid DMA
addresses fault visibly. Completion is synchronous, not cycle-accurate; INVON
is treated as the FM-1 panel's normal electrical drive mode, not an RGB invert.
The CPU runs bounded slices on the UI thread, not at a calibrated real-time rate.

`firmware/display.S` is an opt-in extension of the foundation guest. After the
same five foundation checks, it configures SPI/LCD, draws its own labels and
buffered hexadecimal TIMER4 readout, rescans the matrix, and sends key tiles.
The first frame takes 84,921 instructions and services one guest IRQ. Firmware
is 2,700 bytes. This exercises LCD as a sixth foundation, with no claim of full
panel compatibility or physical validation of this new path.

The full FM-1_980 diagnostic still stops on opcode `0xE160` at `0x02001DB6` before
initializing its LCD. That produces a stopped indicator and an error below a
black device screen. It does not display a simulated diagnostic menu. Full
Felucca, USB, audio, and flash emulation remain incomplete.

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

The separate 592-byte foundation firmware boots from `_start`, copies `.data` and
`.ram_text`, clears `.bss`, runs the hardware-verified CPU probe, executes RAM
code, observes TIMER4 progressing, and services a TIMER5 interrupt through a
guest vector and handler. The handler runs on SSP, saves/restores registers,
acknowledges the timer, and returns with `rti` to the application stack. Firmware
also clocks the two 74HC595 registers on PA4/PA3, latches on PA1, and scans all
eleven active-low matrix columns. GPIO direction, digital enable, pull-ups,
outputs, and the physical PA/PB row wiring determine the observed key states.

```sh
mise run build-foundation
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml --offline -- \
  boot build/foundation/firmware.elf --until foundation_done \
  --inspect foundation_results:10 --press 0:4
```

`--press COLUMN:ROW` injects a physical matrix closure; repeat it for multiple
keys. Columns are 0..10 and packed rows 0..5. `0:4` is OCT-minus; `3:4` is the F3
note key. Debouncing and encoder decoding belong to firmware, not the GPIO
model. Scheduled input events and the full Felucca input routine are future work.

`--until` accepts a breakpoint symbol or numeric address, and `--inspect` accepts
`SYMBOL_OR_ADDRESS:WORDS`. Raw `.bin` boot works with numeric addresses. Successful
boot output reports the reached PC, executed instructions, IRQ entries, and
inspected guest RAM. `--trace PATH` saves executed instructions as JSONL.

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

## Agreed foundation checklist

The completion criterion is five of these ten foundations exercised by booted
guest firmware. Each foundation has equal weight for this milestone. This is
not a percentage of complete instruction-set or musical-feature coverage.

| Foundation | Milestone evidence | Remaining scope |
| --- | --- | --- |
| CPU | Guest executes real vendor machine code; twelve probe words match hardware | Further ISA forms, flags, parallel instructions |
| Memory/startup | ELF equals raw flash image; guest copies data and RAM code, clears dirty BSS, executes RAM code | ROM/SPL, reset retention, boot parameters |
| Timers | Guest sees TIMER4 progress; TIMER5 produces a periodic event | Other sources/dividers and measured cycle timing |
| Interrupts | IRQ63 vector, masking, SSP handler frame, acknowledgment, `rti` | Nested priorities, other IRQs, physical entry-state validation |
| Controls | Guest scans eleven columns; released/pressed and multiple-key cases agree | Scheduled events and full firmware debounce/encoder routines |
| Flash | Not implemented | NOR, SPI, erase/program, XIP busy behavior |
| LCD | Display guest initializes SPI/DMA, draws RGB565 pixels and live timer/key data | Other controller modes, SPI timing, hardware comparison |
| USB serial | Not implemented | Controller, endpoints, enumeration, CDC |
| USB MIDI | Not implemented | USB transport and MIDI packet handling |
| Audio/DMA | Not implemented | Audio clocks, DMA, buffers and sample output |

Original foundation evidence: seventeen Rust integration tests passed; a native boot executes
2,371 instructions, services one guest interrupt, and reaches `foundation_done`.
The guest's last result is `0x0050F00D`. See `build/foundation/verification.txt`.

Current suite: 24 core integration tests and 3 GUI integration tests pass.
See `build/display/verification.txt` for the display milestone and limitations.
