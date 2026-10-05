# Baud Girl FM-1+VA (FM-1_093) and stock V15 (FM-1_015) boot log

Both images share the startup path; every entry below was measured on both
unless noted. Command:
`target/release/examples/diagnose ~/GitHub/fm1-firmware/FM-1_093.fwsc 200000000`.

| # | Blocker (PC) | Evidence | Fix | Instructions after |
|---|---|---|---|---|
| 1 | read byte at 0x3 (0x02034b3c): btif buffer pointer 0x01c0e6a0 never set | `syscfg_btif` init (0x02034abc) opens `mnt/sdfile/app/btif` and fails (-766). sdfile copies the path component with memcpy (0x0204455c); its tail `0312 0712 07b2` = `rep r2 { r2 = b[r1++]; b[r3++] = r2 }` copied only `b`, so `B` was compared against the `BTIF` directory entry and missed. The count register is the body's byte temporary and no branch follows, so the hardware must latch the count. | `rep rA` runs its block rA times with a latched count; rA reads 0 afterwards (`register_repeat_*` tests) | 632,441, then `unsupported instruction 0xee15 at 0x02002bea` |
