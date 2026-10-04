# SPDX-License-Identifier: GPL-3.0-only
"""Build an FM-1-compatible TESTSET probe without replacing the display build.

The original instruction probe, watchdog service and USB updater are retained.
Send `testset` over USB CDC to read PSR, the resulting byte, and the IFEQ
decision for each of the twelve seeds below. Outputs stay under .deps/.
Run from the repository root with the mise/uv-managed Python environment.
"""
import sys
import shutil
import hashlib
from pathlib import Path
sys.path.insert(0, 'tools')
import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/atomic').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
assembly = ['\n.section .text.testset_probe,"ax",@progbits', '.global testset_probe',
            'testset_probe:', '[--sp] = {r7-r1}', 'r7 = psr', 'r1 = r0 + 256']
for i, seed in enumerate([0, 1, 0x80, 0xff, 2, 4, 8, 15, 0x7f, 0xfe, 0xf0, 0x10]):
    off = i * 12
    assembly += [f'r2 = {seed}', 'b[r1] = r2', 'testset b[r1]',
                 f'ifeq goto .Lbusy{i}', 'r4 = 1', f'goto .Lresult{i}',
                 f'.Lbusy{i}:', 'r4 = 0', f'.Lresult{i}:', 'r3 = psr',
                 f'[r0 + {off}] = r3', 'r3 = b[r1] (u)',
                 f'[r0 + {off+4}] = r3', f'[r0 + {off+8}] = r4']
assembly += ['psr = r7', '{r7-r1} = [sp++]', 'rts']
with (src/'probe.S').open('a') as f: f.write('\n'.join(assembly)+'\n')
p=src/'src/diag.c'
code=p.read_text().replace('extern void fm1_probe(uint32_t *out);',
    'extern void fm1_probe(uint32_t *out);\nextern void testset_probe(uint32_t *out);')
report='''static void testset_report(void) {
    uint32_t out[96]={0},i;
    uint32_t f=irq_save();testset_probe(out);irq_restore(f);
    con_puts("TESTSET BEGIN\\r\\n");
    for(i=0;i<36;i++) { con_puts("W ");con_dec(i);con_putc(' ');con_hex(out[i],8);con_puts("\\r\\n"); }
    con_puts("TESTSET END\\r\\n");
}
'''
registers = [0x10000, 0x10008, 0x1000c, 0x10010, 0x10014, 0x10018,
             0x119a0, 0x119a4, 0x119a8, 0x119ac, 0x13e00, 0x13e04,
             0x40200, 0x40204, 0x40208, 0x4020c, 0x40300, 0x40304,
             0x40308, 0x4030c, 0x40310, 0x40314, 0x16a00, 0x16a04, 0x16a08,
             0x10200]
report += f'static uint32_t boot_registers[{len(registers)}];\n'
report += '''static void clocks_report(void) {
    uint32_t i;
    con_puts("CLOCKS BEGIN\\r\\n");
    for(i=0;i<sizeof(boot_registers)/sizeof(boot_registers[0]);i++) { con_puts("W ");con_dec(i);con_putc(' ');con_hex(boot_registers[i],8);con_puts("\\r\\n"); }
    con_puts("CLOCKS END\\r\\n");
}
'''
report += '''static void irq_report(void) {
    uint32_t out[272],i,j,bank,bit,f=irq_save();
    volatile uint32_t *set0=(volatile uint32_t *)0x1eef1a0;
    volatile uint32_t *set1=(volatile uint32_t *)0x1eef3a0;
    uint32_t old0=*(volatile uint32_t *)0x1eef18c>>24;
    uint32_t old1=*(volatile uint32_t *)0x1eef38c>>24;
    uint32_t cfg0=*(volatile uint32_t *)0x1eef13c;
    uint32_t cfg1=*(volatile uint32_t *)0x1eef33c;
    *(volatile uint32_t *)0x1eef13c=0xffffffff;
    *(volatile uint32_t *)0x1eef33c=0xffffffff;
    for(bank=0;bank<2;bank++) for(bit=0;bit<8;bit++) {
        set0[1]=255;set1[1]=255;
        *(bank?set1:set0)=1u<<bit;
        __asm__ volatile("csync");
        i=(bank*8+bit)*17;
        for(j=0;j<8;j++) {
            out[i+j]=((volatile uint32_t *)0x1eef180)[j];
            out[i+8+j]=((volatile uint32_t *)0x1eef380)[j];
        }
        out[i+16]=*(volatile uint32_t *)0x1ee0000;
    }
    set0[1]=255;set1[1]=255;*set0=old0;*set1=old1;
    *(volatile uint32_t *)0x1eef13c=cfg0;
    *(volatile uint32_t *)0x1eef33c=cfg1;
    irq_restore(f);
    con_puts("IRQ BEGIN\\r\\n");
    for(i=0;i<272;i++) { con_puts("W ");con_dec(i);con_putc(' ');con_hex(out[i],8);con_puts("\\r\\n"); }
    con_puts("IRQ END\\r\\n");
}
'''
code=code.replace('static int equal(',report+'static int equal(')
code=code.replace('if(equal(con.line,"probe")) probe_report();',
    'if(equal(con.line,"probe")) probe_report();\n            else if(equal(con.line,"testset")) testset_report();\n            else if(equal(con.line,"clocks")) clocks_report();\n            else if(equal(con.line,"irq")) irq_report();')
code = code.replace('    fm1_audio_stop();\n    f=irq_save();',
    ''.join(f'    boot_registers[{i}]=*(volatile uint32_t *)0x{address:x}u;\n'
            for i, address in enumerate(registers)) + '    fm1_audio_stop();\n    f=irq_save();')
p.write_text(code)
b.FW=src; b.OUT=base/'build'; b.GEN=b.OUT/'gen'; b.LDR=b.OUT/'loader'
b.PRODUCT='FM-1_982'; b.NAME='fm1-atomic'; b.HARDWARE_DISPLAY=False
b.GEN.mkdir(parents=True,exist_ok=True)
(b.GEN/'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK=Path('.deps/sdk').resolve()
ota=b.build_loader()
img,syms,dis,rt=b.build_app()
errors,notes=b.check(img,syms,dis,rt)
assert not errors, errors
package=pkg.ufw(pkg.flash_image(img,pkg.KEY),ota,b.PRODUCT)
(b.OUT/'fm1-atomic.fwsc').write_bytes(package)
(b.OUT/'fm1-atomic.symbols').write_text(syms)
print('\n'.join(notes))
print('Built watchdog/update-compatible',b.PRODUCT,'app',len(img),'bytes, package SHA256',hashlib.sha256(package).hexdigest())
