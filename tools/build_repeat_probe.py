# SPDX-License-Identifier: GPL-3.0-only
"""Build a watchdog/update-compatible register-repeat probe under .deps/."""
import hashlib
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/repeat').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
counts = [0, 1, 2, 4, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 129, 256, 1024]
assembly = ['\n.section .text.repeat_probe,"ax",@progbits',
            '.global repeat_probe', 'repeat_probe:', '[--sp] = {r7-r1}',
            'r7 = psr']
for i, count in enumerate(counts):
    assembly += ['r1 = 0', f'r2 = {count}', 'rep r2 {', 'r1 += 1', '}',
                 f'[r0 + {i * 8}] = r1', f'[r0 + {i * 8 + 4}] = r2']
# Stock memcpy overwrites the register that supplied the repeat count.
assembly += ['r1 = 0', 'r2 = 4', 'rep r2 {', 'r1 += 1', 'r2 = 0x99', '}',
             f'[r0 + {len(counts) * 8}] = r1',
             f'[r0 + {len(counts) * 8 + 4}] = r2']
for i, overwrite in enumerate([False, True]):
    assembly += [f'r3 = r0 + {(len(counts) + 1) * 8 + i * 20}',
                 'r2 = 4', 'rep r2 {', '[r3++=4] = r2']
    if overwrite:
        assembly += ['r2 = 0x99']
    assembly += ['}', '[r3] = r2']
assembly += [
             'psr = r7', '{r7-r1} = [sp++]', 'rts']
with (src / 'probe.S').open('a') as f:
    f.write('\n'.join(assembly) + '\n')
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
                           'extern void fm1_probe(uint32_t *out);\n'
                           'extern void repeat_probe(uint32_t *out);')
report = f'''static void repeat_report(void) {{
    uint32_t out[{(len(counts) + 1) * 2 + 10}]={{0}},i;
    uint32_t f=irq_save();repeat_probe(out);irq_restore(f);
    con_puts("REPEAT BEGIN\\r\\n");
    for(i=0;i<sizeof(out)/sizeof(out[0]);i++) {{
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }}
    con_puts("REPEAT END\\r\\n");
}}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
                    'if(equal(con.line,"probe")) probe_report();\n'
                    '            else if(equal(con.line,"repeat")) repeat_report();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_983'
b.NAME = 'fm1-repeat'
b.HARDWARE_DISPLAY = False
b.GEN.mkdir(parents=True, exist_ok=True)
(b.GEN / 'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK = Path('.deps/sdk').resolve()
ota = b.build_loader()
img, syms, dis, rt = b.build_app()
errors, notes = b.check(img, syms, dis, rt)
assert not errors, errors
package = pkg.ufw(pkg.flash_image(img, pkg.KEY), ota, b.PRODUCT)
(b.OUT / 'fm1-repeat.fwsc').write_bytes(package)
(b.OUT / 'fm1-repeat.symbols').write_text(syms)
print('\n'.join(notes))
print('Built watchdog/update-compatible', b.PRODUCT, 'app', len(img),
      'bytes, package SHA256', hashlib.sha256(package).hexdigest())
