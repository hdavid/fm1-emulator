# SPDX-License-Identifier: GPL-3.0-only
"""Measure interrupt delivery inside REP without removing update/recovery."""
import shutil
import sys
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

predicate = '--predicate' in sys.argv
name = 'predicate-irq' if predicate else 'repeat-irq'
base = Path('.deps/firmware-trial/' + name).resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
assembly = '''
.section .text.repeat_irq_probe,"ax",@progbits
.global repeat_irq_probe
repeat_irq_probe:
    [--sp] = {r7-r1}
    r1 = 0
    r2 = 0x1eef1a0
    r3 = 1
    sti
    rep 32 {
        r1 += 1
        [r2] = r3
        csync
        csync
    }
    csync
    csync
    cli
    [r0] = r1
    {r7-r1} = [sp++]
    rts
.section .text.repeat_irq_isr,"ax",@progbits
.global repeat_irq_isr
repeat_irq_isr:
    [--sp] = {psr, rets, reti}
    [--sp] = {r3-r0}
    r0 = repeat_irq_observed
    r2 = [r0+4]
    if (r2 != 0) goto repeat_irq_count
    [r0] = r1
    r3 = reti
    [r0+8] = r3
    r3 = icfg
    [r0+12] = r3
repeat_irq_count:
    r2 += 1
    [r0+4] = r2
    r2 = 0x1eef1a4
    r3 = 1
    [r2] = r3
    {r3-r0} = [sp++]
    {psr, rets, reti} = [sp++]
    csync
    rti
'''
if predicate:
    assembly = assembly.replace('''    rep 32 {
        r1 += 1
        [r2] = r3
        csync
        csync
    }''', '''    if (r3 != 0) {
        [r2] = r3
        r1 = 2
        r1 = 3
    } else {
        r1 = 254
    }''')
with (src / 'probe.S').open('a') as f:
    f.write(assembly)
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
    'extern void fm1_probe(uint32_t *out);\n'
    'extern void repeat_irq_probe(uint32_t *out),repeat_irq_isr(void);\n'
    'volatile uint32_t repeat_irq_observed[4];')
report = '''static void repeat_irq_report(void) {
    uint32_t out[6]={0},i,f=irq_save();
    for(i=0;i<4;i++)repeat_irq_observed[i]=0;
    repeat_irq_probe(out);
    for(i=0;i<4;i++)out[i+1]=repeat_irq_observed[i];
    out[5]=fm1_icfg();
    irq_restore(f);
    con_puts("REPEAT IRQ BEGIN\\r\\n");
    for(i=0;i<6;i++) {
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }
    con_puts("REPEAT IRQ END\\r\\n");
}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
    'if(equal(con.line,"probe")) probe_report();\n'
    '            else if(equal(con.line,"repirq")) repeat_irq_report();')
code = code.replace('fm1_guard_lock_top();',
    'fm1_irq_attach(FM1_IRQ_SOFT0,repeat_irq_isr,2);fm1_guard_lock_top();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_988' if predicate else 'FM-1_987'
b.NAME = 'fm1-' + name
b.HARDWARE_DISPLAY = False
b.GEN.mkdir(parents=True, exist_ok=True)
(b.GEN / 'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK = Path('.deps/sdk').resolve()
ota = b.build_loader()
img, syms, dis, rt = b.build_app()
errors, notes = b.check(img, syms, dis, rt)
assert not errors, errors
(b.OUT / (b.NAME + '.fwsc')).write_bytes(pkg.ufw(pkg.flash_image(img, pkg.KEY), ota, b.PRODUCT))
(b.OUT / (b.NAME + '.symbols')).write_text(syms)
(b.OUT / (b.NAME + '.dis')).write_text(dis)
print('\n'.join(notes))
print('Built watchdog/update-compatible', b.PRODUCT, 'REP interrupt probe')
