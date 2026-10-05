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

| - | merge feat/boot-fixes (selectable CPU clock 3455af3). From here runs use `FM1_CPU_MHZ=192` (WL82 runs at 120-396 MHz), which clears the RF-calibration watchdog | | 97de989 | 204.5M, `e86d` at 0x0205d304 |
| 11 | `e86d 1602` at 0x0205d304 | Each of five record words gets bits 24-31 (`e1a0 4c20`) and 22-23 (`e1a4 0b08`) from RF registers. Then e86d 16x2/16x3 = `[r1+off] >>= 22` (h bit 0 = bit 4 of the amount; mode 2 logical, mode 3 arithmetic) extends the 10-bit fields. e86c amounts stay 0-15: Felucca `audio_block` `out[2i+1] <<= OUT_SHIFT` (7) is `e86c 3704` | 5d74aba | 204.9M |
| 12 | TIMER4/5 CON = 1 (0x0200573a) | source 0 = lsb_clk (SDK `clk_get("timer")`). **STUB**: counted at the oscillator rate | c4c1389 | |
| 13 | `ee37 5fff` at 0x02004872 | conditional kind 0xe3 = signed `>` against imm12 (`if (r7 > -1)` on a signed byte) | c4c1389 | 205.6M |
| 14 | write of 0x11e00 (SPI2) | **STUB**: SPI2 with no device. MISO idles; a transfer completes after `bytes*8*(BAUD+1)` lsb ticks (lsb_clk *assumed* to be 4x osc) and raises IRQ 37 when CON bit 13 is set. The stock driver restarts a 2-byte DMA (`ff fe`) from that interrupt, so instant completion starved every task | f1df6a0, cf9d6bd | |
| 15 | `e53f` (single-precision FPU, built with -mfprev1) at 0x02004ade | Semantics come from the AC79 SDK's prebuilt **ELF** objects in `libVolcEngineRTCLite.a` (`rx_net_samples_avg`: `(float)n`, then `sum / n`) and from stock code. `rD = rS op rC` with 0 add, 1 sub, 2 mul, 3 div, 5 min, 6 max, 7 `rD += rS*rC`, 8 `rD -= rS*rC` (stock complex multiply). Unary 0x8f/0x9f = int->float (signed/unsigned), 0x1f = float->int. Unseen sub-ops (0x2f, 0x5f) stay unsupported | f1df6a0 | 206.9M |
| 16 | `e8d9 0df0` at 0x02074b18 | masked push/pop with rets/pc (e8d9/e8d5 pair as prologue/epilogue) | 0ca626d | 211.9M |
| 17 | nested `rep` at 0x01c05026 | A repeat restored by rti leaked into another task. The RTOS context switch saves only r0-r15 and {psr, rets, reti}, so no interrupt is now taken inside a repeat or conditional block | f9c871f | runs the full loop |
| 18 | `04e1` in the SPI2 interrupt handler | special-register bitmap {reti, psr}, popped by `04a1` | cf9d6bd | runs the full loop |

## Current state (FM1_CPU_MHZ=192, 300M loop = 479,175,657 instructions)

- Stable main loop: no fault, watchdog fed (54k feeds), interrupts running.
- Audio DMA active: 62,652 stereo frames, 978 halves.
- **No LCD pixels.**
- Stock and Baud Girl are still identical.

## Blocker: the display task never runs (btstack busy-polls)

- **The stock LCD driver exists but is never reached.** It drives SPI1
  (0x11d00) with `CON = 0x4021`, the same value as Felucca's HAL, at
  0x02023d68. That code is inside the `usr_app_task` entry 0x02023b46.
- **`usr_app_task` is created but never scheduled.** It is created at
  0x02004c46 (priority 5, stack 0x400). Its initial frame at 0x01c64744
  (entry wrapper 0x0205bd18, arg 0x01c636a0) is never consumed. Its task
  record flag is 0, while every task that ran shows 0xa3. `midi_route` is
  in the same never-run state.
- **The `btstack` task takes the CPU time** (sp around 0x01c27a18). It loops
  at 0x0207d174..0x0207d39a. There, `cbuf_read(4)` (0x02028286) on an empty
  HCI buffer returns 0, and state word 0x2c keeps it polling without ever
  blocking. Lower-priority tasks starve.
- **Why the buffer stays empty (inferred):** the Bluetooth controller
  (btctrler) would fill it, and it normally runs from BT baseband
  interrupts. No BT core or baseband is emulated (radio.rs only keeps
  register values), so no HCI event arrives.
- **What progress needs:** either a minimal BT controller model (HCI
  command complete for the reset sequence) or the BT core interrupt
  sources. Neither was done: there is no documentation, and inventing HCI
  traffic would be a fake.
- **The secondary core is also stuck behind this.** It waits at 0x01c02480
  for byte 0x01c192e0 or 0x01c192e1 to become 2. The setters (0x0201dc8e,
  0x0201dda4) are called from UI/LCD code (0x02023eb8), so they never run
  either.

New diagnose options: `FM1_CALLERS=PC` (rets histogram in the hot window),
`FM1_SPI2_LOG=1`, and a `secondary core:` line.

## Earlier blockers (default 24 MHz clock)

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
