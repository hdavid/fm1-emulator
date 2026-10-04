#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Verify that the hardware ELF contains the identical probe machine code."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile

from build import tool, ROOT


def check(path):
    symbols = tool("common/bin/objdump", "-t", str(path.resolve()))
    headers = tool("common/bin/objdump", "-h", str(path.resolve()))
    def symbol(name):
        return int(re.search(r"^([0-9a-f]+) .*\s" + name + r"$", symbols, re.M)[1], 16)
    base = int(re.search(r"^\s*\d+\s+\.text\s+[0-9a-f]+\s+([0-9a-f]+)", headers, re.M)[1], 16)
    # Temp output must be in ROOT for the Docker build route.
    with tempfile.TemporaryDirectory(dir=ROOT/"build") as directory:
        raw = Path(directory)/"text.bin"
        tool("common/bin/objcopy", "-O", "binary", "-j", ".text", str(path.resolve()), str(raw))
        body = raw.read_bytes()[symbol("fm1_probe")-base:symbol("fm1_probe_end")-base]
    manifest = json.loads((ROOT/"build/probe.json").read_text())
    digest = hashlib.sha256(body).hexdigest()
    if digest != manifest["probe_sha256"]:
        raise SystemExit(f"Probe mismatch: {digest}, expected {manifest['probe_sha256']}")
    print(f"PASS: hardware ELF has the same {len(body)} probe bytes ({digest}).")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("elf", type=Path)
    check(parser.parse_args().elf)
