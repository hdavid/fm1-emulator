# Rust FM-1 emulator

Rust 1.91.1 is managed by the root `mise.toml`. The interpreter executes vendor-built pi32v2 machine code without external
Rust dependencies. The optional `gui` feature uses eframe/egui 0.31.1 for a
native OpenGL window; the version and transitive dependencies are locked.

The [full Felucca boot investigation](FELUCCA.md) records the verified firmware
inputs, resolved startup failures, decoder fixes, full boot and note checks. It also
documents the bounded `diagnose` runner for symbol and peripheral reports.

Timing: `examples/latency.rs` measures, in exact guest time, the input-to-audio
latency of a Felucca-family package (a key, a USB-MIDI or TRS note-on to the
first DMA frame out of silence) and, for SLOOP builds with MIDI clock, the
phase of the step hits against an injected 24 PPQN clock (USB or TRS, jitter,
tempo ramps) and of the clock the firmware sends. The TRS MIDI IN line (UART1
RX DMA at 31250 baud: `Bus::uart_midi_send`), an onset probe on the audio DMA
(`audio.probe`) and send times of USB-MIDI IN packets
(`usb.midi_received_ticks`) support it. Interrupts do not nest in the model: a
timer interrupt waits for the audio interrupt to return.

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
./emulator "$HOME/Downloads/FM-1.fwsc"
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
clicks/keystrokes are held for at least 100 ms of both host and guest time so a
slow guest scan can observe and debounce them.
Losing window focus releases contacts. Pause stops guest execution; Restart
reloads the selected image and resets CPU, RAM, peripherals, and input state.

### Web editor (USB-MIDI over a local WebSocket)

When the firmware has a web editor, `fm1-ui` serves it on
`http://127.0.0.1:8765/` (loopback only) and connects it to the emulated
device: `FIRMWARE-ui.zip` next to `FIRMWARE.fwsc`, or `--ui DIR` (a folder with
`index.html` or `editor.html`, e.g. a Felucca-family `web/`). "Open editor"
opens the default browser; the status line shows the URL, connected pages, MIDI
message counts and the port name. Without an editor the button is disabled.

```sh
fm1-ui ~/GitHub/fm1-firmware/jangada-0.1-alpha.fwsc --cpu-mhz=48
fm1-ui firmware.fwsc --ui ~/GitHub/jangada/web
scripts/make-ui-sidecar.sh ~/GitHub/sloop v2.2 ~/GitHub/fm1-firmware/sloop-2.2.fwsc
```

- The USB host model then also acts as a USB-MIDI host (`usb.enable_midi_host()`):
  it reads the product string and moves class-compliant 4-byte event packets on
  the MIDI streaming endpoints (`usb_midi.rs`: SysEx split / reassembly).
- Every served HTML page gets a Web MIDI shim (`web/shim.js`):
  `navigator.requestMIDIAccess` returns one input and one output named
  `"<USB product> (FM-1 Emulator)"` ("Felucca (FM-1 Emulator)" for Felucca,
  Jangada and SLOOP, whose editors look for /felucca/i), so editors run unchanged
  in any browser.
- `/midi` is a WebSocket of binary frames of raw MIDI bytes: from the browser any
  split (running status allowed), to the browser one complete message per frame.
- Requests with a Host other than 127.0.0.1 / localhost / [::1] on that port,
  and WebSocket Origins other than those, are refused (DNS rebinding).

### USB audio host (UAC1) and interrupt nesting

