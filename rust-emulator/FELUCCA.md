# Felucca boot investigation (2026-10-04)

Full Felucca currently stops during user sample storage initialization, before
LCD or USB startup. The published firmware and a build from the local source
both fail reading `0x0209c000`, the XIP address for NOR flash offset `0xA0000`.
The emulator has no backing for this part of flash: it maps only the supplied
application bytes. An empty sample slot must read as erased flash, rather than
fault. This is an emulator limitation, not evidence of a Felucca bug.

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

## Observed failures

| Emulator state | Published application | Local ELF | Cause |
| --- | --- | --- | --- |
| Before this investigation | 155,138 instructions; PC `0x0200cf52`, opcode `0x25a0` | 155,138 instructions; PC `0x0200cd2a`, opcode `0x21a0` | Short stack load/store decoder omitted offset bit 7. The flash driver pointer spills to `[sp+148]` / `[sp+132]`. Fixed in `c5d768a`. |
| After stack fix | 266,973 instructions; PC `0x020049f0`, opcode `0xecdc` | 266,973 instructions; PC `0x020049fa`, opcode `0xecdc` | Missing `r1 = [++r6=r1]` word load. Fixed in `745b9ec`. |
| After both fixes | PC `0x020049f0`, reading `0x0209c000` | PC `0x020049fa`, `smp_user_scan+0x20`, reading `0x0209c000` | User sample flash lies outside the loaded application. Still blocked after 266,973 completed instructions. |

`firmware/src/main.c` calls `persist_boot()` before `lcd_init()`.
`firmware/src/project.c` scans the user sample slots during `persist_boot()`;
`firmware/src/eng_sample.c:79` reads the first slot header's magic;
`firmware/hal/fm1_xip.h` maps physical flash offset to the XIP window.
The local diagnostic run confirms **0 LCD pixels, 0 interrupt entries, 0 USB
setups/packets/CDC bytes**, and one watchdog feed. The window therefore has a
blank guest screen and a fault; no real UI or button serial output is reached.

## Reproduce from the emulator checkout

The existing launcher accepts the local build directly:

```sh
./emulator ../Felucca/build/felucca.elf
```

For a compact failure report with the last twelve completed instructions,
nearest ELF symbols, registers, LCD activity, USB activity, and watchdog feeds:

```sh
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml \
  --release --locked --offline --example diagnose -- \
  ../Felucca/build/felucca.elf 10000000
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

1. Back the complete NOR address space and connect plain XIP reads to the same
   flash storage as SPI. Begin with erased user regions, then support loading a
   flash snapshot. Preserve application XIP/decryption boundaries and reject
   truly invalid addresses.
2. Rerun the unchanged guest to discover the next actual failure. Add regression
   coverage for each newly encountered instruction or peripheral.
3. Later requirements visible in source, **not yet reached in this run**: ADC
   conversion for the master/battery controls, audio codec and ALNK DMA/interrupts,
   USB MIDI host transfers, and NOR program/erase/persistence. The synth DSP
   instruction coverage and timing cannot be assessed before startup reaches it.

## Validation

All 36 Rust tests passed with the GUI feature enabled, including the existing
hardware display boot and USB button output checks. Formatting and Clippy with
warnings denied passed. The new decoder tests cover stack offsets 0..252 at key
boundaries, all eight encoded registers, preincrement operand overlap, and
wrapping addition. Their instruction forms were checked against vendor
instructions; individual new forms have not been compared on the physical FM-1.
The diagnostic runner was also checked against the working display application:
it advances the real LCD/USB state and forwards the guest CDC banner to stdout.
