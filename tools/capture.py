#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["pyserial==3.5"]
# ///
# SPDX-License-Identifier: GPL-3.0-only
"""Run the probe command over USB serial. No firmware writes."""
import argparse
from pathlib import Path
import sys
import time

import serial
from serial.tools import list_ports
from compare import parse_capture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port", nargs="?")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--output", type=Path, default=Path("hardware.txt"))
    args = parser.parse_args()
    if args.list:
        for item in list_ports.comports():
            print(f"{item.device}: {item.description}")
        return 0
    if not args.port:
        parser.error("select a port; use --list to show ports")
    capture = bytearray()
    try:
        with serial.Serial(args.port, 115200, timeout=0.1, write_timeout=2) as port:
            port.dtr = True
            # Wait for the console to see DTR. Drain the optional greeting.
            until = time.monotonic() + 1
            while time.monotonic() < until:
                port.read(4096)
            port.write(b"\rprobe\r")
            until = time.monotonic() + 5
            while time.monotonic() < until:
                capture.extend(port.read(4096))
                if b"FM1PROBE END" in capture:
                    break
        text = capture.decode("ascii", errors="replace")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)  # Retain incomplete output for device debugging too.
        parse_capture(text)
        print(f"Saved probe output to {args.output}")
        return 0
    except (serial.SerialException, OSError, ValueError) as error:
        print(f"capture: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
