#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Draw macos/icon-placeholder.png: a PLACEHOLDER app icon (a dark rounded square
with a teal "LCD" and four knobs), made with the standard library only.

It is not artwork: replace macos/icon-placeholder.png with a real 1024x1024 PNG
and bundle-macos.sh will use it unchanged. Run: python3 make-placeholder-icon.py
"""
import math
import struct
import sys
import zlib
from pathlib import Path

SIZE = 1024
SAMPLES = 2  # per axis, for smooth edges


def inside_rounded_rect(x, y, left, top, right, bottom, radius):
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius**2


def colour_at(x, y):
    """RGBA of the design at a point (alpha 0 outside the square)."""
    if not inside_rounded_rect(x, y, 100, 100, 924, 924, 190):
        return (0, 0, 0, 0)
    if inside_rounded_rect(x, y, 200, 200, 824, 520, 36):
        # The LCD: a teal glow, brighter toward the middle.
        glow = 1 - min(1, math.hypot(x - 512, y - 360) / 420)
        return (int(40 + 60 * glow), int(120 + 90 * glow), int(110 + 70 * glow), 255)
    for centre in (275, 425, 575, 725):
        if math.hypot(x - centre, y - 700) <= 62:
            ring = math.hypot(x - centre, y - 700) > 46
            return (231, 193, 91, 255) if ring else (190, 194, 193, 255)
    return (31, 35, 37, 255)


def pixel(px, py):
    total = [0, 0, 0, 0]
    for sy in range(SAMPLES):
        for sx in range(SAMPLES):
            r, g, b, a = colour_at(px + (sx + 0.5) / SAMPLES, py + (sy + 0.5) / SAMPLES)
            total[0] += r * a
            total[1] += g * a
            total[2] += b * a
            total[3] += a
    alpha = total[3]
    if alpha == 0:
        return (0, 0, 0, 0)
    n = SAMPLES * SAMPLES
    return (total[0] // alpha, total[1] // alpha, total[2] // alpha, alpha // n)


def png(rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + bytes(v for p in row for v in p) for row in rows)
    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).with_name("icon-placeholder.png")
    out.write_bytes(png([[pixel(x, y) for x in range(SIZE)] for y in range(SIZE)]))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
