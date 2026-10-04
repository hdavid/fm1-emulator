/* SPDX-License-Identifier: GPL-3.0-only */
/* Small application-entry integration firmware. Uses the same XIP/RAM layout
 * and peripheral addresses as Felucca. No ROM/SPL, USB updater, or recovery;
 * this image is for emulator tests and must not be flashed as an update. */
    .section .text.crt0,"ax",@progbits
    .globl _start
_start:
    goto .Lentry
    rts
.Lentry:
    cli
    sp = _ustack_top
    ssp = _sstack_top

    /* These RAM initializers stay in flash until startup executes the copies. */
    r0 = _data_load
    r1 = _data_start
    r2 = _data_end
.Ldata:
    r3 = [r0 + 0]
    [r1 + 0] = r3
    r0 += 4
    r1 += 4
    r3 = r2 - r1
    if (r3 != 0) goto .Ldata

    r0 = _rt_load
    r1 = _rt_start
    r2 = _rt_end
.Lramtext:
    r3 = [r0 + 0]
    [r1 + 0] = r3
    r0 += 4
    r1 += 4
    r3 = r2 - r1
    if (r3 != 0) goto .Lramtext

    r0 = _bss_start
    r1 = _bss_end
    r2 = 0
.Lbss:
    [r0 + 0] = r2
    r0 += 4
    r3 = r1 - r0
    if (r3 != 0) goto .Lbss

    r0 = cpu_probe_results
    call fm1_probe
    r0 = foundation_results
    r1 = foundation_data
    r2 = [r1 + 0]
    [r0 + 0] = r2
    r1 = foundation_bss
    r2 = [r1 + 0]
    [r0 + 4] = r2
    r1 = ram_magic
    call r1

    /* Input matrix: 16 serial bits through PA4/PA3, latch on PA1. Scan all
     * 11 columns and read PA0/PA5..PA8, exactly as fm1_input.h does. */
    r0 = 0x50000
    r1 = 0x1E1
    [r0 + 8] = r1
    [r0 + 16] = r1
    r1 = 0x1FB
    [r0 + 12] = r1
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
    r0 = matrix_results
    r1 = [r0 + 0]
    r2 = foundation_results
    [r2 + 20] = r1

    /* TIMER4: OSC /1, free-running, as in fm1_time.h. */
    r0 = 0x10800
    r1 = 0
    [r0 + 0] = r1
    [r0 + 4] = r1
    r1 = 0xFFFFFFFF
    [r0 + 8] = r1
    r1 = 0x4009
    [r0 + 0] = r1
    r1 = [r0 + 4]
    r2 = foundation_results
    [r2 + 12] = r1
    r3 = 10
.Lwait_ticks:
    r3 += -1
    if (r3 != 0) goto .Lwait_ticks
    r1 = [r0 + 4]
    [r2 + 16] = r1

    /* TIMER5: OSC /4, period 600, interrupt 63, as in fm1_timer.h. */
    r0 = 0x1C7FEFC
    r1 = foundation_timer_irq
    [r0 + 0] = r1
    r0 = 0x1EEF11C
    r1 = 0x30000000
    [r0 + 0] = r1
    r1 = 0x100
    icfg = r1
    r0 = 0x10900
    r1 = 0x4000
    [r0 + 0] = r1
    r1 = 0
    [r0 + 4] = r1
    r1 = 600
    [r0 + 8] = r1
    r1 = 0x4019
    [r0 + 0] = r1
    csync
    sti
    r0 = foundation_irqs
.Lwait_irq:
    r1 = [r0 + 0]
    if (r1 == 0) goto .Lwait_irq
    cli
    r0 = 0x10900
    r1 = 0x4000
    [r0 + 0] = r1
    r0 = foundation_irqs
    r1 = [r0 + 0]
    r2 = foundation_results
    [r2 + 24] = r1
    r0 = foundation_irq_sp
    r1 = [r0 + 0]
    [r2 + 28] = r1
    r1 = sp
    [r2 + 32] = r1
    r1 = 0x0050F00D
    [r2 + 36] = r1
#ifdef FM1_DISPLAY
    call display_main
#endif
    .globl foundation_done
foundation_done:
    goto foundation_done

    /* Same IRQ save/restore sequence as firmware/hal/fm1_isr.S. */
    .globl foundation_timer_irq
foundation_timer_irq:
    [--sp] = {psr, rets, reti}
    [--sp] = {r3-r0}
    r0 = 0x10900
    r1 = [r0 + 0]
    r2 = 0x4000
    r1 |= r2
    [r0 + 0] = r1
    r0 = foundation_irqs
    r1 = [r0 + 0]
    r1 += 1
    [r0 + 0] = r1
    r0 = foundation_irq_sp
    r1 = sp
    [r0 + 0] = r1
    {r3-r0} = [sp++]
    {psr, rets, reti} = [sp++]
    csync
    rti

    .section .ram_text,"ax",@progbits
    .globl ram_magic
ram_magic:
    r1 = 0x5A17
    [r0 + 8] = r1
    rts

    .section .data,"aw",@progbits
    .globl foundation_data
foundation_data:
    .long 0xC001CAFE

    .section .bss,"aw",@nobits
    .align 2
    .globl foundation_bss
foundation_bss:
    .space 4
    .globl foundation_irqs
foundation_irqs:
    .space 4
    .globl foundation_irq_sp
foundation_irq_sp:
    .space 4
    .globl foundation_results
foundation_results:
    .space 40
    .globl cpu_probe_results
cpu_probe_results:
    .space 48
    .globl matrix_results
matrix_results:
    .space 44
