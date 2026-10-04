#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build the test with the vendor pi32v2 assembler and linker."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TC = Path(os.environ.get("JIELI_TOOLCHAIN", str(Path.home()/".jieli/toolchain"))).resolve()


def tool(name, *args):
    command = [str(TC/name), *map(str, args)]
    if platform.system() != "Linux" or platform.machine() not in ("x86_64", "AMD64"):
        command = ["docker", "run", "--rm", "--platform", "linux/amd64",
                   "-v", f"{ROOT}:{ROOT}", "-v", f"{TC}:{TC}:ro", "-w", str(ROOT),
                   "debian:bookworm-slim", *command]
    return subprocess.check_output(command, cwd=ROOT, text=True)


def build():
    if not (TC/"pi32v2/bin/clang").exists():
        raise SystemExit("Set JIELI_TOOLCHAIN to the directory containing pi32v2/ and common/.")
    out = ROOT/"build"
    out.mkdir(exist_ok=True)
    for name in ("start", "probe"):
        tool("pi32v2/bin/clang", "-target", "pi32v2", "-c", f"firmware/{name}.S", "-o", f"build/{name}.o")
    tool("pi32v2/bin/ld", "-T", "firmware/probe.ld", "build/start.o", "build/probe.o", "-o", "build/probe.elf")
    tool("common/bin/objcopy", "-O", "binary", "-j", ".text", "build/probe.elf", "build/probe.bin")
    dis = tool("common/bin/objdump", "-d", "build/probe.elf")
    symbols = tool("common/bin/objdump", "-t", "build/probe.elf")
    def symbol(name):
        return int(re.search(r"^([0-9a-f]+) .*\s" + re.escape(name) + r"$", symbols, re.M)[1], 16)
    base = symbol("_start")
    data = (out/"probe.bin").read_bytes()
    body = data[symbol("fm1_probe")-base:symbol("fm1_probe_end")-base]
    manifest = {"base": base, "stop": symbol("probe_done"), "result": 0x01C08000,
                "words": 12, "probe_start": symbol("fm1_probe"), "probe_end": symbol("fm1_probe_end"),
                "image_sha256": hashlib.sha256(data).hexdigest(),
                "probe_source_sha256": hashlib.sha256((ROOT/"firmware/probe.S").read_bytes()).hexdigest(),
                "probe_sha256": hashlib.sha256(body).hexdigest(),
                "toolchain": tool("pi32v2/bin/clang", "--version").splitlines()[0],
                "felucca_commit": "1e838e17e170b20ff09b9660c9a7171aadfc5dca"}
    (out/"probe.json").write_text(json.dumps(manifest, indent=2)+"\n")
    (out/"probe.dis").write_text(dis)
    (out/"probe.symbols").write_text(symbols)
    print(f"Built {len(data)} bytes; probe SHA256 {manifest['probe_sha256']}")


if __name__ == "__main__":
    build()