For firmware with USB audio (SLOOP `feat/usb-audio`, `FELUCCA_USB_AUDIO=1`; Melodee's layout),
`usb.enable_audio_host(alt)` (alt 1 = 16 bit, 2 = 24 bit) makes the host model a USB audio host as
well (and a MIDI host): it reads the whole configuration, checks it as a UAC1 class driver would
(`usb_audio.rs`: structure, IADs, AC headers and terminals, type I formats, isochronous endpoints,
explicit feedback), selects the alternate on every streaming interface, sets and reads the sampling
rate (SET_CUR with its OUT data stage, GET_CUR), then every 1 ms frame (`usb_audio_host.rs`) takes
the capture IN packet and the 10.14 feedback the device armed (an isochronous IN waits for its
frame; nothing armed counts as a missed frame), and sends a playback OUT packet sized by the
feedback from `audio_mut().play_queue` (silence when empty; a packet into a buffer the device still
owns is lost and counted). Captured samples are in `audio().capture`; `usb_audio::wav_bytes` writes
WAVs. Without the audio host nothing changes (the iso deferral is off), and a configuration without
a CDC interface no longer stops the plain host.

These builds let TIMER5 (USB service) preempt the audio render: `cpu.nested_irqs = true`
(`FM1_NESTED_IRQ=1` for `diagnose`, `play_check`, `fm1-ui`) lets an interrupt of a higher priority
than the running handler's enter once that handler re-enabled interrupts (`sti`); the preempted
handler's source, priority level and block state are kept, the stack stays the system stack. Off by
default; the Felucca / Jangada / SLOOP 2.2 baselines are the same with it on (they never re-enable
interrupts inside a handler). Tests: `tests/usb_audio.rs`, `tests/usb_audio_host.rs` (a scripted
device), `tests/timers.rs` (nesting), and with a build's ELF `tests/usb_audio_firmware.rs`
(`USB_AUDIO_ELF=.../felucca.elf`, `--ignored`): stems equal the firmware's mix taps, DAC = mix +
playback, no ring under/overruns, USB-MIDI SysEx alongside.

The UI reads only the panel's 240×240 framebuffer, gated by display enable,
sleep, and active-low PA2 backlight. There are no symbol-specific drawing hooks
or substituted application functions. SPI1 commands and SRAM DMA implement
software reset, sleep/display enable, RGB565 format, unrotated RGB/BGR, column/
row windows, and pixel writes. Unsupported commands, rotations, and invalid DMA
addresses fault visibly. Completion is synchronous, not cycle-accurate; INVON
is treated as the FM-1 panel's normal electrical drive mode, not an RGB invert.
The CPU runs bounded slices on the UI thread, not at a calibrated real-time rate.

`build/display/firmware.elf` is the 17,056-byte FM-1_981 hardware application.
It uses Felucca-derived startup, watchdog, recovery, input scanning, USB CDC and
MIDI updater code. `firmware/display.S` draws labels, a buffered TIMER4 readout
and key tiles, called by the hardware application's main loop. There is no
emulator-specific application path in this image. Package integrity tests
compare its bytes against the decrypted `build/display/firmware.fwsc` payload.
The same package was flashed successfully to the FM-1; its USB serial status
reports advancing display frames, and all twelve CPU probe words match.

Button transitions pass through guest GPIO scanning and debounce, the CDC
ring, USB endpoint DMA and the emulator host before stdout receives
`KEY <id> down` or `KEY <id> up`. The host performs GET_DESCRIPTOR,
SET_ADDRESS, SET_CONFIGURATION and CDC SET_CONTROL_LINE_STATE requests.
It discovers the CDC interface and IN endpoint from the configuration descriptor.
Host-to-device console input and USB MIDI host transport are not implemented.
The hardware firmware retains both its serial console and MIDI updater.

P33 accesses model watchdog arming/feeding and stop with an expiry fault if
feeding ceases. NOR supports JEDEC/status/read transactions used at startup;
erase/program and persistent flash images are not implemented. CPU write guards
reject protected RAM writes; full guard exception dispatch and stack/PC limit
hardware remain incomplete. Reset requests stop rather than emulate ROM boot.
Full Felucca boots into its UI and renders note samples through ALNK DMA.
FX rendering and continued execution now pass with both the published package
and local source ELF. Support remains partial: the local presets path can stop
on an unsupported instruction. See the [Felucca investigation](FELUCCA.md) for
the verified scope and remaining failure.
Additional engines, CPU forms and peripheral behavior remain incomplete;
host audio playback is absent. See [the measured full-firmware checks](FELUCCA.md).

The loader accepts an FM-1 `.fwsc` package, an application `.bin` mapped at
`0x02000120`, or an executable ELF32-pi32v2. Package loading checks the outer
header/table, complete `flash.bin`, its flash directory, chip key and decrypted
application CRCs. It retains the original flash bytes and supplies the SDK's
SPL boot-device parameter block; encrypted SFC reads include the package's
directory and embedded configuration. Separate outer auxiliary payloads and
the update process are not emulated. ELF flash load addresses reconstruct the exact application `.bin`,
including the initializers for RAM code and data. Startup performs the RAM
copies; the loader does not move them early. Package support does not imply
that every stock CPU or peripheral path is implemented; see the
[stock firmware boot trials](STOCK-FIRMWARE.md).

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
model. The hardware display application runs the inherited full input routine.
Scheduled input events and UI encoder contacts remain future work.

`--until` accepts a breakpoint symbol or numeric address, and `--inspect` accepts
`SYMBOL_OR_ADDRESS:WORDS`. Raw `.bin` boot works with numeric addresses. Successful
boot output reports the reached PC, executed instructions, IRQ entries, and
inspected guest RAM. `--trace PATH` saves executed instructions as JSONL.

This ELF/raw application is for emulator tests. It has no updater or recovery
and must not be installed on the FM-1. It is a historical unit-test fixture;
use the shared hardware display build for device comparisons.

Timers use a deterministic virtual clock of one 24 MHz oscillator tick per guest
instruction bundle, not measured CPU cycle timing. TIMER4 supports
OSC /1 and TIMER5 supports OSC /4; other clock modes fail explicitly. Interrupt
delivery currently covers non-nested TIMER5/IRQ63 with global and per-source
masking. SPL initial state and the interrupt stack handoff are functional
approximations that still need independent physical validation.

The hardware display image runs from its application entry without host
substitutions for firmware functions. Unknown MMIO and instructions still
fault. This models the application after the SPL handoff, not ROM or SPL.
Parallel instruction pairs execute the following slot first. Conditional-block
state is preserved across IRQ63 entry/return. Compiler-derived encodings have
regression tests and boot coverage; only the probe's forms have individual
physical comparison evidence.

Probe encodings come from the Python reference, vendor disassembly, and the
physical comparison. Startup stack arithmetic, immediate masks, and special
register mappings were checked against the Apache-2.0
[Quarkslab pi32v2 reference](https://github.com/quarkslab/ghidra-jieli/tree/e1bd0707874b77b759401555d24839ad43af1267/data/languages).
New CPU/peripheral behavior needs separate hardware validation.

### Instruction coverage scan

`scripts/op-scan.sh [--out DIR] FIRMWARE...` (Docker and the JieLi toolchain,
as `scripts/pi32-objdump.sh`) disassembles each firmware with the vendor objdump
and runs `examples/op_scan` on the listing: every instruction objdump decodes is
classified by the interpreter's decoder (`fm1_emu::describe`) and executed once
on a scratch CPU. The report groups, by encoding, the instructions the
interpreter decodes as unsupported, rejects when executing, advances over with
another length than objdump, or would skip in a conditional block with another
length. An ELF is split into code by its STT_FUNC symbols; a `.fwsc` or raw
image by recursive descent from the entry (branch and call targets, tbb/tbh
tables, pointers to function prologues), with unreached clean runs and data
reported separately. `--check-against ELF` (op_scan) measures that heuristic
against the ELF the image was built from, and `--classes FILE` writes every
instruction's class.

objdump runs with `-mattr=+fprev1` (`PI32_OBJDUMP_FLAGS`): without it the FPU
instructions (`e53f`, the `iff` compares) are `<unknown>`. For an image, the
scan boots it in the emulator (`--boot-steps`, default 5M; 0 disables) and
matches RAM against the image to find the code startup copies to RAM, so
branches into RAM code and code pointers to it are followed. The report also
flags, inside code, halfwords objdump cannot decode (`vendor-unknown`) and
`??` predicates (`vendor-ambiguous`), with what the interpreter does there.

Reliability, measured against the ELFs of four SLOOP builds (sloop,
sloop-merged, sloop-dx7, sloop-ui) and their `.fwsc`: every reachable
instruction is an instruction of the ELF (100%), and reachable covers
99.5% of the ELF's instructions (flash and RAM code; the rest is a few
functions called only through computed pointers, such as `srec_finish`).
The unreached clean runs are 0.7% code and the rest is data (none of their
findings fall on an ELF instruction); likely data is 0% code. Stock
firmware has no ELF to check against; there, only the reachable class
should be read as code.

## Agreed foundation checklist

The completion criterion is five of these ten foundations exercised by booted
guest firmware. Each foundation has equal weight for this milestone. This is
not a percentage of complete instruction-set or musical-feature coverage.

| Foundation | Milestone evidence | Remaining scope |
| --- | --- | --- |
| CPU | Guest executes real vendor machine code; twelve probe words match hardware | Further ISA forms, flags and independent instruction probes |
| Memory/startup | ELF equals raw flash image; guest copies data and RAM code, clears dirty BSS, executes RAM code | ROM/SPL, reset retention, boot parameters |
| Timers | Guest sees TIMER4 progress; TIMER5 produces a periodic event | Other sources/dividers and measured cycle timing |
| Interrupts | IRQ63/ALNK11 vectors, masking, SSP handler frame, acknowledgment, `rti`, priority selection; optional nesting by priority after `sti` (`nested_irqs`) | Physical validation of nesting and of the entry state (is IE cleared on entry?), other IRQs |
| Controls | Guest scans eleven columns; released/pressed and multiple-key cases agree | Scheduled events and UI encoder input |
| Flash | Startup JEDEC/status/NOR reads; plain XIP shares physical NOR storage | Erase/program, persistence, XIP busy behavior |
| LCD | Display guest initializes SPI/DMA, draws RGB565 pixels and live timer/key data | Other controller modes, SPI timing, pixel-exact physical comparison |
| USB serial | Hardware guest enumerates and sends CDC debug bytes through DMA | Host OUT packets, broader controller/USB behavior |
| USB MIDI | Not implemented | USB transport and MIDI packet handling |
| Audio/DMA | Unchanged Felucca renders stereo SRAM, alternates ALNK halves, services audio IRQs; note samples are nonzero | Host playback, other clocks/formats, codec analog behavior, cycle timing |

Original foundation evidence: seventeen Rust integration tests passed; a native boot executes
2,371 instructions, services one guest interrupt, and reaches `foundation_done`.
The guest's last result is `0x0050F00D`. See `build/foundation/verification.txt`.

Current suite: 65 ordinary Rust tests and an opt-in full Felucca test pass.
See `build/display/verification.txt` for the display milestone and limitations.
