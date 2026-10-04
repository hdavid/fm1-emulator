/* SPDX-License-Identifier: GPL-3.0-only */
/* Emulator-only display extension to foundation.S. Every visible pixel is
 * drawn by guest instructions through the same LCD SPI/DMA interface as
 * Felucca. No host rendering hooks, updater, or hardware recovery. */
    .section .text,"ax",@progbits
    .globl display_main
display_main:
    r0 = 0x51020
    r1 = 0x10
    [r0 + 0] = r1
    r0 = 0x50080
    r1 = 0
    [r0 + 0] = r1
    [r0 + 8] = r1
    r0 = 0x11D00
    r1 = 0x4021
    [r0 + 0] = r1
    r1 = 4
    [r0 + 4] = r1
    r0 = 1
    call lcd_command
    r0 = 0x11
    call lcd_command
    r0 = 0x3A
    call lcd_command
    r0 = 0x55
    call lcd_byte
    r0 = 0x36
    call lcd_command
    r0 = 0
    call lcd_byte
    r0 = 0x21
    call lcd_command
    r0 = 0x13
    call lcd_command
    r0 = 0
    r1 = 0
    r2 = 240
    r3 = 240
    r4 = 0x82108210
    call lcd_rect
    r0 = title
    r1 = 24
    r2 = 14
    r3 = 3
    call lcd_text
    r0 = time_label
    r1 = 24
    r2 = 50
    r3 = 4
    call lcd_text
    r0 = keys_label
    r1 = 24
    r2 = 112
    r3 = 4
    call lcd_text
    r0 = 0x29
    call lcd_command
.Lframe:
    r0 = 24
    r1 = 80
    r2 = 144
    r3 = 30
    r4 = 0x82108210
    call lcd_rect
    r0 = 0x10800
    r0 = [r0 + 4]
    r1 = display_ticks
    [r1 + 0] = r0
    r1 = 24
    r2 = 80
    call lcd_hex

    /* Rescan all columns. PA2 stays low: active-low display backlight. */
    r0 = 0x50000
    r5 = matrix_results
    r6 = 1
    r7 = 11
.Lcolumn:
    r4 = 0xFFFF
    r4 ^= r6
    r1 = 16
    r2 = 8
.Lshift:
    r3 = r4 >> 15
    r3 = r3 << 4
    [r0 + 0] = r3
    r3 |= r2
    [r0 + 0] = r3
    r3 ^= r2
    [r0 + 0] = r3
    r4 = r4 << 1
    r1 += -1
    if (r1 != 0) goto .Lshift
    r1 = 2
    [r0 + 0] = r1
    r1 = 0
    [r0 + 0] = r1
    r1 = [r0 + 4]
    [r5 + 0] = r1
    r5 += 4
    r6 = r6 << 1
    r7 += -1
    if (r7 != 0) goto .Lcolumn

    r5 = matrix_results
    r6 = 24
    r7 = 11
.Ltiles:
    .set row, 0
    .rept 4
    r0 = [r5 + 0]
    r0 = r0 >> (5 + row)
    r1 = 1
    r0 &= r1
    r4 = 0x86318631
    if (r0 != 0) goto 1f
    r4 = 0x40F640F6
1:
    r0 = r6
    r1 = 148 + row * 18
    r2 = 12
    r3 = 12
    call lcd_rect
    .set row, row + 1
    .endr
    r5 += 4
    r6 += 18
    r7 += -1
    if (r7 != 0) goto .Ltiles
    r0 = display_frames
    r1 = [r0 + 0]
    r1 += 1
    [r0 + 0] = r1
    .globl display_frame_done
display_frame_done:
    goto .Lframe

/* Helpers preserve all eight working registers and the guest return address. */
.macro save
    [--sp] = {rets, r7-r4}
    [--sp] = {r3-r0}
.endm
.macro restore
    {r3-r0} = [sp++]
    {pc, r7-r4} = [sp++]
.endm
lcd_command:
    save
    r1 = 0x50080
    r2 = 0
    [r1 + 0] = r2
    goto .Lspi_byte
lcd_byte:
    save
    r1 = 0x50080
    r2 = 0x100
    [r1 + 0] = r2
.Lspi_byte:
    r1 = 0x11D00
    r2 = 0x4021
    [r1 + 0] = r2
    [r1 + 8] = r0
    call lcd_wait
    restore
