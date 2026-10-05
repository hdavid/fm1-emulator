# SPDX-License-Identifier: GPL-3.0-only
"""Measure stock BT indirect table accesses without enabling transmission."""
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/bt-indirect').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
assembly = '''
.section .text.memory_arithmetic_probe,"ax",@progbits
.global memory_arithmetic_probe
memory_arithmetic_probe:
    [--sp] = {r3-r1}
    r3 = psr
    r1 = 4
    [r0] = r1
    r2 = 4
    [r0] -= r2
    r1 = psr
    [r0+4] = r1
    [r0+8] = r2
    r1 = 0xffffffff
    [r0+12] = r1
    [r0+12] += r2
    r1 = psr
    [r0+16] = r1
    psr = r3
    {r3-r1} = [sp++]
    rts
'''
with (src / 'probe.S').open('a') as f:
    f.write(assembly)
p = src / 'src/diag.c'
code = p.read_text().replace('extern void fm1_probe(uint32_t *out);',
    'extern void fm1_probe(uint32_t *out);\nextern void memory_arithmetic_probe(uint32_t *out);')
report = '''static void bt_table_report(void) {
    volatile uint32_t *bt=(volatile uint32_t *)0x28000;
    volatile uint32_t *wl=(volatile uint32_t *)0x14000;
    volatile uint32_t *clk=(volatile uint32_t *)0x10010;
    volatile uint32_t *a=(volatile uint32_t *)0x11900;
    static const uint32_t analog[18]={0xeaa20005u,0x01c3c880u,0x9c010010u,0x673b9379u,
      0x087fd98eu,0x447c1b19u,0x00076a27u,0x2915c066u,0x0067fdc0u,0x51b801f7u,
      0x0afbe9e7u,0x0052a071u,0x05d9625fu,0x01bfffffu,0x8db7d065u,0x12640f32u,
      0x08b41411u,0x00000a80u};
    uint32_t saved[18],out[199]={0},i,n,cmd,dat,con,stage,f=irq_save(),wc=*wl,cc=*clk;
    for(i=0;i<18;i++)saved[i]=a[i];
    /* RF.c's clock gates and crystal reference; initialize the disabled packet engine.
     * Only the register interface is enabled; no analog transmitter, DMA or
     * advertising/connection request is configured. */
    *clk=cc&~0xc000u;*wl=0xc0081u;
    for(n=0;n<2000;n++) __asm__ volatile("csync");
    con=bt[0];bt[0]=1u;
    for(n=0;n<2000;n++) __asm__ volatile("csync");
    cmd=bt[7];dat=bt[8];
    memory_arithmetic_probe(out);
    for(stage=0;stage<2;stage++) {
      uint32_t *row=out+5+stage*97;
      if(stage) {
        for(i=0;i<18;i++)a[i]=analog[i];
        for(n=0;n<20000;n++) __asm__ volatile("csync");
      }
      row[0]=*wl;row[1]=*clk;row[2]=bt[0];row[3]=bt[7];row[4]=bt[8];row[5]=bt[9];
      for(i=0;i<8;i++) {
        uint32_t table=i<6 ? i>>1 : 16u,key=(i&1u)<<4 | table<<10,old;
        bt[7]=key|2u;for(n=0;n<2000;n++) __asm__ volatile("csync");
        old=bt[9];row[6+i*11]=old;row[7+i*11]=bt[7];row[8+i*11]=bt[8];
        bt[8]=0x12345678u+i;bt[7]=key|5u;
        for(n=0;n<2000;n++) __asm__ volatile("csync");
        row[9+i*11]=bt[7];row[10+i*11]=bt[8];row[11+i*11]=bt[9];
        bt[7]=key|2u;for(n=0;n<2000;n++) __asm__ volatile("csync");
        row[12+i*11]=bt[7];row[13+i*11]=bt[8];row[14+i*11]=bt[9];
        bt[8]=old;bt[7]=key|5u;for(n=0;n<2000;n++) __asm__ volatile("csync");
        bt[7]=key|2u;for(n=0;n<2000;n++) __asm__ volatile("csync");
        row[15+i*11]=bt[7];row[16+i*11]=bt[9];
    }
      }
    bt[8]=dat;bt[7]=cmd;bt[0]=con;*wl=wc;*clk=cc;
    for(i=0;i<18;i++)a[i]=saved[i];
    irq_restore(f);
    con_puts("BT TABLE BEGIN\\r\\n");
    for(i=0;i<199;i++) {con_puts("W ");con_dec(i);con_putc(' ');con_hex(out[i],8);con_puts("\\r\\n");}
    con_puts("BT TABLE END\\r\\n");
}
'''
code = code.replace('static int equal(', report + 'static int equal(')
code = code.replace('if(equal(con.line,"probe")) probe_report();',
    'if(equal(con.line,"probe")) probe_report();\n            else if(equal(con.line,"bttable")) bt_table_report();')
p.write_text(code)
b.FW = src
b.OUT = base / 'build'
b.GEN = b.OUT / 'gen'
b.LDR = b.OUT / 'loader'
b.PRODUCT = 'FM-1_993'
b.NAME = 'fm1-bt-indirect'
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
print('Built watchdog/update-compatible', b.PRODUCT, 'BT indirect table probe')
