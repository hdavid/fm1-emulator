/* SPDX-License-Identifier: GPL-3.0-only */
/* Pure CPU test. r0 points to 12 output words. Preserve r1..r7.
 * The same object is linked into the emulator image and Felucca.
 * No MMIO, global state, or interrupt changes. */
    .section .text.fm1_probe,"ax",@progbits
    .globl fm1_probe
    .globl fm1_probe_end
fm1_probe:
    [--sp] = {r7-r1}
    r1 = 0x12345678
    [r0 + 0] = r1
    r1 = 0xFFFFFFFF
    r2 = 2
    r1 += r2
    [r0 + 4] = r1
    r1 = 3
    r2 = 5
    r1 -= r2
    [r0 + 8] = r1
    r1 = 0xA5A55A5A
    r2 = 0x12345678
    r1 ^= r2
    [r0 + 12] = r1
    r1 = 0xA5A55A5A
    r1 &= r2
    [r0 + 16] = r1
    r1 = 0xA5A55A5A
    r1 |= r2
    [r0 + 20] = r1
    r1 = ~r2
    [r0 + 24] = r1
    r1 = r2 << 4
    [r0 + 28] = r1
    r1 = r2 >> 4
    [r0 + 32] = r1
    r1 = [r0 + 0]
    r1 += 7
    [r0 + 36] = r1
    r1 = 0
    r3 = 10
.Lsum:
    r1 += r3
    r3 += -1
    if (r3 != 0) goto .Lsum
    [r0 + 40] = r1
    r4 = 0x13579BDF
    [--sp] = {r4}
    r4 = 0
    {r4} = [sp++]
    [r0 + 44] = r4
    {r7-r1} = [sp++]
    rts
fm1_probe_end:
