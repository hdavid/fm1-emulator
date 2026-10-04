# Felucca boot investigation (2026-10-04)

Full, unchanged Felucca now boots into its main screen. The published 0.9-beta
application and the local source build both run for 100 million instructions
without a guest fault, send the console banner through USB CDC, update the LCD,
render audio DMA halves and service the watchdog. An additional local ELF test
presses/releases a note and selects ENV: after 112 million instructions it has
126 UI frames, 589 guest-rendered audio halves and 127 watchdog feeds. The note
produces nonzero stereo DMA samples. Host speaker playback is not implemented.

This is bounded boot/input validation, not full synth compatibility. Other
engines and UI paths may encounter more instruction gaps. Flash programming,
flash persistence, rotary input, USB MIDI host input and CDC host input remain.
No Felucca application bytes, source, feature flags or watchdog were patched.

## Inputs and provenance

- Published [Felucca 0.9-beta](https://github.com/hugelton/Felucca/releases/tag/v0.9-beta),
  package identity `FM-1_909`, tag commit
  `e5a908d0383848cd85149de6dc35150c792231fc`.
- Package: 609,649 bytes; SHA256
  `320ef650a5c123becb46e749514f2b71c9ffa821a3fff8a515673cd9d21194a5`.
- Extracted application slot: 581,564 bytes, including package padding; SHA256
  `5a00708f4988b6ecf31e1218f8e68235640c9f8a1cb8efb6d966a275d99843d2`.
  The existing package decoder validated the identity, outer header, file table,
  file payload CRCs, and decrypted `app.bin` entry/payload CRCs. No guest bytes
  were patched. Download, extraction script, and baseline traces are cached in
  the ignored `.deps/felucca-trial/` directory.
- Local source: `~/src/Felucca`, clean commit
  `1e838e17e170b20ff09b9660c9a7171aadfc5dca`. This differs from the release tag.
  Built with `--release 0.9-beta`, default feature flags, and `-g` added to the
  compiler flags. Optimization remains `-Os`; watchdog, storage, audio, CDC, and
  update support remain compiled in. No Felucca source edits were necessary.
- Local application: 417,668 bytes; SHA256
  `12a4b4ea47248467f566ec3b6984b08f2f89d6ef5a8e9e494cab5184e8fadb36`.
  The vendor build's RAM, RAM-code, and register-access checks passed. The ELF,
  disassembly, application, and update package are in `~/src/Felucca/build/`.

## Initial failures (resolved)

| Emulator state | Published application | Local ELF | Cause |
| --- | --- | --- | --- |
| Before this investigation | 155,138 instructions; PC `0x0200cf52`, opcode `0x25a0` | 155,138 instructions; PC `0x0200cd2a`, opcode `0x21a0` | Short stack load/store decoder omitted offset bit 7. The flash driver pointer spills to `[sp+148]` / `[sp+132]`. Fixed in `c5d768a`. |
| After stack fix | 266,973 instructions; PC `0x020049f0`, opcode `0xecdc` | 266,973 instructions; PC `0x020049fa`, opcode `0xecdc` | Missing `r1 = [++r6=r1]` word load. Fixed in `745b9ec`. |
| After both fixes | PC `0x020049f0`, reading `0x0209c000` | PC `0x020049fa`, `smp_user_scan+0x20`, reading `0x0209c000` | User sample flash lies outside the loaded application. Fixed by plain XIP reads sharing the NOR storage (`5fd57ae`). |

`firmware/src/main.c` calls `persist_boot()` before `lcd_init()`.
`firmware/src/project.c` scans the user sample slots during `persist_boot()`;
`firmware/src/eng_sample.c:79` reads the first slot header's magic;
`firmware/hal/fm1_xip.h` maps physical flash offset to the XIP window.
The original diagnostic run had zero LCD/USB activity. Subsequent fixes added
compiler load/store forms, register-pair operations, signed arithmetic and
conditional blocks. They also corrected simultaneous register reads in parallel
bundles and load sign bits that had been mistaken for address bits. Missing CPU
forms continue to fault rather than silently inventing results.

## Hardware models reached by the full firmware

- User flash: plain XIP reads and SPI reads share the erased 1 MiB NOR storage.
  SFC enable, plaintext window and physical bounds are enforced. The supplied
  application is already decrypted; encrypted reads outside it are unsupported.
- Audio: ALNK0 reads actual guest stereo SRAM, advances at 44.1 kHz, alternates
  DMA halves and delivers/acknowledges IRQ 11. Interrupt selection respects the
  configured ALNK/Timer5 priorities. A bounded sample queue retains DMA output.
  Codec analog behavior, interrupt nesting and cycle-accurate timing are absent.
- ADC: channels 3/4 sample battery/master inputs, complete after a functional
  delay, expose the completion bit and cancel when disabled. Default simulated
  inputs are battery 800 and master 512 (10-bit); other channels fault.
- LCD and controls: the ordinary SPI/DMA and GPIO matrix models run Felucca's
  own drawing, scan, debounce, note and page-selection code. Short UI clicks
  remain held for at least 100 ms of guest time as well as host time.
- USB: the host enumerates the guest's configuration and asserts CDC DTR. The
  console banner on stdout comes from a real guest CDC endpoint packet.
  Felucca does not normally log every panel button; there are no synthetic logs.

At 100 million instructions the local ELF writes 1,370,362 LCD pixels, enters
8,162 interrupts, completes five USB setup requests and sends one 39-byte CDC
packet, with 100 watchdog feeds. The published image writes 1,370,362 pixels,
enters 8,381 interrupts and sends the same banner. These runs end deliberately
at the instruction budget, not a crash. The macOS window was visually checked
for the actual guest screen and correctly formatted positive parameter values.

## Reproduce from the emulator checkout

The existing launcher accepts the local build directly:

```sh
./emulator ../Felucca/build/felucca.elf
```

For a compact failure report with the last twelve completed instructions,
nearest ELF symbols, registers, LCD/USB/audio/ADC activity, and watchdog feeds:

```sh
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml \
  --release --locked --offline --example diagnose -- \
  ../Felucca/build/felucca.elf 100000000
```

The diagnostic runner uses the same CPU and bus as the UI. Reports go to stderr;
stdout contains only guest bytes received through emulated USB CDC. It returns
failure on either a guest fault or the instruction limit. ELF names are nearest
symbols, not a reconstructed C call stack or source-line debugger. The full
instruction trace is still available through the existing `boot --trace PATH`.

To repeat the local build with the emulator's mise/uv and vendor dependencies:

```sh
mise exec -- uv run --no-project --python .venv/bin/python \
  --with pillow==11.3.0 python - <<'PY'
from pathlib import Path
import sys
sys.path.insert(0, str(Path('../Felucca/tools').resolve()))
import build
build.CFLAGS.append('-g')
sys.argv = ['build.py', '--release', '0.9-beta']
build.main()
PY
```

This uses the existing Docker build path on macOS and writes Felucca's ignored
build outputs. Firmware diagnostics remain compatible with the ordinary FM-1
application build; no emulator-only guest replacement was introduced.

## Next work

1. Play the guest DMA samples through the host audio device and wire the panel
   encoders/master control to the existing hardware inputs.
2. Add CDC OUT and USB MIDI host transfers, then exercise console commands and
   other synth engines without guest modifications.
3. Support NOR write enable, program, erase and persistence so project/settings
   saves work. Load real flash snapshots when comparing user sample playback.
4. Extend CPU and peripheral coverage only where unchanged guest execution or
   independent hardware comparisons demonstrate the required semantics.

## Validation

The ordinary Rust suite includes the existing hardware display boot and USB
button output checks, plus independent regressions for the new decoding and
peripheral behavior. Individual new instruction forms have been checked against
vendor disassembly but have not been compared on the physical FM-1. Timing is
functional, using one oscillator tick per completed instruction bundle.

The external full-firmware test is deliberately opt-in: no Felucca application
or assets are redistributed. It checks guest breadcrumbs, debounced note state,
nonzero stereo output, ENV page selection, LCD/ADC/USB activity and watchdog
service. Run it after building the local source:

```sh
FELUCCA_ELF="$HOME/src/Felucca/build/felucca.elf" mise exec -- cargo test \
  --manifest-path rust-emulator/Cargo.toml --release --locked --offline \
  --test felucca -- --ignored --nocapture
```

On macOS, launching from a restricted automation sandbox can abort in
`_RegisterApplication` before the emulator loads any guest code. The desktop
launch was verified outside that sandbox. A WindowServer watchdog report alone
does not identify which application caused the display service to hang.
