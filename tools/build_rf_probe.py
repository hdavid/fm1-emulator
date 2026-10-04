# SPDX-License-Identifier: GPL-3.0-only
"""Build an update-compatible, non-transmitting RF PLL comparator probe."""
from pathlib import Path
import shutil
import build_diag as b
import fm1pkg_make as pkg

base = Path('.deps/firmware-trial/rf-probe').resolve()
src = base / 'firmware'
shutil.copytree('firmware', src, dirs_exist_ok=True)
# Configuration observed before stock wfa_pll_bank_scan, excluding the
# system PLL and audio words. No MAC transmitter is enabled by this probe.
initial = [0xea220005, 0x01c35880, 0x9c010010, 0x672b9349,
           0x0508098e, 0x4078c319, 0x00076a24, 0x2915c066,
           0x00a641c0, 0x65b80057, 0x1afae7e7, 0x0052a070,
           0x05d9625f, 0x01000000, 0x819fd033, 0x3a780ef2,
           0x08b41490, 0x00000a80]
report = '''static void rf_wait(void) {
    uint32_t n;for(n=0;n<20000;n++) __asm__ volatile("nop");
    fm1_wdt_feed();
}
static uint32_t rf_measure(uint32_t cap,uint32_t feedback) {
    volatile uint32_t *a=(volatile uint32_t *)0x11900;
    uint32_t n;
    a[14]=(a[14]&~0x03f80000u)|(cap<<19);
    a[15]=(a[15]&~0x1fe0u)|(feedback<<5);
    a[14]&=~0x50000u;a[13]&=~0x1000000u;rf_wait();
    a[14]|=0x10000u;rf_wait();
    a[13]|=0x1000000u;a[14]|=0x40000u;rf_wait();
    a[26]=(a[26]&0x0fffffffu)|0x10000000u;
    for(n=0;n<8;n++) a[30]=1;
    a[30]=0;return a[30];
}
static void rf_report(void) {
    static const uint32_t config[18]={CONFIG};
    uint32_t saved[31],out[256],i,f=irq_save();
    volatile uint32_t *a=(volatile uint32_t *)0x11900;
    for(i=0;i<31;i++) saved[i]=a[i];
    for(i=0;i<18;i++) a[i]=config[i];
    a[16]|=0x1000000u;
    for(i=0;i<128;i++) {
        uint32_t lo=0,hi=256;
        while(lo<hi) {
            uint32_t mid=(lo+hi)/2,vc=rf_measure(i,mid);
            if(vc&0x20000u) lo=mid+1;else hi=mid;
        }
        out[i*2]=lo;lo=0;hi=256;
        while(lo<hi) {
            uint32_t mid=(lo+hi)/2,vc=rf_measure(i,mid);
            if(vc&0x40000u) hi=mid;else lo=mid+1;
        }
        out[i*2+1]=lo;
    }
    for(i=0;i<31;i++) a[i]=saved[i];
    irq_restore(f);
    con_puts("RF BEGIN\\r\\n");
    for(i=0;i<256;i++) {
        con_puts("W ");con_dec(i);con_putc(' ');
        con_hex(out[i],8);con_puts("\\r\\n");
    }
    con_puts("RF END\\r\\n");
}
'''.replace('CONFIG', ','.join(hex(v)+'u' for v in initial))
p=src/'src/diag.c'
code=p.read_text().replace('static int equal(',report+'static int equal(')
code=code.replace('if(equal(con.line,"probe")) probe_report();',
                  'if(equal(con.line,"probe")) probe_report();\n'
                  '            else if(equal(con.line,"rf")) rf_report();')
p.write_text(code)
b.FW=src;b.OUT=base/'build';b.GEN=b.OUT/'gen';b.LDR=b.OUT/'loader'
b.PRODUCT='FM-1_985';b.NAME='fm1-rf-probe';b.HARDWARE_DISPLAY=False
b.GEN.mkdir(parents=True,exist_ok=True)
(b.GEN/'probe_hash.h').write_text(Path('build/gen/probe_hash.h').read_text())
pkg.SDK=Path('.deps/sdk').resolve()
ota=b.build_loader();img,syms,dis,rt=b.build_app()
errors,notes=b.check(img,syms,dis,rt)
assert not errors,errors
(b.OUT/'fm1-rf-probe.fwsc').write_bytes(pkg.ufw(pkg.flash_image(img,pkg.KEY),ota,b.PRODUCT))
print('\n'.join(notes))
print('Built watchdog/update-compatible',b.PRODUCT)
