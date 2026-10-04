#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build a separate application-entry firmware for emulator foundations."""
import struct
from build import tool, ROOT


def main(display=False):
    directory = "build/display" if display else "build/foundation"
    out = ROOT / directory
    out.mkdir(parents=True, exist_ok=True)
    sources = [("foundation", "start"), ("probe", "probe")]
    if display:
        sources.append(("display", "display"))
    for source, name in sources:
        tool("pi32v2/bin/clang", "-target", "pi32v2", "-c",
             *(["-DFM1_DISPLAY"] if display else []),
             f"firmware/{source}.S", "-o", f"{directory}/{name}.o")
    tool("pi32v2/bin/ld", "-e", "_start", "-T", "firmware/app.ld",
         *[f"{directory}/{name}.o" for _, name in sources],
         "-o", f"{directory}/firmware.elf")
    for option, name in [("-d", "firmware.dis"), ("-t", "firmware.symbols")]:
        text = tool("common/bin/objdump", option, f"{directory}/firmware.elf")
        (out / name).write_text("\n".join(line.rstrip() for line in text.splitlines()) + "\n")
    elf = (out / "firmware.elf").read_bytes()
    phoff = struct.unpack_from("<I", elf, 28)[0]
    phsize, phcount = struct.unpack_from("<HH", elf, 42)
    image = bytearray()
    base = 0x02000120
    for i in range(phcount):
        kind, off, _, load, size, _, _, _ = struct.unpack_from("<IIIIIIII", elf, phoff + i * phsize)
        if kind != 1 or not size or load + size <= base or load >= 0x02100000:
            continue
        start = max(load, base)
        source = elf[off + start - load:off + size]
        end = start - base + len(source)
        image.extend(b"\xff" * max(0, end - len(image)))
        image[start - base:end] = source
    (out / "firmware.bin").write_bytes(image)
    print(f"Built {len(image)} bytes of application firmware for emulator testing.")
    print("No update package, USB updater, or recovery is included; do not flash this image.")


if __name__ == "__main__":
    main()
