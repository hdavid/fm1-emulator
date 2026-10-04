![FM-1 emulator running the Felucca firmware](docs/felucca.jpg)

# FM-1 emulator

A Rust emulator for the M-VAVE FM-1. Load firmware and interact with its own
screen and buttons. USB serial output from the firmware appears in your terminal.

## Download and run

**[Download a prebuilt emulator from Releases](../../releases).** Extract the
archive for your platform; Rust, mise and Docker are not needed to run it.
Firmware is supplied separately. [Felucca 0.9-beta](https://github.com/hugelton/Felucca/releases/tag/v0.9-beta)
boots with partial functionality.

| Platform | Archive |
| --- | --- |
| Linux AMD64 | `emulator-linux-amd64.tar.gz` |
| Linux ARM64 | `emulator-linux-arm64.tar.gz` |
| macOS ARM64 (Apple Silicon) | `emulator-macos-arm64.tar.gz` |
| Windows AMD64 | `emulator-windows-amd64.zip` |

From the extracted directory, run:

```sh
# Linux / macOS
./emulator /path/to/felucca-0.9-beta.fwsc
```

```powershell
# Windows
.\emulator.exe C:\path\to\felucca-0.9-beta.fwsc
```

The loader accepts `.fwsc` packages, application `.elf` files and raw `.bin`
images. Click and hold the panel buttons, use the arrow keys for octave changes,
and `A W S E D R F G T H Y J K` for notes. Pause and Restart control execution.
Linux downloads target Ubuntu 24.04 or newer and need a working OpenGL display.
The macOS application is ad-hoc signed, without Apple notarization.

## Build from source

Install [mise](https://mise.jdx.dev/installing-mise.html) and a native C/C++
toolchain: Xcode Command Line Tools on macOS, Visual Studio Build Tools with
**Desktop development with C++** on Windows, or the following on Ubuntu:

```sh
sudo apt-get install build-essential pkg-config libx11-dev libxi-dev libgl1-mesa-dev libxkbcommon-dev libwayland-dev
```

From this checkout:

```sh
mise trust
mise install rust
mise exec -- cargo build --manifest-path rust-emulator/Cargo.toml --locked --release --features gui --bin fm1-ui
mise exec -- cargo test --manifest-path rust-emulator/Cargo.toml --locked --release --features gui
```

On Linux/macOS, run `./emulator /path/to/firmware.fwsc`; the launcher builds and
opens the emulator. On Windows, run
`.\rust-emulator\target\release\fm1-ui.exe C:\path\to\firmware.fwsc`.
Building the emulator does not require the vendor firmware compiler or Docker.

[GitHub Actions](.github/workflows/emulator.yml) tests and builds all four
platforms, including boot, note-rendering and FX checks using the checksum-pinned
Felucca release. Successful push builds publish a release tagged with the commit
SHA, with archives and SHA256 checksums. Pull requests run the same checks.

## Firmware compatibility

Updated on 2026-10-05. **Partial** means boot and some controls work, but
other firmware paths can stop emulation.

| Firmware | Status | Verified behavior / blocker |
| --- | --- | --- |
| Felucca 0.9-beta (`FM-1_909`, `.fwsc`) | Partial | LCD, USB console, watchdog, note audio/DMA and FX pass; full UI coverage remains incomplete |
| Felucca source build (`1e838e1`, `.elf`) | Partial | Boot, note press/release, FX, HOME and ENV pass; presets can still stop emulation |
| Official `FM-1_015` (`FM-1.fwsc`) | Fails | Missing CPU instruction during startup; no screen yet |
| Baud Girl `FM-1_093` (`FM-1_093.fwsc`) | Fails | Same startup blocker; no screen yet |

FX now opens and renders in both Felucca builds. A separate unsupported
instruction remains on the local build's presets path; see the
[Felucca investigation](rust-emulator/FELUCCA.md).

## Still to implement

- Remaining CPU instructions and peripherals, including other Felucca paths and official/Baud Girl startup.
- Host audio playback, rotary controls, USB MIDI and serial input.
- Flash erase/program and persistence, plus fuller encryption, interrupt and timing behavior.

GPL-3.0-only; see [LICENSE](LICENSE). Based on research and components from
[Felucca](https://github.com/hugelton/Felucca), the
[JieLi AC79 SDK](https://gitee.com/Jieli-Tech/fw-AC79_AIoT_SDK), and
[Quarkslab's pi32v2 reference](https://github.com/quarkslab/ghidra-jieli).
