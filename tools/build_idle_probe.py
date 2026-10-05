# SPDX-License-Identifier: GPL-3.0-only
"""Measure IDLE wake/return using TIMER3, retaining watchdog and USB recovery."""
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/idle').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
assembly = '''
.section .text.idle_probe,"ax",@progbits
.global idle_probe, idle_resume
idle_probe:
    [--sp] = {r7-r1}
    r1 = 0x10700
    r2 = 0x4000
    [r1] = r2
    r2 = 0
    [r1+4] = r2
    r2 = 6000
    [r1+8] = r2
    r3 = 0x10804
    r4 = [r3]
    r2 = 0x4019
    [r1] = r2
    sti
    idle
idle_resume:
    csync
    csync
    csync
    csync
    csync
    csync
    csync
    csync
    cli
    r5 = [r3]
    [r0] = r4
    [r0+4] = r5
    r2 = 0x4000
    [r1] = r2
    {r7-r1} = [sp++]
    rts
.section .text.idle_isr,"ax",@progbits
.global idle_isr
idle_isr:
    [--sp] = {psr, rets, reti}
    [--sp] = {r3-r0}
    r0 = idle_observed
    r1 = [r0]
    r1 += 1
    [r0] = r1
    r1 = reti
    [r0+4] = r1
    r1 = 0x10700
    r2 = 0x4000
    [r1] = r2
    {r3-r0} = [sp++]
    {psr, rets, reti} = [sp++]
    csync
    rti
'''
with (src / 'probe.S').open('a') as f:
    f.write(assembly)
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
    'extern void fm1_probe(uint32_t *out);\n'
    'extern void idle_probe(uint32_t *out),idle_isr(void),idle_resume(void);\n'
    'volatile uint32_t idle_observed[2];')
report = '''static void idle_report(void) {
    uint32_t out[5]={0},cfg[16],i,f=irq_save();
    /* Isolate the 1 ms TIMER3 wake; restore USB/tick IRQs immediately. */
    for(i=0;i<16;i++) {
        cfg[i]=*(volatile uint32_t *)(0x1eef100u+i*4u);
        *(volatile uint32_t *)(0x1eef100u+i*4u)=0;
    }
    *(volatile uint32_t *)0x1eef100u=cfg[0]&0xf00000f0u;
    idle_observed[0]=0;idle_observed[1]=0;
    idle_probe(out);
    out[2]=idle_observed[0];out[3]=idle_observed[1];
    out[4]=(uint32_t)(uintptr_t)idle_resume;
    for(i=0;i<16;i++) *(volatile uint32_t *)(0x1eef100u+i*4u)=cfg[i];
    irq_restore(f);
    con_puts("IDLE BEGIN\\r\\n");
    for(i=0;i<5;i++) {
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }
    con_puts("IDLE END\\r\\n");
}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
    'if(equal(con.line,"probe")) probe_report();\n'
    '            else if(equal(con.line,"idle")) idle_report();')
# WL82 TIMER3 is IRQ 7; TIMER4/5 and USB are left running for recovery.
code = code.replace('fm1_guard_lock_top();',
    'fm1_irq_attach(7,idle_isr,2);fm1_guard_lock_top();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_996'
b.NAME = 'fm1-idle'
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
print('Built watchdog/update-compatible', b.PRODUCT, 'IDLE interrupt probe')
