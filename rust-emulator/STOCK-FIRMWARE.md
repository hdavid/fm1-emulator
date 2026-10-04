# Stock and Baud Girl firmware boot trials (2026-10-04)

Neither supplied application boots in the emulator at commit
`61e779cc05094f4d35e893d2924ce6436f6b0de9`. Both execute three instructions,
then fault on an unsupported six-byte relative call at `0x02000130`.
Neither reaches LCD, USB, watchdog, interrupts, ADC or audio initialization.
This identifies an emulator instruction gap; it does not establish that either
firmware is defective or verify its behavior on physical hardware.

## Inputs

The user supplied one official and one Baud Girl package. The decoded identities
and application comparison identify `FM-1.fwsc` as the `FM-1_015` baseline and
`FM-1_093.fwsc` as the modified image. Authorship and download authenticity were
not independently verified.

| File in `~/Downloads` | Package identity | Package bytes | Extracted application bytes |
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

The modified application adds 110,916 bytes and changes 2,001 bytes in 177
contiguous regions of the shared application range. Its first changed byte is
at application offset `0x17c6`; the startup sequence through the fault is
identical. Both contain their respective product strings at `0x0204eb84`.
These are binary observations, not a reconstruction of the modification source.

## Extraction and integrity

The original packages were read without modification or device access. The
existing `tools/fm1_install.py` decoder removes the twenty package identity
marker bytes. `tools/fm1pkg_make.py` supplies the existing ENC, SFC and CRC16
implementations used for read-only extraction.

For both inputs these checks pass:

- Outer package header and file-table CRCs.
- Complete stored `flash.bin` CRC.
- Flash header and four top-level directory entry CRCs.
- Embedded flash `isd_config.ini` CRC and its chip-key blob CRC.
- Decrypted application-area header, area contents, `app.bin` entry and
  complete `app.bin` payload CRCs.

Both embedded chip keys decode to `0x980f`. The application area starts at
physical flash offset `0x4000`; its `app.bin` starts at area offset `0x120`
and executes at `0x02000120`. Extraction uses each package's declared lengths,
including the larger application area in `FM-1_093`, rather than the Felucca
packager's fixed application slot size.

The separate outer `USR`, `isd_config.ini`, `script.ver` and `blimit.bin`
payloads do not match their directory CRCs when checked as stored bytes. Their
plaintext encoding was not decoded or validated in this trial. Those stored
payloads and CRC fields are identical between the two packages. Consequently,
the checks above establish integrity of the extracted boot applications, not
complete validation of every update-package component or the update process.

## Boot result and first blocker

Each unchanged application was run with the release-mode `diagnose` example,
which uses the same firmware loader, CPU and bus as the GUI. The requested
instruction budget was 100,000,000; both faulted after three completed
instructions, well before that limit. Startup `r0` was the runner's normal
`0x01c7fe08`. All reported peripheral counters were zero, the LCD was invisible
and neither application emitted USB CDC output.

The installed vendor pi32v2 assembler/linker/disassembler was used through the
existing Docker tool wrapper. Temporary ELF wrappers place the exact application
bytes in a text section at their execution address; they do not change the
application bytes used for the boot tests. Both disassemblies begin:

```text
02000120: 04 81                goto 0x02000124
02000124: ee ff d0 6f c1 01    sp = 0x01c16fd0
0200012a: ed ff d0 7f c1 01    ssp = 0x01c17fd0
02000130: 80 ff b0 00 00 00    call 0x020001e6
```

The fault is `unsupported instruction 0xff80 at PC 0x02000130`. The target
begins by saving `rets` and making another six-byte relative call. The CPU
currently implements short and 22-bit relative calls plus register calls, but
not this long relative form. No application patches or instruction skips were
used. The next implementation step is to support this call form, verify its
signed displacement and return address against vendor-generated instructions,
then rerun both images to identify subsequent blockers. These trials provide
no evidence about later stock peripheral or ROM compatibility.

## Local reproduction

Ignored local trial artifacts are in `.deps/firmware-trial/`: `extract.py`,
`disassemble.py`, and per-file directories containing `app.bin`, `metadata.json`,
`app.dis`, `boot.log` and `stdout.log`. Firmware binaries are not committed.

```sh
mise exec -- .venv/bin/python .deps/firmware-trial/extract.py \
  "$HOME/Downloads/FM-1.fwsc" "$HOME/Downloads/FM-1_093.fwsc"
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml \
  --release --offline --example diagnose -- \
  .deps/firmware-trial/FM-1/app.bin 100000000
mise exec -- cargo run --manifest-path rust-emulator/Cargo.toml \
  --release --offline --example diagnose -- \
  .deps/firmware-trial/FM-1_093/app.bin 100000000
```

`diagnose` exits with status 1 for a fault or an intentional instruction limit;
read its final message to distinguish them. The GUI launcher accepts extracted
application `.bin` or executable `.elf` files; packed `.fwsc` containers are
not directly supported. GUI presentation and physical flashing were not part
of these boot trials.
