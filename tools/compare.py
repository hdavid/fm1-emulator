#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Compare emulator JSON with the output of Felucca's probe command."""
import argparse
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from emu import NAMES


def parse_capture(text):
    # Use the final complete capture. Ignore prompts and command echo.
    matches = list(re.finditer(r"FM1PROBE 1 ([0-9a-f]{64})\r?\n(.*?)FM1PROBE END", text, re.S))
    if not matches:
        raise ValueError("No complete FM1PROBE block. Run 'probe' in the USB serial console.")
    match = matches[-1]
    words = {}
    for line in match[2].splitlines():
        item = re.fullmatch(r"W (\d+) ([0-9A-Fa-f]{8})", line.strip())
        if not item:
            raise ValueError(f"Invalid result line: {line!r}")
        index = int(item[1])
        if index in words or not 0 <= index < len(NAMES):
            raise ValueError(f"Duplicate or invalid result index: {index}")
        words[index] = int(item[2], 16)
    if len(words) != len(NAMES):
        raise ValueError(f"Expected {len(NAMES)} words; got {len(words)}")
    return match[1], {name: words[i] for i, name in enumerate(NAMES)}


def compare(emulated, capture):
    digest, hardware = parse_capture(capture)
    if emulated.get("format") != "fm1-probe-v1" or digest != emulated["probe_sha256"]:
        raise ValueError("Probe version/hash mismatch. Use matching firmware and emulator builds.")
    if set(emulated["values"]) != set(NAMES):
        raise ValueError("Emulator result fields do not match the probe format")
    return [(name, emulated["values"][name], hardware[name]) for name in NAMES
            if emulated["values"][name] != hardware[name]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("emulator", type=Path)
    parser.add_argument("hardware_log", type=Path)
    args = parser.parse_args()
    try:
        differences = compare(json.loads(args.emulator.read_text()), args.hardware_log.read_text())
        for name, emu, real in differences:
            print(f"FAIL {name}: emulator=0x{emu:08x} device=0x{real:08x}")
        if not differences:
            print("PASS: all 12 words match the supplied capture. Capture origin is not authenticated.")
        return bool(differences)
    except (ValueError, KeyError, OSError) as error:
        print(f"compare: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
