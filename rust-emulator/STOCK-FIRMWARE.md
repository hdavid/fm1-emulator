# Stock and Baud Girl firmware trials (2026-10-05)

The unchanged official `FM-1_015` package boots in the emulator, loads its
factory **PIANO 1** preset and responds to FX, HOME and note presses. A note
produces nonzero stereo samples through the firmware's own audio DMA path.
Compatibility remains **Partial**: this verifies basic operation, not every
preset, menu, effect or update path. Host audio playback is not implemented.

Baud Girl `FM-1_093` also reaches its LCD interface and responds to FX/HOME.
Its separate factory preset payload fails plaintext integrity validation and is
left unloaded. Note presses produced silent DMA samples in this trial.

## Inputs

These are the two packages supplied by the user, read without modifying their
contents. Firmware binaries are not committed.

| File | Package identity | Package bytes | Application bytes |
| --- | --- | ---: | ---: |
| `FM-1.fwsc` | `FM-1_015` | 699,956 | 581,564 |
| `FM-1_093.fwsc` | `FM-1_093` | 810,548 | 692,480 |

SHA256 identities:

```text
FM-1.fwsc
  package: db1642b2b6fa5c2cccb11ffd13878068bb28601678d3644049f99dc40e7edb8a
  app.bin: 306e47065f35d7a7a05ada7f5dd092f6e770952054f33f0a86b75fd10ffe3203
FM-1_093.fwsc
  package: ac69c2cd070a5fa606fb4170f190fee0aa593f73e8fdc58b638ac89269e39b48
  app.bin: 4dd80425cbd4713d9183c1b161001e0e60fdf1879372d37c6302433d5cb20112
```

## Package loading and presets

Load the complete `.fwsc` package. An extracted application `.bin` lacks the
stock flash directory, boot parameters and auxiliary factory presets.

The loader checks the package header/table, stored flash CRC, flash directory,
embedded configuration and chip key, and decrypted application CRCs. Both chip
keys decode to `0x980f`; the application lives at physical flash offset `0x4000`
and enters at `0x02000120`. The emulated startup supplies the boot parameters
and preserves the package's flash directory. Guest instructions execute without
firmware patches, replacement routines or instruction skips.

The official package's encrypted `USR` auxiliary payload decodes using its
logical package position and passes plaintext CRC `0xe3bc`. It is installed at
its declared flash reservation (`0xea000`, capacity `0x12000`), preserving the
application and key/configuration regions. The guest then loads its factory
voices, including **PIANO 1**.

Baud Girl's package contains identical `USR` ciphertext at a different package
position (`0xae400` instead of `0x93400`). Decoding at its declared position
produces CRC `0xbc95`, rather than the declared `0xe3bc`. The emulator prints:

```text
FWSC: USR checksum mismatch; USR preset data was not loaded
```

The application still loads. The emulator neither substitutes presets from
another package nor guesses a different encryption position. A physical update
may retain previously installed user data; persistent flash and migration of
existing device data have not been verified here. Other auxiliary updater
payloads are not installed by this application loader.

## Verification

The local stock regression boots both guest CPUs beyond the delayed temperature
ADC subscription, verifies LCD, watchdog, interrupt, ADC and audio DMA activity,
switches FX/HOME pages, then presses and releases a note. Idle audio is silent;
the held note produces nonzero stereo DMA samples. The test uses the same CPU,
bus and firmware loader as the graphical emulator.

The official trial completed **3,022,634,760 CPU steps** without a fault:
1,500,260 LCD pixels, 2,753 audio DMA halves, 2,140 ADC conversions and 493,010
watchdog feeds. The Baud Girl trial reached its intentional budget after
**3,292,769,970 CPU steps**, with 605,917 LCD pixels and 3,013 audio DMA halves.
Both runs continued past the formerly unsupported temperature channel.

The release build with GUI support passes all 183 regular tests. The two
external Felucca regressions also pass, covering note audio and the previously
failing FX page. The graphical executable was rebuilt; the stock results above
were verified with the shared core rather than automated native-window input.

The temperature input is a fixed hardware measurement, not a thermal model.
A watchdog/updater-compatible `FM-1_997` probe measured PMU mux source 3 at
349–351 over 60 samples (median 350). The firmware's ADC conversion completion,
restart and IRQ behavior remain active. `tools/build_temperature_probe.py`
reproduces that measurement firmware. After capturing the samples, the physical
FM-1 was restored to **FM-1_981**; neither supplied stock package was flashed.

Run the graphical emulator or the opt-in stock regression:

```sh
./emulator /path/to/FM-1.fwsc

mise exec -- env FM1_STOCK_FWSC=/path/to/FM-1.fwsc \
  cargo test --manifest-path rust-emulator/Cargo.toml \
  --release --locked --test stock -- --ignored --nocapture
```

For a bounded startup trace of either package:

```sh
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml \
  --release --locked --example diagnose -- /path/to/firmware.fwsc 1300000000
```

`diagnose` exits with status 1 for either a fault or an intentional instruction
limit; its last message distinguishes the two. CPU budgets are functional
emulation steps, not a claim of cycle-accurate timing. Full UI coverage, rotary
controls, host audio, USB MIDI/serial input and persistent flash remain open.
