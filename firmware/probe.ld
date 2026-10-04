/* SPDX-License-Identifier: GPL-3.0-only
 * XIP origin follows Felucca firmware/app.ld. */
ENTRY(_start)
MEMORY { XIP (rx) : ORIGIN = 0x02000120, LENGTH = 64K }
SECTIONS {
    .text : { KEEP(*(.text.crt0)) *(.text*) } > XIP
    /DISCARD/ : { *(.comment) *(.note*) }
}
