/* SPDX-License-Identifier: GPL-3.0-only
 * Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments */
/* Serial console on the CDC-ACM function (FELUCCA_CDC=1): read-only
 * diagnostics. Runs in the main loop (cdc_task); usb_poll moves the bytes.
 * The baud rate is ignored. Nothing here writes memory or flash; `uboot`
 * does what the SysEx soft key does. */
#define CON_LINE 64u

static struct {
    char line[CON_LINE];
    uint32_t len;
    uint8_t dtr_seen, stalled;   /* stalled: the host stopped reading, drop the rest of this reply */
} con;

static void con_putc(char c)
{
    uint32_t t0 = fm1_ms;
    while (co_w - co_r >= CO_N) {                      /* full: wait for usb_poll, briefly */
        if (!cdc.dtr || con.stalled || fm1_ms - t0 > 20u) {
            con.stalled = 1;
            return;
        }
        fm1_wdt_feed();
    }
    cdc_out[co_w % CO_N] = (uint8_t)c;
    RING_PUBLISH();
    co_w++;
}

static void con_puts(const char *s)
{
    while (*s)
        con_putc(*s++);
}

static void con_hex(uint32_t v, uint32_t digits)
{
    while (digits--)
        con_putc("0123456789ABCDEF"[(v >> (digits * 4u)) & 15u]);
}

static void con_dec(int32_t v)
{
    char b[12];
    uint32_t n = 0, u = v < 0 ? (uint32_t)-v : (uint32_t)v;
    if (v < 0)
        con_putc('-');
    do
        b[n++] = (char)('0' + u % 10u);
    while ((u /= 10u) != 0 && n < sizeof b);
    while (n)
        con_putc(b[--n]);
}

