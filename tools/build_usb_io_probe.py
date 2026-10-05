# SPDX-License-Identifier: GPL-3.0-only
"""Read USB I/O status before and after CDC init, retaining watchdog/updater."""
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/usb-io').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
p = src / 'src/diag.c'
code = p.read_text()
report = '''static uint32_t usb_io_boot[3];
static void usb_io_report(void) {
    uint32_t i;
    con_puts("USBIO BEGIN\\r\\n");
    for(i=0;i<6;i++) {
        uint32_t v=i<3?usb_io_boot[i]:
            *(volatile uint32_t *)(0x51000u+(i-3u)*4u);
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(v,8);con_puts("\\r\\n");
    }
    con_puts("USBIO END\\r\\n");
}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
    'if(equal(con.line,"probe")) probe_report();\n'
    '            else if(equal(con.line,"usbio")) usb_io_report();')
code = code.replace('    fm1_audio_stop();\n    f=irq_save();',
    '    usb_io_boot[0]=*(volatile uint32_t *)0x51000u;\n'
    '    usb_io_boot[1]=*(volatile uint32_t *)0x51004u;\n'
    '    usb_io_boot[2]=*(volatile uint32_t *)0x51008u;\n'
    '    fm1_audio_stop();\n    f=irq_save();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_997'
b.NAME = 'fm1-usb-io'
b.HARDWARE_DISPLAY = False
b.GEN.mkdir(parents=True, exist_ok=True)
(b.GEN / 'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK = Path('.deps/sdk').resolve()
ota = b.build_loader()
img, syms, dis, rt = b.build_app()
errors, notes = b.check(img, syms, dis, rt)
assert not errors, errors
(b.OUT / 'fm1-usb-io.fwsc').write_bytes(
    pkg.ufw(pkg.flash_image(img, pkg.KEY), ota, b.PRODUCT))
(b.OUT / 'fm1-usb-io.symbols').write_text(syms)
print('\n'.join(notes))
print('Built watchdog/update-compatible', b.PRODUCT)
