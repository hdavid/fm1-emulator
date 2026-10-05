# Baud Girl FM-1+VA (FM-1_093) and stock V15 (FM-1_015) boot log

Both images share the startup path. Every entry below was measured on both,
and both gave identical instruction counts. Command:
`target/release/examples/diagnose ~/GitHub/fm1-firmware/FM-1_093.fwsc 200000000`.
"Instructions" is diagnose's `executed:` count, which includes the secondary
core, so it is larger than the loop limit.

## Fixes (branch fix/baudgirl-boot)

| # | Blocker (PC) | Evidence | Fix | Instructions after |
|---|---|---|---|---|
| 1 | 1-byte read at 0x3 (0x02034b3c). The btif buffer pointer 0x01c0e6a0 was never set | `syscfg_btif` init (0x02034abc) opens `mnt/sdfile/app/btif` and fails with -766. sdfile copies the path component with memcpy (0x0204455c), whose tail `0312 0712 07b2` is `rep r2 { r2 = b[r1++]; b[r3++] = r2 }`. It copied only `b`, so `B` was compared with the `BTIF` directory entry and missed. The count register is the body's byte temporary and no branch follows, so the count must be latched. | `rep rA` runs its block rA times with a latched count; rA reads 0 afterwards (ccbfe7c) | 632,441 |
| 2 | `ee15 4200` at 0x02002bea | Conditional kind 0xe1 = register signed `>`. Same condition order as the compare-branches, where 0xee00 is signed `>` | 455f05f | 632,621 |
| 3 | `0488` at 0x02003238 | A variadic wrapper ends with push {r3..r0}; push rets; ...; sp += 16; 0488; sp += 16; rts. 0x0480 \| mask pops special registers by bitmap. Later superseded by the SLOOP agent's equivalent PopSpecial in the merge | pop {rets} (c6748b8) | 637,196 |
| 4 | write of 0x14300 (0x0203abb2) | JL_SRC resampler (WL82.h lsfr group 0x43), CON0 = 0 | register file; enabling a conversion faults (ddd3a54) | 96.6M, then the watchdog fired while list removal (0x0200552c) spun |
| 5 | circular list without its head | list_add_tail at 0x02003338 stores `{r2, r1}` with eb20 0006. Vendor-compiled Felucca `fm1_fault_c` (`{r5, r1} = [r4+]`) reads fm1_crash.magic into r1 | register-list loads/stores ascend from the lowest register; two old runtime tests corrected (a840af5) | 642,872 |
| 6 | `f1d0 2a00` at 0x0207493e | in-place 64-bit pair shift. The stock code writes a 40-bit field with it. Felucca sign-extends with `e1d0 4200; e1d0 4e00` and scales Q30 with `e1d0 490e` | ShiftPair (7f13eab) | 646,048 |
| - | merge feat/boot-fixes (SLOOP agent fixes) | | a0ba550 | 727,726 |
| 7 | read of 0x13b00 (0x02003472) | JL_RAND R64L (WL82.h lsfr 0x3b) | fixed-seed xorshift (54b41de) | 737,431 |
| 8 | write of 0x14040 (0x0205c610), then 0x30f04, 0x3101c, 0x11930 | `btctrler` task (created at 0x0205d834) programs JL_WL, the BT core window and JL_ANA WLA_CON. **STUB**: register file. The RF port busy bit 17 clears at once; WLA_CON30 bit 5 reads as ready (both inferred) | radio.rs (f8a8924, 0601235) | 96,737,028, then the **watchdog expires** (see below) |
| 9 | `e1d8 4700` at 0x02045210 (reached only with FM1_WATCHDOG_OFF=1) | soft-float unsigned-to-double: r5:r4 <<= r7 (47 for 48), then the bias 1023 and `e1d0 0304` | ShiftPairRegister (71eae48) | experiment only |
| 10 | `e1fc ae60` at 0x02044f5c (experiment) | 64-bit multiply-accumulate. Felucca DSP uses signed `e1fc bde0` before `e1d0 490e` | MultiplyAccumulateLong (8421d44) | experiment: runs the full 200M loop |

## Current blockers

1. **Watchdog at 96,737,028 instructions (normal run).** The last feed is at
   about 0.7M instructions. The btctrler RF calibration then busy-loops: the
   outer loop at 0x02036b2e counts r7 from 0 to 47 at about 3M instructions
   per pass, using calibrated delay loops (0x020369a8 and 0x02036a00). The
   watchdog is 4 s (96M oscillator ticks) and the emulator advances one 24 MHz
   oscillator tick per instruction. With `FM1_WATCHDOG_OFF=1` the calibration
   finishes at about 142M instructions, so the loop is finite. That the
   hardware runs it within 4 s because the CPU is faster than one instruction
   per oscillator tick is a hypothesis. It has not been measured on hardware.
   Changing the instruction/oscillator ratio would shift every regression
   baseline (Felucca, Jangada and SLOOP frame counts), so it was not done.
2. **`e86d 1602` at 0x0205d304 (experiment, ~206.6M).** Five e86d ops on
   [r1+0..16] (low bits 2, 3, 2, 3, 3) follow BT RF register reads. The
   family is memory-operand ops (e864 logic, e866 bit, e868 add, e86c shift
   by immediate). e86d is probably a shift by register r6, but r6 = 0 here,
   so the run cannot tell. It appears only in the stock image, not in
   Felucca or SLOOP. Not implemented: no evidence.
3. No LCD pixels yet. Neither image reaches display init before these blockers.

Experiment: `FM1_WATCHDOG_OFF=1` (diagnose prints `EXPERIMENT:`).
