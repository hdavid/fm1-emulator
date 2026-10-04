/* SPDX-License-Identifier: GPL-3.0-only
 * Derived from Felucca firmware/crt0.S, commit 1e838e17e170b20ff09b9660c9a7171aadfc5dca.
 * Original copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments.
 * Modified: small application-entry test. No hardware drivers or update service.
 * Use only in the emulator. For the FM-1 use the Felucca probe integration. */
    .section .text.crt0,"ax",@progbits
    .globl _start
    .globl probe_done
_start:
    goto 1f
    rts
1:
    sp = 0x01C7A000
    ssp = 0x01C7C000
    r0 = 0x01C08000
    call fm1_probe
probe_done:
    goto probe_done
