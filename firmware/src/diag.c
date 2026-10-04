/* SPDX-License-Identifier: GPL-3.0-only
 * Minimal diagnostic firmware. Hardware/startup/update portions derived from
 * Felucca, Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments.
 * No synthesis, sequencer, samples, effects, editor, or persistent settings. */
#include <stdint.h>
#include "fm1_time.h"
#include "fm1_sys.h"
#include "fm1_irq.h"
#include "fm1_guard.h"
#include "fm1_input.h"
#include "fm1_timer.h"
#include "fm1_audio.h" /* only DMA stop; no audio initialization */
#include "fm1_lcd_hw.h"
#include "libc.c"
#include "lcd.c"
#define FELUCCA_CDC 1
#define FELUCCA_OTA 1
#define FELUCCA_OTA_DRYRUN 0
#define RING_PUBLISH() __asm__ volatile("" ::: "memory")
#include "usb.c"
#include "fm1_flash.h"
#include "ota.c"
#include "probe_hash.h"
#define BOOTGUARD_MAGIC 0x42475244u
static volatile uint32_t fm1_ms;
struct { uint32_t magic, failed, pending; } bootguard __attribute__((section(".noinit")));
#include "console.c"

extern void fm1_probe(uint32_t *out);
extern void isr_timer5(void);
static uint8_t flash_ok;
static uint32_t runs;
#ifdef FM1_HARDWARE_DISPLAY
extern void display_init(void), display_frame(void);
extern volatile uint32_t display_frames, display_ticks;
uint32_t matrix_results[11];
static void display_task(void) {
    uint32_t i, f=irq_save();
    /* The timer ISR owns the shift register. Snapshot its raw scan instead
     * of racing it with a second bit-banged scan in the display renderer. */
    for(i=0;i<11;i++) matrix_results[i]=1u|((~fm1_in.raw[i]&0x1eu)<<4);
    irq_restore(f);
    lcd_sync(); display_frame();
}
#endif
/* A tiny hex font avoids the synth's font-generation and graphics dependencies. */
static const uint8_t hexfont[16][5] = {
 {31,17,17,17,31},{4,12,4,4,14},{30,1,14,16,31},{30,1,14,1,30},
 {17,17,31,1,1},{31,16,30,1,30},{15,16,30,17,14},{31,1,2,4,4},
 {14,17,14,17,14},{14,17,15,1,30},{14,17,31,17,17},{30,17,30,17,30},
 {15,16,16,16,15},{30,17,17,17,30},{31,16,30,16,31},{31,16,30,16,16}};