lcd_wait:
    save
    r1 = 0x11D00
    r3 = 0x8000
.Lpending:
    r2 = [r1 + 0]
    r2 &= r3
    if (r2 == 0) goto .Lpending
    r2 = 0x4021
    [r1 + 0] = r2
    restore
/* r0 = SRAM source, r1 = byte count */
lcd_dma:
    save
    r2 = 0x50080
    r3 = 0x100
    [r2 + 0] = r3
    r2 = 0x11D00
    r3 = 0x4021
    [r2 + 0] = r3
    [r2 + 12] = r0
    [r2 + 16] = r1
    call lcd_wait
    restore
/* r0=x, r1=y, r2=even width, r3=height, r4=two byte-swapped RGB565 pixels */
lcd_rect:
    save
    r5 = lcd_window
    r6 = r0 + r2
    r6 += -1
    r6 = r6 << 24
    r0 = r0 << 8
    r0 |= r6
    [r5 + 0] = r0
    r6 = r1 + r3
    r6 += -1
    r6 = r6 << 24
    r1 = r1 << 8
    r1 |= r6
    [r5 + 4] = r1
    r0 = 0x2A
    call lcd_command
    r0 = r5
    r1 = 4
    call lcd_dma
    r0 = 0x2B
    call lcd_command
    r0 = r5
    r0 += 4
    call lcd_dma
    r0 = 0x2C
    call lcd_command
    r5 = lcd_line
    r6 = r2 >> 1
.Lfill:
    [r5 + 0] = r4
    r5 += 4
    r6 += -1
    if (r6 != 0) goto .Lfill
    r0 = lcd_line
    r1 = r2 << 1
.Lline:
    call lcd_dma
    r3 += -1
    if (r3 != 0) goto .Lline
    restore
/* 3x5 bitmap in low 15 bits, x, y. Each dot is 4x4 with 2px spacing. */
lcd_glyph:
    save
    r5 = r0
    r6 = r1
    r7 = r2
    .rept 5
    .rept 3
    r0 = r5 >> 14
    r1 = 1
    r0 &= r1
    if (r0 == 0) goto 2f
    r0 = r6
    r1 = r7
    r2 = 4
    r3 = 4
    r4 = 0xFFFFffff
    call lcd_rect
2:
    r5 = r5 << 1
    r6 += 6
    .endr
    r6 += -18
    r7 += 6
    .endr
    restore
/* bitmap-word string, x, y, count */
lcd_text:
    save
    r5 = r0
    r6 = r1
    r7 = r2
    r4 = r3
.Ltext:
    r0 = [r5 + 0]
    r1 = r6
    r2 = r7
    call lcd_glyph
    r5 += 4
    r6 += 18
    r4 += -1
    if (r4 != 0) goto .Ltext
    restore
/* 32-bit value, x, y */
lcd_hex:
    save
    r5 = r0
    r6 = r1
    r7 = r2
    r4 = 8
.Lhex:
    r0 = r5 >> 28
    r0 = r0 << 2
    r1 = hex_font
    r0 += r1
    r0 = [r0 + 0]
    r1 = r6
    r2 = r7
    call lcd_glyph
    r5 = r5 << 4
    r6 += 18
    r4 += -1
    if (r4 != 0) goto .Lhex
    restore
    .align 4
hex_font:
    .long 0b111101101101111, 0b010110010010111
    .long 0b111001111100111, 0b111001111001111
    .long 0b101101111001001, 0b111100111001111
    .long 0b111100111101111, 0b111001001001001
    .long 0b111101111101111, 0b111101111001111
    .long 0b111101111101101, 0b110101110101110
    .long 0b111100100100111, 0b110101101101110
    .long 0b111100110100111, 0b111100110100100
title:
    .long 0b111100110100100, 0b101111111101101, 0b010110010010111
time_label:
    .long 0b111010010010010, 0b111010010010111
    .long 0b101111111101101, 0b111100110100111
keys_label:
    .long 0b101101110101101, 0b111100110100111
    .long 0b101101010010010, 0b111100111001111
    .section .bss,"aw",@nobits
    .align 4
lcd_window: .space 8
lcd_line: .space 480
    .globl display_frames
display_frames: .space 4
    .globl display_ticks
display_ticks: .space 4
