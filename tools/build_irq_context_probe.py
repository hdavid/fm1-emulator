# SPDX-License-Identifier: GPL-3.0-only
"""Measure IRQ context/priority while retaining watchdog and USB recovery."""
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/irq-context').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
assembly = '''
.section .text.irq_context_probe,"ax",@progbits
.global irq_context_probe
irq_context_probe:
    [--sp] = {r3-r1}
    r2 = 0x1eef1a0
    r3 = [r0]
    sti
    [r2] = r3
    rep 32 {
        csync
    }
    cli
    r1 = icfg
    [r0+4] = r1
    r2 = 0x1eef1a8
    r1 = [r2]
    [r0+8] = r1
    {r3-r1} = [sp++]
    rts
'''
for number in range(2):
    assembly += f'''
.section .text.irq_context_isr{number},"ax",@progbits
.global irq_context_isr{number}
irq_context_isr{number}:
    [--sp] = {{psr, rets, reti}}
    [--sp] = {{r3-r0}}
    r0 = irq_context_observed
    r1 = [r0]
    if (r1 >= 2) goto irq_context_ack{number}
    r2 = r1 << 4
    r2 += r0
    r3 = {number}
    [r2+4] = r3
    r3 = icfg
    [r2+8] = r3
    r3 = 0x1eef1a8
    r3 = [r3]
    [r2+12] = r3
    r3 = reti
    [r2+16] = r3
    r1 += 1
    [r0] = r1
irq_context_ack{number}:
    r2 = 0x1eef1a4
    r3 = {1 << number}
    [r2] = r3
    {{r3-r0}} = [sp++]
    {{psr, rets, reti}} = [sp++]
    csync
    rti
'''
with (src / 'probe.S').open('a') as f:
    f.write(assembly)
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
    'extern void fm1_probe(uint32_t *out);\n'
    'extern void irq_context_probe(uint32_t *out),irq_context_isr0(void),irq_context_isr1(void);\n'
    'volatile uint32_t irq_context_observed[9];')
report = '''static void irq_context_report(void) {
    uint32_t out[120],i,j,f=irq_save();
    uint32_t *cfg=(uint32_t *)0x1eef13c,old=*cfg;
    for(i=0;i<10;i++) {
        uint32_t *row=out+i*12;
        for(j=0;j<9;j++)irq_context_observed[j]=0;
        *cfg=(old&~255u)|(i<8 ? (1u|(i<<1)) : (i==8 ? 0xb5u : 0x55u));
        row[0]=i<8 ? 1 : 3;
        irq_context_probe(row);
        for(j=0;j<9;j++)row[j+3]=irq_context_observed[j];
    }
    *cfg=old;
    irq_restore(f);
    con_puts("IRQ CONTEXT BEGIN\\r\\n");
    for(i=0;i<120;i++) {
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }
    con_puts("IRQ CONTEXT END\\r\\n");
}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
    'if(equal(con.line,"probe")) probe_report();\n'
    '            else if(equal(con.line,"irqctx")) irq_context_report();')
code = code.replace('fm1_guard_lock_top();',
    'fm1_irq_attach(FM1_IRQ_SOFT0,irq_context_isr0,2);'
    'fm1_irq_attach(FM1_IRQ_SOFT0+1,irq_context_isr1,5);fm1_guard_lock_top();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_989'
b.NAME = 'fm1-irq-context'
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
print('Built watchdog/update-compatible', b.PRODUCT, 'IRQ context probe')