static void screen_hex(uint32_t y, uint32_t v) {
    uint32_t n,row,col;
    for(n=0;n<8;n++) for(row=0;row<5;row++) for(col=0;col<5;col++)
        lcd_fill(20+n*25+col*4,y+row*4,4,4,
          hexfont[(v>>(28-4*n))&15][row]&(16>>col)?0xffff:0);
}
static void status(void) {
#ifdef FM1_HARDWARE_DISPLAY
    lcd_sync(); display_init();
#else
    lcd_fill(0,0,240,240,0);
    lcd_fill(0,0,240,12,flash_ok?0x07e0:0xffe0);
    screen_hex(32,0xD1A60001); screen_hex(72,runs);
#endif
}
static void fm1_fault(const fm1_crash_t *c) {
    fm1_audio_stop();
    lcd_fill(0,0,240,240,0xf800);
    screen_hex(40,c->vec); screen_hex(80,c->pc); screen_hex(120,c->emu);
    fm1_delay_ms(4000); fm1_reboot();
}
void fm1_timer5_irq(void) {
    static uint32_t sub,last,acc;
    uint32_t now;
    fm1_timer5_ack(); fm1_input_tick();
    if(sub%5u==0) usb_poll();
    if(++sub==10u) sub=0;
    now=fm1_ticks(); acc+=now-last; last=now;
    while(acc>=1000u*FM1_TICKS_PER_US) { acc-=1000u*FM1_TICKS_PER_US; fm1_ms++; }
}
static uint32_t ota_now_ms(void) { return fm1_ms; }
static void ota_idle(void) { fm1_wdt_feed(); }
static int ota_erase(uint32_t off) {
    uint32_t took;
    if(!FL_IN(off,0x1000u,OTA_AREA,OTA_AREA+OTA_AREA_LEN)||(off&0xfffu)) return -8;
    return fl_erase4k(off,&took);
}
static int ota_prog(uint32_t off,const void *p,uint32_t n) {
    if(!FL_IN(off,n,OTA_AREA,OTA_AREA+OTA_AREA_LEN)) return -8;
    return fl_write(off,p,n);
}
static int ota_fread(uint32_t off,void *p,uint32_t n) {
    uint8_t *d=p;
    while(n) {
        uint32_t k=n>256u?256u:n,f=irq_save();
        int rc=FL_FAR(fl_read_ram)(off,d,k);
        irq_restore(f); if(rc) return rc;
        off+=k;d+=k;n-=k;
    }
    return 0;
}
static void ota_show(uint32_t step,int32_t code) {
    lcd_fill(0,0,240,240,step>=9u?0xf800:0x001f);
    screen_hex(40,step);screen_hex(80,(uint32_t)code);
}
static void ota_commit(const uint8_t *parm) {
    bootguard.pending=0;usb_detach();fm1_delay_ms(30);fm1_enter_update(parm);
}
static void recovery(void) {
    bootguard.pending=0;usb_detach();fm1_delay_ms(30);fm1_enter_uboot();
}
static void probe_report(void) {
    uint32_t out[12]={0},i;
    /* Suppress interrupt preemption only for the short deterministic probe. */
    uint32_t f=irq_save();fm1_probe(out);irq_restore(f);
    ++runs;
    con_puts("FM1PROBE 1 " PROBE_SHA256 "\r\n");
    for(i=0;i<12;i++) {
        con_puts("W ");con_dec(i);con_putc(' ');con_hex(out[i],8);con_puts("\r\n");
    }
    con_puts("FM1PROBE END\r\n");status();
}
static int equal(const char *a,const char *b) {
    while(*a && *a==*b) {a++;b++;} return *a==*b;
}
static void cdc_task(void) {
    if(!cdc.dtr) {con.dtr_seen=0;con.len=0;return;}
    if(!con.dtr_seen) {con.dtr_seen=1;con.stalled=0;con_puts("FM-1 DIAG 1: probe info uboot\r\n");}
    while(ci_r!=ci_w) {
        char c=cdc_in[ci_r++%CI_N];
        if(c=='\r'||c=='\n') {
            con.line[con.len]=0;con.stalled=0;
            if(equal(con.line,"probe")) probe_report();
            else if(equal(con.line,"info")) {
                con_puts("FM-1 DIAG 1 " FELUCCA_ID "\r\nprobe_sha256 " PROBE_SHA256 "\r\nflash ");
                con_dec(flash_ok);con_puts("\r\n");
#ifdef FM1_HARDWARE_DISPLAY
                con_puts("display_frames ");con_dec(display_frames);
                con_puts("\r\ndisplay_ticks ");con_hex(display_ticks,8);
                con_puts("\r\n");
#endif
            } else if(equal(con.line,"uboot")) recovery();
            else if(con.len) con_puts("ERROR: use probe, info, or uboot\r\n");
            con.len=0;
        } else if(c==8||c==127) {if(con.len)con.len--;}
        else if(c>=32 && con.len<CON_LINE-1) con.line[con.len++]=c;
    }
}
#ifdef FM1_HARDWARE_DISPLAY
static void input_debug(void) {
    static uint32_t previous_buttons, previous_notes;
    uint32_t f=irq_save(), buttons=fm1_in.buttons, notes=fm1_in.notes;
    irq_restore(f);
    if(cdc.dtr) {
        uint32_t id;
        for(id=0;id<41;id++) {
            uint32_t current=id<14?buttons:notes;
            uint32_t previous=id<14?previous_buttons:previous_notes;
            uint32_t bit=1u<<(id<14?id:id-14);
            if((current^previous)&bit) {
                con_puts("KEY ");con_dec(id);
                con_puts(current&bit?" down\r\n":" up\r\n");
            }
        }
    }
    previous_buttons=buttons;previous_notes=notes;
}
#endif
static void fm1_main(void) {
    uint32_t f,held=0;
#ifdef FM1_HARDWARE_DISPLAY
    uint32_t last_frame=0;
#endif
    fm1_audio_stop();
    f=irq_save();flash_ok=FL_FAR(fl_jedec_ram)()==0x856014u;irq_restore(f);
    if(flash_ok) {fl_plain_window_init();ota_boot_cleanup();}
    lcd_init();status();fm1_input_init();usb_start();
    fm1_timer5_start(isr_timer5,1);fm1_guard_lock_top();fm1_irq_enable_all();
    for(;;) {
        fm1_wdt_feed();usb_retry(fm1_ms);mi_r=mi_w;
        if(fm1_ms>30000u) {bootguard.pending=0;bootguard.failed=0;}
        if((fm1_in.buttons&3u)!=3u) held=fm1_ms;
        else if(fm1_ms-held>5000u) recovery();
        ota_service();
        if(usb.ota_req) {usb.ota_req=0;if(flash_ok)ota_session();status();}
        if(usb.uboot_req) recovery();
        cdc_task();
#ifdef FM1_HARDWARE_DISPLAY
        input_debug();
        if((uint32_t)(fm1_ms-last_frame)>=100u) {
            last_frame=fm1_ms;display_task();
        }
#endif
    }
}
#include "startup.c"
