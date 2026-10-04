#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Small pi32v2 machine-code interpreter. Python 3.10+, no dependencies."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct
import sys

MASK = 0xFFFFFFFF
XIP = 0x02000120
RAM = 0x01C00000
RAM_SIZE = 512 * 1024
RESULT = 0x01C08000
NAMES = ["constant", "add_wrap", "subtract", "xor", "and", "or", "not",
         "shift_left", "shift_right", "load_add", "loop_sum", "stack"]


class Fault(RuntimeError):
    pass


def signed(value, bits):
    return (value & ((1 << (bits - 1)) - 1)) - (value & (1 << (bits - 1)))


class CPU:
    def __init__(self, image, base=XIP):
        self.image, self.base = bytes(image), base
        self.ram = bytearray(RAM_SIZE)
        self.r = [0] * 16
        self.sr = [0] * 16  # rets=3, ssp=13, sp=14
        self.pc = base
        self.steps = 0
        self.counts = Counter()

    def read(self, addr, size):
        if addr % size:
            raise Fault(f"unaligned read at 0x{addr:08x}, size {size}")
        if self.base <= addr and addr + size <= self.base + len(self.image):
            data, offset = self.image, addr - self.base
        elif RAM <= addr and addr + size <= RAM + RAM_SIZE:
            data, offset = self.ram, addr - RAM
        else:
            raise Fault(f"unmapped read at 0x{addr:08x}, size {size}")
        return int.from_bytes(data[offset:offset + size], "little")

    def write(self, addr, value, size=4):
        if addr % size:
            raise Fault(f"unaligned write at 0x{addr:08x}, size {size}")
        if not RAM <= addr <= RAM + RAM_SIZE - size:
            raise Fault(f"unmapped/read-only write at 0x{addr:08x}")
        self.ram[addr - RAM:addr - RAM + size] = (value & ((1 << (8*size))-1)).to_bytes(size, "little")

    def push(self, value):
        self.sr[14] = (self.sr[14] - 4) & MASK
        self.write(self.sr[14], value)

    def pop(self):
        value = self.read(self.sr[14], 4)
        self.sr[14] = (self.sr[14] + 4) & MASK
        return value

    def step(self):
        pc = self.pc
        if not self.base <= pc < self.base + len(self.image):
            raise Fault(f"execution outside XIP at 0x{pc:08x}")
        h = self.read(pc, 2)
        a, b = h & 7, (h >> 4) & 7
        next_pc, op = pc + 2, ""
        if h & 0xFFF0 in (0xFFC0, 0xFFE0):
            value = self.read(pc + 2, 2) | (self.read(pc + 4, 2) << 16)
            n = h & 15
            if h & 0xFFF0 == 0xFFC0:
                self.r[n], op = value, "mov_imm32"
            elif n in (13, 14):
                self.sr[n], op = value, "stack_imm32"
            else:
                raise Fault(f"unsupported special register {n} at 0x{pc:08x}")
            next_pc = pc + 6
        elif h & 0xFFF0 == 0xE040:
            self.r[h & 15], next_pc, op = self.read(pc + 2, 2), pc + 4, "mov_imm16"
        elif h & 0xE0C0 == 0x2040:
            self.r[a] = (((h >> 3) & 7) << 5) | ((h >> 8) & 31)
            op = "mov_imm8"
        elif h & 0xE0F8 == 0x2010:
            self.r[a], op = (0xFFFFFFE0 | ((h >> 8) & 31)), "mov_negative"
        elif h & 0xFF00 == 0x1600:
            self.r[h & 15], op = self.r[(h >> 4) & 15], "mov_reg"
        elif h & 0xFE00 in (0x1C00, 0x1E00):
            c = ((h >> 7) & 3) * 2 + ((h >> 3) & 1)
            sub = h & 0xFE00 == 0x1E00
            self.r[a] = (self.r[b] - self.r[c] if sub else self.r[b] + self.r[c]) & MASK
            op = "sub" if sub else "add"
        elif h & 0xE0C0 == 0x20C0:
            imm = signed((((h >> 3) & 7) << 5) | ((h >> 8) & 31), 8)
            self.r[a], op = (self.r[a] + imm) & MASK, "add_imm8"
        elif h & 0xE088 == 0x8008:
            self.r[a], op = (self.r[b] + ((h >> 8) & 31)) & MASK, "add_small"
        elif h & 0xFF88 in (0x1900, 0x1908, 0x1980, 0x1988):
            # Vendor objdump: 0x1980 is AND; 0x1988 is NOT.
            kind = h & 0xFF88
            if kind == 0x1900:
                self.r[a], op = self.r[a] | self.r[b], "or"
            elif kind == 0x1908:
                self.r[a], op = self.r[a] ^ self.r[b], "xor"
            elif kind == 0x1980:
                self.r[a], op = self.r[a] & self.r[b], "and"
            else:
                self.r[a], op = (~self.r[b]) & MASK, "not"
        elif h & 0xE008 == 0xA000:
            shift = (h >> 8) & 31
            right = bool(h & 0x80)
            self.r[a] = (self.r[b] >> shift if right else self.r[b] << shift) & MASK
            op = "lsr" if right else "lsl"
        elif h & 0xE008 == 0x6000:
            addr = (self.r[b] + signed((h >> 8) & 31, 5) * 4) & MASK
            if h & 0x80:
                self.write(addr, self.r[a])
                op = "store32"
            else:
                self.r[a], op = self.read(addr, 4), "load32"
        elif h in (0xE8D8, 0xE8D4):
            mask = self.read(pc + 2, 2)
            regs = [n for n in range(16) if mask & (1 << n)]
            if h == 0xE8D8:
                for n in reversed(regs):
                    self.push(self.r[n])
                op = "push_mask"
            else:
                for n in regs:
                    self.r[n] = self.pop()
                op = "pop_mask"
            next_pc = pc + 4
        elif h in (0x0464, 0x0444):
            # Only this short-form register list is supported in v0.1.
            if h == 0x0464:
                self.push(self.r[4])
                op = "push_r4"
            else:
                self.r[4], op = self.pop(), "pop_r4"
        elif h & 0xFFC0 in (0xEA80, 0xEAC0):
            disp = signed(((h & 63) << 16) | self.read(pc + 2, 2), 22) * 2
            if h & 0xFFC0 == 0xEA80:
                self.sr[3] = pc + 4
                op = "call_rel22"
            else:
                op = "goto_rel22"
            next_pc = (pc + 4 + disp) & MASK
        elif h & 0xE00C == 0x8004:
            disp = signed(((h & 3) << 10) | (((h >> 4) & 15) << 6) | (((h >> 8) & 31) << 1), 12)
            next_pc, op = (pc + 2 + disp) & MASK, "goto_rel12"
        elif h & 0xE008 == 0x4000:
            disp = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9)
            nonzero = bool(h & 0x80)
            if (self.r[a] != 0) == nonzero:
                next_pc = (pc + 2 + disp) & MASK
            op = "branch_nonzero" if nonzero else "branch_zero"
        elif h == 0x0080:
            next_pc, op = self.sr[3], "return"
        else:
            raise Fault(f"unsupported instruction at 0x{pc:08x}: {h:04x}")
        self.pc = next_pc
        self.steps += 1
        self.counts[op] += 1
        return {"pc": f"0x{pc:08x}", "word": f"0x{h:04x}", "op": op,
                "next_pc": f"0x{self.pc:08x}", "registers": self.r.copy(), "sp": self.sr[14]}

    def run(self, stop, limit=10000, trace=None):
        while self.pc != stop:
            if self.steps >= limit:
                raise Fault(f"instruction limit {limit} at 0x{self.pc:08x}")
            event = self.step()
            if trace:
                trace.write(json.dumps(event) + "\n")


