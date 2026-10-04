# SPDX-License-Identifier: GPL-3.0-only
import io
import json
from pathlib import Path
import random
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from emu import CPU, Fault, MASK, RAM, XIP, execute, expected, NAMES
from tools.compare import compare, parse_capture


class EmulatorTests(unittest.TestCase):
    def test_vendor_built_firmware(self):
        manifest = json.loads((ROOT/"build/probe.json").read_text())
        trace = io.StringIO()
        result = execute(ROOT/"build/probe.bin", manifest, trace)
        self.assertEqual(result["values"], expected())
        self.assertEqual(result["instructions"], 75)
        self.assertEqual(len(trace.getvalue().splitlines()), 75)
        self.assertFalse(result["hardware_validated"])

    def test_logic_and_arithmetic_edges(self):
        # Encodings are from the vendor-generated probe.dis, not an encoder
        # shared with the interpreter. Expected values use Python arithmetic.
        rng = random.Random(1201)
        pairs = [(0, 0), (MASK, 1), (0x80000000, MASK), (0xA5A55A5A, 0x12345678)]
        pairs += [(rng.getrandbits(32), rng.getrandbits(32)) for _ in range(128)]
        cases = [("911c", lambda a, b: (a+b)&MASK), ("911e", lambda a, b: (a-b)&MASK),
                 ("2919", lambda a, b: a^b), ("a119", lambda a, b: a&b),
                 ("2119", lambda a, b: a|b), ("a919", lambda a, b: (~b)&MASK),
                 ("21a4", lambda a, b: (b<<4)&MASK), ("a1a4", lambda a, b: b>>4)]
        for opcode, oracle in cases:
            for a, b in pairs:
                cpu = CPU(bytes.fromhex(opcode))
                cpu.r[1], cpu.r[2] = a, b
                cpu.step()
                self.assertEqual(cpu.r[1], oracle(a, b), (opcode, a, b))

    def test_backward_branch_taken_and_not_taken(self):
        for value, target in [(0, XIP+2), (1, XIP-4), (MASK, XIP-4)]:
            cpu = CPU(bytes.fromhex("f35d"))
            cpu.r[3] = value
            cpu.step()
            self.assertEqual(cpu.pc, target)

    def test_stack_layout_and_register_preservation(self):
        cpu = CPU(bytes.fromhex("d8e8fe00d4e8fe00"))
        cpu.sr[14] = RAM+256
        cpu.r = list(range(16))
        cpu.step()
        self.assertEqual(cpu.sr[14], RAM+228)
        self.assertEqual([cpu.read(RAM+228+i*4, 4) for i in range(7)], list(range(1, 8)))
        cpu.r[1:8] = [99]*7
        cpu.step()
        self.assertEqual(cpu.r, list(range(16)))
        self.assertEqual(cpu.sr[14], RAM+256)

    def test_memory_guards(self):
        cpu = CPU(b"\x00\x00")
        for address in (RAM-4, RAM+512*1024, XIP, 0x12E00, RAM+1):
            with self.assertRaises(Fault):
                cpu.write(address, 1)
        with self.assertRaises(Fault):
            cpu.read(0x12E00, 4)
        cpu.write(RAM, 0x12345678)
        self.assertEqual(cpu.ram[:4], bytes.fromhex("78563412"))

    def test_unsupported_instruction_and_limit(self):
        with self.assertRaisesRegex(Fault, "unsupported instruction"):
            CPU(bytes.fromhex("ffff")).step()
        cpu = CPU(bytes.fromhex("f79f"))
        with self.assertRaisesRegex(Fault, "instruction limit"):
            cpu.run(XIP+2, limit=3)
        self.assertEqual(cpu.steps, 3)

    def test_manifest_integrity(self):
        manifest = json.loads((ROOT/"build/probe.json").read_text())
        manifest["image_sha256"] = "bad"
        with self.assertRaisesRegex(Fault, "hash"):
            execute(ROOT/"build/probe.bin", manifest)


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.result = {"format": "fm1-probe-v1", "probe_sha256": "a"*64, "values": expected()}
        self.capture = ("Synthetic unit-test data; not a hardware measurement.\nFM1PROBE 1 " + "a"*64 + "\n" +
                        "\n".join(f"W {i} {expected()[name]:08X}" for i, name in enumerate(NAMES)) +
                        "\nFM1PROBE END\n> ")

    def test_equal_and_changed_word(self):
        self.assertEqual(compare(self.result, self.capture), [])
        changed = self.capture.replace("W 0 12345678", "W 0 12345679")
        self.assertEqual(compare(self.result, changed), [("constant", 0x12345678, 0x12345679)])

    def test_missing_duplicate_and_wrong_build(self):
        with self.assertRaisesRegex(ValueError, "Expected 12"):
            parse_capture(self.capture.replace("W 0 12345678\n", ""))
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            parse_capture(self.capture.replace("W 1", "W 0"))
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            compare(self.result, self.capture.replace("a"*64, "b"*64))


if __name__ == "__main__":
    unittest.main()
