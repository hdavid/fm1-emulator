# SPDX-License-Identifier: GPL-3.0-only
"""Check shipped package identity, checksums and decoded app against its build."""
import hashlib
import json
from pathlib import Path
import struct
import sys
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools'))
import fm1_install as install
import fm1pkg_make as pkg

class PackageTests(unittest.TestCase):
    def test_artifact_integrity(self):
        manifest = json.loads((ROOT/'build/fm1-diag.json').read_text())
        for name, record in manifest['files'].items():
            data = (ROOT/'build'/name).read_bytes()
            self.assertEqual(len(data), record['bytes'], name)
            self.assertEqual(hashlib.sha256(data).hexdigest(), record['sha256'], name)

    def test_package_contains_built_app(self):
        product, image = install.load_package(ROOT/'build/fm1-diag.fwsc', False)
        self.assertEqual(product, 'FM-1_980')
        header = bytearray(image[:0x40])
        pkg.enc(header, 0, len(header))
        self.assertEqual(pkg.crc16(header[2:]), struct.unpack_from('<H', header)[0])
        count = struct.unpack_from('<H', header, 8)[0]
        table = image[0x40:0x40+count*0x50]
        self.assertEqual(pkg.crc16(table), struct.unpack_from('<H', header, 2)[0])
        flash_off = None
        for i in range(count):
            entry = bytearray(table[i*0x50:(i+1)*0x50])
            pkg.enc(entry, 0, len(entry))
            offset, size = struct.unpack_from('<II', entry, 8)
            self.assertEqual(pkg.crc16(image[offset:offset+size]), struct.unpack_from('<H', entry, 4)[0])
            if entry[0x40:].split(b'\0')[0] == b'flash.bin':
                flash_off = offset
        self.assertIsNotNone(flash_off)
        area = bytearray(image[flash_off+0x4000:flash_off+0x93000])
        pkg.sfc(area, 0, len(area), 0, pkg.KEY)
        app = (ROOT/'build/fm1-diag.bin').read_bytes()
        self.assertEqual(area[0x120:0x120+len(app)], app)
        self.assertEqual(area[0x120+len(app):0x120+pkg.APP_SLOT], b'\xff'*(pkg.APP_SLOT-len(app)))

if __name__ == '__main__':
    unittest.main()
