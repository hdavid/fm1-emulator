# FM-1 minimal diagnostic firmware and CPU emulator

Version 0.2. This is a separate, small firmware derived from Felucca. It contains
USB serial, the USB-MIDI update protocol, the update loader, timer/input drivers,
basic LCD status, watchdog and crash recovery, and a deterministic CPU probe.
It contains **no synth engines, effects, sequencer, sample data, editor, or preset
storage**. The compiled application is 14,804 bytes. Its experimental package
identity is `FM-1_980`; this is not an official M-VAVE or Felucca release.

**Initial physical FM-1 comparison passed on 2026-10-04.** The device was updated
from Felucca `FM-1_909` to diagnostic `FM-1_980`, restarted successfully, and all
twelve CPU-probe result words matched the emulator with the same probe hash.
The initial capture is preserved in `build/hardware-initial.txt`; the session record is
`build/hardware-verification.txt`. This validates the probe's tested instruction
forms and inputs; recovery paths and other hardware behavior remain unverified.
The emulator executes the shared CPU probe. It does not boot the complete diagnostic firmware
or emulate USB, LCD, flash hardware, interrupts, audio, or cycle timing.

## Setup with mise

Install [mise](https://mise.jdx.dev/installing-mise.html), unzip this project, and
run these commands from the `fm1-emulator` directory:

```sh
mise trust
mise install
mise run setup
mise run build
mise run test
```

`mise.toml` pins uv 0.8.24. uv manages Python 3.12.11, pinned in `.python-version`.
All Python tasks run through uv. `setup` installs Python and the packages from
`uv.lock`, downloads the checksum-pinned JieLi compiler, and fetches the
pinned AC79 SDK. The SDK revision and the three packaging blobs are checked.
Git and network access are required on first setup. Dependencies go in `.deps`
and `.venv`. They are not included in the archive.

The compiler runs natively on Linux x86-64. On macOS or Linux ARM, the build
uses Docker with `linux/amd64`; install and start Docker first and put this
project somewhere Docker can share. Docker is a system prerequisite, not installed
by mise. Native Windows builds are not supported by these build scripts; use
WSL for building. Serial capture accepts Windows COM names when run with Python.

The archive already includes built artifacts. `setup` and `build` let you reproduce
them or change the probe. The vendor archive is pinned by SHA256, not its mutable
"latest" download endpoint. Its directory/version is `20250324.1`.

## Flash the minimal firmware

The installable file is **`build/fm1-diag.fwsc`**. Do not flash `probe.bin` or the
raw `fm1-diag.bin` as an update package.

Connect the FM-1 with a USB data cable and close applications using its MIDI
port. This replaces the running synth with the diagnostic firmware; it will not
play notes. Keep a known working firmware package and your recovery method
available for this first hardware test. Use stable power during the update.

```sh
mise run flash
```

The included Felucca-derived updater identifies the device, asks before writing,
sends the package, and checks for `FM-1_980` after restart. If there are multiple
MIDI devices or auto-selection fails, specify the FM-1 MIDI port name:

```sh
mise run flash -- --port "FM-1"
```

This is a MIDI port name, not the serial device path used below. The firmware
currently on the device must support the FM-1 update protocol. This workflow is
not a universal ROM/UBOOT recovery flasher. The inherited updater and loader
have passed simulations; that does not prove this new firmware boots on hardware.

After boot, the screen shows:

- `D1A60001`: diagnostic firmware, protocol version 1.
- A hex counter beneath it: number of completed `probe` commands.
- Green top strip: expected flash JEDEC ID found. Yellow: unexpected ID;
  flash staging is disabled. The CPU probe can still run.
- Blue during an update; red on a fault, with vector, PC, and exception data.

Holding OCT− and OCT+ for five seconds requests UBOOT recovery. The serial
`uboot` command does the same. Two early failed boots also request UBOOT.
These paths are inherited from Felucca and still require hardware confirmation
in this minimal build. UBOOT mode needs a compatible recovery tool; the normal
MIDI update command cannot be assumed to work in that mode.

## Run and compare

Run the emulator, then find the diagnostic USB serial port:

```sh
mise run emulate
mise run ports
```

Capture from the port shown by the second command, for example:

```sh
mise run capture -- /dev/ttyACM0
mise run compare
```

On macOS, the path normally starts with `/dev/cu.usbmodem`. The capture tool
asserts DTR, sends `probe`, and saves `build/hardware.txt`. It performs no flash
writes. Incomplete output is also saved so that timeouts can be diagnosed.
A serial terminal at 115200 with DTR enabled can issue `info`, `probe`, or `uboot`.

The comparator checks the probe hash and all twelve result words. Exit codes:
0 = all match; 1 = values differ; 2 = invalid/incomplete capture or a build hash
mismatch. It does not authenticate the origin of a text file.

Send back `build/hardware.txt` and `build/emulator.json`. For a mismatch, also
include `build/trace.jsonl`. If USB never appears, report the screen contents and
the updater output. The original archive's `build/validation.json` describes
host validation before the physical test; see `build/hardware-verification.txt`
for the subsequent device comparison.

## What is compared

`firmware/probe.S` is assembled once for a tiny emulator test image and once into
the diagnostic firmware. The build checks that the function's 114 machine-code
bytes match exactly in both ELFs. Interrupts are masked only during this short
probe on hardware. The probe uses stack-local output there; the emulator uses a
fixed SRAM result buffer. The memory addresses need not match for these tests.

The twelve words cover a 32-bit constant, wrapping addition, subtraction, XOR,
AND, OR, NOT, logical left/right shifts, a load followed by addition, a loop sum
(55), and stack storage. Emulator tests also check register/stack preservation.
A successful capture validates these instruction forms for these inputs only.

The emulator maps XIP at `0x02000120` and 512 KiB of SRAM at `0x01C00000`, supports
the 16/32/48-bit instruction encodings needed by this probe, and rejects unknown
instructions. The 142-byte test image executes 75 instructions. The JSONL trace
records each instruction and register state.

To extend coverage, edit `firmware/probe.S`, implement new instruction forms in
`emu.py`, add independent expected-value tests, rebuild, flash, and capture again.
For more result words, also update `NAMES` and `expected()` in `emu.py`, the count
in `tools/build.py`, the result array and reporting loop in `firmware/src/diag.c`,
and the protocol parser/version. Never treat new instructions as hardware-verified
until the device capture agrees.

## Files and verification

| File | Purpose |
| --- | --- |
| `firmware/src/diag.c` | Minimal application and diagnostic console |
| `firmware/src/startup.c` | Felucca-derived startup and boot-loop guard |
| `firmware/probe.S` | Shared CPU test |
| `firmware/hal`, `firmware/loader` | Hardware access and update loader |
| `build/fm1-diag.fwsc` | Installable update package |
| `build/fm1-diag.elf`, `.dis`, `.symbols` | Hardware debugging artifacts |
| `build/fm1-diag.json` | Source, compiler, SDK, and binary hashes |
| `build/probe.bin`, `.elf`, `.json` | Emulator image and manifest |
| `build/validation.json` | Scope and results of host validation |

The 609,649-byte update package includes the fixed, padded app slot and the
loader; its size does not indicate bundled synthesis engines.

`mise run test` runs eleven tests covering the CPU, parser, package CRCs, and the
decrypted packaged app. Arithmetic and logic tests include 1,056 input cases.
The build also checks XIP placement, RAM use, and that flash-driver RAM code
contains no calls back into flash.

With a host C compiler (`cc`, or set `CC`), run the optional updater simulations:

```sh
mise run test-update
```

They test staging, timeout cleanup, simulated NOR installation, boot-area
preservation, corrupt-header rejection, and host reconnect/failure handling.
They do not emulate the CPU or prove physical flash behavior. The temporary
baseline used in these simulations is never installed on a device.

## Sources and licence

GPL-3.0-only; see `LICENSE`. Borrowed files retain their upstream copyright
notices. This project adapts Felucca startup, HAL, USB/CDC, OTA, loader, packaging,
installer, and updater tests. The synth sources and assets are omitted. Local
changes are the diagnostic app and console, probe, emulator, build/setup tasks,
USB product string, timer-only ISR wrapper, and associated tests.

- [Felucca pinned source](https://github.com/hugelton/Felucca/tree/1e838e17e170b20ff09b9660c9a7171aadfc5dca)
- [JieLi AC79 SDK](https://gitee.com/Jieli-Tech/fw-AC79_AIoT_SDK/tree/AC79NN_SDK_V1.2.1_2023-12-13), commit `d179b4484759423312073f5fbb232501aa491047`
- [Quarkslab pi32v2 reference](https://github.com/quarkslab/ghidra-jieli/tree/e1bd0707874b77b759401555d24839ad43af1267)
- [AL-255 FM-1 research](https://github.com/AL-255/FM-1-RE)

The package incorporates the SDK's `uboot.boot`, `cfg_tool.bin`, and
`cfg/eq_cfg_hw.bin`; its Apache-2.0 licence is included under `LICENSES`.
The compiler is downloaded from JieLi separately. No vendor toolchain binaries
or Felucca sound/font assets are redistributed in this source archive.
