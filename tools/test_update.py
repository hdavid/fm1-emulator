#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Run the inherited updater simulations; never open MIDI or a serial port."""
from pathlib import Path
import os
import subprocess
import tempfile
import sys
import fm1pkg_make as pkg
ROOT = Path(__file__).resolve().parents[1]

def main():
    cc = os.environ.get('CC', 'cc')
    with tempfile.TemporaryDirectory() as d:
        tmp = Path(d)
        for name in ('ota', 'ldr'):
            subprocess.run([cc, '-O2', '-o', str(tmp/name), str(ROOT/'tests'/f'{name}_test.c')], check=True)
        new = ROOT/'build/fm1-diag.fwsc'
        # A different but well-formed package exercises actual sector writes in
        # simulated NOR. This fixture is never offered as installable firmware.
        old = bytearray((ROOT/'build/fm1-diag.bin').read_bytes())
        old[64] ^= 1
        baseline = pkg.ufw(pkg.flash_image(old, pkg.KEY), (ROOT/'build/loader/ota.bin').read_bytes(), 'FM-1_979')
        (tmp/'baseline.fwsc').write_bytes(baseline)
        subprocess.run([str(tmp/'ota'), str(new)], check=True)
        subprocess.run([str(tmp/'ldr'), str(tmp/'baseline.fwsc'), str(new)], check=True)
        subprocess.run([sys.executable, str(ROOT/'tests/install_test.py')], cwd=ROOT, check=True)

if __name__ == '__main__':
    main()
