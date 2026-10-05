# SPDX-License-Identifier: GPL-3.0-only
"""Build a watchdog/update-compatible probe for the stock PMU temperature ADC."""
import shutil
from pathlib import Path

import build_diag as b
import fm1pkg_make as pkg

base = Path(".deps/firmware-trial/temperature").resolve()
src = base / "firmware"
shutil.copytree("firmware", src, dirs_exist_ok=True)
p = src / "src/diag.c"
code = p.read_text().replace('#include "fm1_audio.h"', '#include "fm1_adc.h"\n#include "fm1_audio.h"')
report = r'''static void temperature_report(void) {
    uint32_t i, f, previous;
    int32_t samples[20];
    con_puts("TEMP BEGIN\r\n");
    f=irq_save();
    previous=fm1_p33_read(4);
    fm1_p33_write(4,(previous&~0x0fu)|(3u<<1)|1u);
    for(i=0;i<20;i++) samples[i]=fm1_adc_read(15);
    fm1_p33_write(4,(uint8_t)previous);
    irq_restore(f);
    fm1_wdt_feed();
    for(i=0;i<20;i++) {
        con_puts("ADC 3 ");con_dec(samples[i]);con_puts("\r\n");
    }
    con_puts("TEMP END\r\n");
}
'''
code = code.replace("static int equal(", report + "static int equal(")
code = code.replace('if(equal(con.line,"probe")) probe_report();',
                    'if(equal(con.line,"probe")) probe_report();\n'
                    '            else if(equal(con.line,"temp")) temperature_report();')
p.write_text(code)
b.FW = src
b.OUT = base / "build"
b.GEN = b.OUT / "gen"
b.LDR = b.OUT / "loader"
b.PRODUCT = "FM-1_997"
b.NAME = "fm1-temperature-probe"
b.HARDWARE_DISPLAY = False
b.GEN.mkdir(parents=True, exist_ok=True)
(b.GEN / "probe_hash.h").write_text(Path("build/gen/probe_hash.h").read_text())
pkg.SDK = Path(".deps/sdk").resolve()
ota = b.build_loader()
img, syms, dis, rt = b.build_app()
errors, notes = b.check(img, syms, dis, rt)
assert not errors, errors
(b.OUT / "fm1-temperature-probe.fwsc").write_bytes(
    pkg.ufw(pkg.flash_image(img, pkg.KEY), ota, b.PRODUCT))
(b.OUT / "fm1-temperature-probe.symbols").write_text(syms)
print("\n".join(notes))
print("Built watchdog/update-compatible", b.PRODUCT)
