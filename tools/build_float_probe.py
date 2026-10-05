# SPDX-License-Identifier: GPL-3.0-only
"""Build an update/watchdog-compatible probe for stock r3 FP instructions."""
import json
import shutil
from pathlib import Path
import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/float-probe').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
# Each opcode writes r3; binary inputs are r1/r2, conversions read r1.
cases = []
for op in [0, 1, 2, 3, 5, 6, 7, 8]:
    for a, c, acc in [(0x3fc00000, 0x40100000, 0x3f800000),
                      (0x3f800001, 0x3f7ffffe, 0xbf800000)]:
        cases.append((0x3210 | op, a, c, acc))
for op in [0x8f, 0x9f, 0x1f, 0x5f]:
    inputs = [0, 1, 0xffffffff, 0x80000000, 0x01000001] if op in [0x8f, 0x9f] else [
        0x3fc00000, 0x40f00000, 0x4effffff, 0]
    for a in inputs:
        cases.append((0x3100 | op, a, 0, 0))
# Additional finite boundary cases, without division by zero or invalid casts.
for op in [0, 1, 2, 5, 6, 7, 8]:
    for a, c, acc in [(0xbfc00000, 0xc0100000, 0x3f800000),
                      (0x80000000, 0, 0),
                      (0, 0x80000000, 0),
                      (0x3fc00000, 0xc0100000, 0),
                      (0x3fc00000, 0x3fc00000, 0)]:
        cases.append((0x3210 | op, a, c, acc))
for a in [0xbfc00000, 0xcf000000]:
    cases.append((0x311f, a, 0, 0))
assembly = ['\n.section .text.float_probe,"ax",@progbits',
            '.global float_probe', 'float_probe:',
            '[--sp] = {r7-r1}', 'r7 = psr']
for i, (op, a, c, acc) in enumerate(cases):
    assembly += [f'r1 = {a}', f'r2 = {c}', f'r3 = {acc}', 'psr = r7',
                 f'.short 0xe53f, {op}', 'r4 = psr',
                 f'[r0+{i*8}] = r3', f'[r0+{i*8+4}] = r4']
assembly += ['psr = r7', '{r7-r1} = [sp++]', 'rts']
with (src / 'probe.S').open('a') as f:
    f.write('\n'.join(assembly) + '\n')
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
                           'extern void fm1_probe(uint32_t *out);\n'
                           'extern void float_probe(uint32_t *out);')
report = '''static void float_report(void) {
    uint32_t out[COUNT]={0},i,f=irq_save();
    uint32_t ec=FM1_EMU_CON;
    FM1_EMU_CON=ec&~(0x1fu<<16);
    float_probe(out);FM1_EMU_CON=ec;irq_restore(f);
    con_puts("FLOAT BEGIN\\r\\n");
    for(i=0;i<COUNT;i++) {
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }
    con_puts("FLOAT END\\r\\n");
}
'''.replace('COUNT', str(len(cases)*2))
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
                    'if(equal(con.line,"probe")) probe_report();\n'
                    '            else if(equal(con.line,"float")) float_report();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_986'
b.NAME = 'fm1-float-probe'
b.HARDWARE_DISPLAY = False
b.GEN.mkdir(parents=True, exist_ok=True)
(b.GEN / 'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK = Path('.deps/sdk').resolve()
ota = b.build_loader()
img, syms, dis, rt = b.build_app()
errors, notes = b.check(img, syms, dis, rt)
assert not errors, errors
package = pkg.ufw(pkg.flash_image(img, pkg.KEY), ota, b.PRODUCT)
(b.OUT / 'fm1-float-probe.fwsc').write_bytes(package)
(b.OUT / 'fm1-float-probe.symbols').write_text(syms)
(base / 'cases.json').write_text(json.dumps(cases))
print('\n'.join(notes))
print('Built watchdog/update-compatible', b.PRODUCT, len(cases), 'FP cases')
