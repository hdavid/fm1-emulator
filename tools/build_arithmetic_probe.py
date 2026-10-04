# SPDX-License-Identifier: GPL-3.0-only
"""Build a watchdog/update-compatible arithmetic probe under .deps/."""
import hashlib
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/arithmetic').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
cases = [(False, 0xffffffff, 1), (False, 0, 0),
         (False, 0x7fffffff, 1), (True, 0, 1), (True, 1, 0),
         (True, 0, 0), (True, 0x80000000, 1)]
assembly = ['\n.section .text.arithmetic_probe,"ax",@progbits',
            '.global arithmetic_probe', 'arithmetic_probe:',
            '[--sp] = {r7-r1}', 'r7 = psr']
for i, (subtract, left, right) in enumerate(cases):
    assembly += [f'r1 = {left}', f'r2 = {right}',
                 f'r3 = r1 {"-" if subtract else "+"} r2', 'r4 = psr',
                 f'r5 = r1 {"- r2 - !c" if subtract else "+ r2 + c"}',
                 f'[r0 + {i * 12}] = r3', f'[r0 + {i * 12 + 4}] = r4',
                 f'[r0 + {i * 12 + 8}] = r5']
for i, shift in enumerate([0, 1, 32, 63, 64, 65, 80]):
    assembly += ['r2 = 0xfedcba98', 'r3 = 0x81234567', f'r4 = {shift}',
                 'r3_r2 <<= r4', f'[r0 + {84 + i * 8}] = r2',
                 f'[r0 + {88 + i * 8}] = r3']
assembly += ['psr = r7', '{r7-r1} = [sp++]', 'rts']
with (src / 'probe.S').open('a') as f:
    f.write('\n'.join(assembly) + '\n')
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
                           'extern void fm1_probe(uint32_t *out);\n'
                           'extern void arithmetic_probe(uint32_t *out);')
report = '''static void arithmetic_report(void) {
    uint32_t out[39]={0},i,f=irq_save();
    volatile uint32_t *lrct=(volatile uint32_t *)0x13600;
    arithmetic_probe(out);
    for(i=0;i<2;i++) {
        uint32_t n;
        lrct[0]=64;lrct[0]=0;lrct[0]=1|(i<<1);
        for(n=0;n<400000;n++) __asm__ volatile("nop");
        out[35+i*2]=lrct[0];out[36+i*2]=lrct[1];lrct[0]=64;lrct[0]=0;
    }
    irq_restore(f);
    con_puts("ARITHMETIC BEGIN\\r\\n");
    for(i=0;i<sizeof(out)/sizeof(out[0]);i++) {
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }
    con_puts("ARITHMETIC END\\r\\n");
}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
                    'if(equal(con.line,"probe")) probe_report();\n'
                    '            else if(equal(con.line,"arithmetic")) arithmetic_report();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_984'
b.NAME = 'fm1-arithmetic'
b.HARDWARE_DISPLAY = False
b.GEN.mkdir(parents=True, exist_ok=True)
(b.GEN / 'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK = Path('.deps/sdk').resolve()
ota = b.build_loader()
img, syms, dis, rt = b.build_app()
errors, notes = b.check(img, syms, dis, rt)
assert not errors, errors
package = pkg.ufw(pkg.flash_image(img, pkg.KEY), ota, b.PRODUCT)
(b.OUT / 'fm1-arithmetic.fwsc').write_bytes(package)
(b.OUT / 'fm1-arithmetic.symbols').write_text(syms)
print('\n'.join(notes))
print('Built watchdog/update-compatible', b.PRODUCT, 'app', len(img),
      'bytes, package SHA256', hashlib.sha256(package).hexdigest())