def expected():
    a, b = 0xA5A55A5A, 0x12345678
    return dict(zip(NAMES, [b, 1, MASK-1, a ^ b, a & b, a | b, (~b) & MASK,
                           (b << 4) & MASK, b >> 4, b+7, sum(range(1, 11)), 0x13579BDF]))


def execute(binary, manifest, trace=None):
    image = Path(binary).read_bytes()
    if hashlib.sha256(image).hexdigest() != manifest["image_sha256"]:
        raise Fault("image hash does not match manifest; rebuild both")
    cpu = CPU(image, manifest["base"])
    # Nonzero registers expose missing save/restore operations.
    cpu.r = [(0x10203040 + n * 0x01010101) & MASK for n in range(16)]
    before = cpu.r.copy()
    cpu.run(manifest["stop"], trace=trace)
    values = {name: cpu.read(RESULT + i*4, 4) for i, name in enumerate(NAMES)}
    if cpu.r[1:] != before[1:] or cpu.sr[14] != 0x01C7A000:
        raise Fault("probe did not preserve registers or stack")
    return {"format": "fm1-probe-v1", "source": "emulator", "hardware_validated": False,
            "probe_sha256": manifest["probe_sha256"], "values": values,
            "expected_match": values == expected(), "instructions": cpu.steps,
            "coverage": dict(sorted(cpu.counts.items()))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="build/probe.bin")
    parser.add_argument("--manifest", default="build/probe.json")
    parser.add_argument("--output")
    parser.add_argument("--trace")
    args = parser.parse_args()
    try:
        manifest = json.loads(Path(args.manifest).read_text())
        if args.trace:
            with open(args.trace, "w") as trace:
                result = execute(args.binary, manifest, trace)
        else:
            result = execute(args.binary, manifest)
        output = json.dumps(result, indent=2) + "\n"
        if args.output:
            Path(args.output).write_text(output)
        print(output, end="")
        return 0 if result["expected_match"] else 1
    except (Fault, OSError, ValueError, KeyError) as error:
        print(f"emu: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
