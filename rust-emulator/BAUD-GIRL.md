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

| 19 | btstack task spinning on the HCI buffer | **The busy-poll was an emulator bug, not a missing controller.** The cbuf did hold data: `cbuf_read(cb, buf, 4)` at 0x02028286 copied 4 bytes and returned 4. Its bookkeeping `e868 6c1a; e868 6c16` is `data_len -= n; tmp_len -= n` (SDK `circular_buf.h`: tmp_len at +0x14, data_len at +0x18; Quarkslab pi32v2 lists 0x868 imm1617 = 2 as sub). The emulator added instead, so data_len grew (0x2c) and btstack re-read the same message forever. No HCI traffic was invented | fa64637 | 207.2M, usr_app_task runs |
| 20 | `ee81 e8a5` at 0x02021d3c | Float compare-branch (x bit 11): r14 = 0.192f vs r1 = 1.0f after an e53f add. Every use, in stock and in the SDK ELF objects, follows FPU code; integer-only Felucca/SLOOP/Jangada code never sets the bit. Decoded for eq/ne and the signed kinds only | 20c5598 | 207.6M |
| 21 | read of 0x16a0c (0x020356a8) | HUSB_PHY0_CON0..2 (WL82.h husb_phy_base); reset value unmeasured, 0 assumed | see log | |
| 22 | read of 0x51028 (0x02023b6a, usr_app_task) | JL_IOMAP CON2-4 and CON6-8 (WL82.h psfr 0x1007): plain pin-mux registers, reset value unmeasured (0) | see log | |
| 23 | 2-byte write of 0x12100 (0x02023be2) | JL_UART1 = the MIDI DIN port (hal/fm1_uart.h). Pending bits follow the SDK's debug.c putchar (bit 15 TX pending, bit 13 clears it) and fm1_uart.h. No MIDI input is connected; TX completes at once; BUF bytes are logged (a `UART1 (MIDI out)` diagnose line) | see log | |
| 24 | `0x0001` at 0x0205b8da (IDLE0 task) | `asm("idle")` (SDK init.c / adc_api.c wait-for-interrupt), executed as a hint | see log | |
| 25 | btctrler spin on 0x2001c (0x0206f096, interrupts off) | **STUB, inferred**: a self-clearing BT command register. The code writes it and then polls it until 0 with interrupts disabled, so only hardware can clear it | see log | 248M |
| 26 | SPI1 DMA from flash 0x0204f8b2 (0x02023de0); CASET 0..240; ST7789V config commands (b2, b7, bb, c0, c2-c4, c6, d0, e0, e1, e7, 51) | The stock app sends its panel init table from XIP flash, so DMA from flash is allowed. Window addresses past the frame memory are accepted and their pixels dropped: this follows my reading of the ST7789V datasheet's CASET/RASET note, which I have not re-checked against the document. Frame memory is 240x320 (stock uses RASET 40..279). Config commands have no frame-memory effect | see log | 57,361 pixels |
| 27 | `0x13c0` at 0x0200de98 | Resolved by the main session with JieLi's objdump: `r0 = b[r4++=r15] (u)`. The whole 0x1000-0x13ff family is byte load/store with register post-increment | f092145 (feat/boot-fixes) | |
| 28 | `e53f 5f22` at 0x0201e4a4 | objdump `--mattr=+fprev1` gives the whole unary set: ftoi/ftou with (even/trunc/ceil/floor) rounding, itof, utof, `rD.l = ftof(rC)` / `rD = ftof(rC.l)` (binary16), integral-float rounding. It confirmed the earlier inferred 0x8f = itof and 0x9f = utof. Op 4 is `fcmp`; its flags are unknown, so it stays undecoded | 220d532 | 305M |
| 29 | float compare-branch predicates | objdump prints `iff`: e800 ==, e880 u!=, e900 u>=, e980 <, ec00 u>, ec80 <=, ed00 >=, ed80 u<, ee00 >, ee80 u<= (u = also taken when unordered). The earlier version only had eq/ne/signed kinds, with ordered semantics | 220d532 | |
| 30 | read of 0x51004 (0x02025878, battery/charge code) | JL_USB_IO CON1 (bit 1 read). **STUB**: register, reset value assumed 0 | 220d532 | |
| 31 | byte write to 0x16001 (0x02006f76) | Stock enables the high-speed USB controller for usb id 1. **STUB** husb.rs: register file, and SIE_CON bit 4 reads ready once bits 0-1 are set (inferred from the poll). Nothing is attached | 220d532 | |
| 32 | SPI1 CON 0x2021 | The stock LCD driver runs DMA from the SPI1 interrupt (IRQ 16, CON bit 13 = IE). Interrupt mode completes after the shift time (lsb_clk *assumed* to be 4x osc); polled mode (Felucca) is unchanged. The IRQ fast path `any_irq_pending` from the perf merge lacked the source. With instant completion the CPU stayed in the handler until a FreeRTOS queue lock overflowed (assert at 0x0205a1fa, then reset) | 220d532 | runs the full loop |

## Current state (FM1_CPU_MHZ=192)

- **Both images reach their main screens and run a stable main loop.**
  Baud Girl at 800M steps: 1.55G instructions, 1,840,592 LCD pixels, audio
  309,288 frames, no fault.
- **Baud Girl:**
  - `~/GitHub/fm1-firmware/screens/baudgirl-192mhz-splash.png` ("BAUD
    GIRL / VERSION-93").
  - `baudgirl-192mhz-main.png` and `baudgirl-192mhz-main-800M.png`: four
    level meters ("100"), "Volume 00" popup, "0011", battery icon.
- **Stock V15:**
  - `stock-v15-192mhz-main.png`: an "OSC" screen with a full-height blue
    vertical-stripe area and a yellow line.
  - **Not verified against hardware.** I cannot tell whether the stripes
    are the real waveform view or an emulation artefact. The left digits
    and the right-hand battery look clipped.
- **Unverified assumptions behind these screens:**
  - lsb_clk rate.
  - USB_IO CON1 = 0 (the not-charging path).
  - High-speed USB "ready" bit.
  - Radio stubs.
- **Open question for someone with the hardware:** which 240 of the 320
  frame-memory rows the glass shows. Both firmwares draw their UI in rows
  0-239 after init, so the question matters less than it seemed.

## Former blocker: `0x13c0` at 0x0200de98 (resolved, fix 27)

- **Where it is:** in a text loop. The code just before tests r6 != 13
  (CR) and r7 != 10 (LF), adds r13 to the counter at [sp+0x14], and sets
  r4 = r10 (a buffer pointer, 0x01c654e0). The code after compares r0
  with 10 and 13 and later loads r0 = b[r4].
- **Why I didn't implement it:** 0x12xx/0x13xx appears nowhere else as a
  standalone instruction. Every other occurrence, in stock, Felucca, SLOOP,
  Jangada and the SDK ELF objects, is an operand halfword. The Quarkslab
  spec has no `ins0712 = 0x27` form.
- **The path into it looks valid** (goto at 0x0200dd3e to 0x0200de5a, a
  branch to 0x0200de78, then straight-line code). So either this is a rare
  instruction (a byte load or a three-register op on r0/r4/r7 would fit
  the use) or an earlier instruction was executed with the wrong
  semantics. Not implemented: no evidence.

### Round 4 investigation (still blocked)

- **Alignment checks out.** The executed lengths are 0x0200de88 e9d4 f014
  (4 bytes), de8c f80d 001e (4), de90 18df (2), de92 e9d4 f015 (4), de96
  16a4 (2). The decode cache is not involved: with it disabled the run
  stops at the same place (main session).
- **No other occurrence anywhere, anywhere else checked.**
  - The second FM-1_093 hit (0x0204a564) is inside a data table of
    4-halfword records (`100b 0001 158c 0560 ...`).
  - A linear sweep of every SDK ELF function (libVolcEngineRTCLite.a,
    cpu.a, system.a, codec .a ELF members) finds 0x10xx-0x13xx only inside
    inline jump tables after 0x0102/0x0103/0x0107 table branches, never
    as an instruction.
  - A sweep from every direct call target of stock, Baud Girl, Felucca,
    SLOOP and Jangada finds none as an instruction either.
  - Nothing in the Quarkslab pi32 v1 or q32s specs matches.
- **What the surrounding function shows:**
  - It is a text writer called through a function pointer. At 0x0200de5a
    the format loop has ended.
  - `0x13c0` must set r0. The two branches right after it send r0 == 10 and
    r0 == 13 to 0x0200decc (the exit). Otherwise the code loads
    r0 = b[r4] (r4 = buffer 0x01c654e0), tests that for CR/LF too, and if
    found increments the counter at [sp+0x14] (0x0200dec4).
  - Registers at the fault: r2 = 0x0a (the last character processed),
    r12 = 0, r0 = r1 = r5 = r6 = r7 = 0.
  - "r0 = last character" would fit the C shape. But no field of
    0x13c0 (`0001 0011 1100 0000`) names r2, and the r12 reading
    (`13 c 0` as mov-like dest r0, src r12) gives 0, which makes the
    first test dead code.
- **What would unblock it:** the vendor objdump for pi32v2 (it decodes all
  forms), any toolchain-built object containing a 0x10xx-0x13xx
  instruction, or the stock source of this text writer. Guessing a meaning
  from the register values above would be a fake.

New diagnose options: `FM1_CALLERS=PC`, `FM1_SPI2_LOG=1`, `FM1_PNG_RAM`,
a `secondary core:` line and a `UART1 (MIDI out)` line.

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
2. (Resolved by fix 11; the analysis below was superseded.) **`e86d 1602` at 0x0205d304 (experiment, ~206.6M).** Five e86d ops on
   [r1+0..16] (low bits 2, 3, 2, 3, 3) follow BT RF register reads. The
   family is memory-operand ops (e864 logic, e866 bit, e868 add, e86c shift
   by immediate). e86d is probably a shift by register r6, but r6 = 0 here,
   so the run cannot tell. It appears only in the stock image, not in
   Felucca or SLOOP. Not implemented: no evidence.
3. No LCD pixels yet. Neither image reaches display init before these blockers.

Experiment: `FM1_WATCHDOG_OFF=1` (diagnose prints `EXPERIMENT:`).

## Rendering artefacts (round 6): CPU forms, not the LCD controller

- **Method.** New diagnose options show where a bad pixel came from:
  - `FM1_LCD_LOG=N` logs SPI1 commands and DMAs together with the current
    CASET/RASET window.
  - `FM1_LCD_OWNER=x,y;...` names the transfer that last wrote a visible
    pixel.
- **The panel gets exactly what the guest renders:**
  - Both firmwares draw full-width 24-row strips from two RAM buffers (IRQ
    DMA) and small glyph windows through a 128-byte bounce buffer (polled
    DMA).
  - FM1_MEMWATCH on a bad strip-buffer word showed the guest storing the
    wrong value there. The renderer was at fault, not the panel model.
- **Three forms disagreed with the JieLi objdump (fixed in d1574e9):**
  - **Packed immediates.** x[9:8] = 1 gives `0x00AB00AB` and 2 gives
    `0xAB00AB00`; the emulator had `0xAB00AB00` and `0xAB`. The stock fill
    builds its two-pixel word with `e1e0 0101` = `r0 * 0x10001`, so every
    fill came out as alternating stripes. This also caused Baud Girl's
    dotted meter lines.
  - **ed50-ed5f halfword forms.**
    - The offset is signed 10-bit from h[1:0]: `ed5b cf2a` =
      `r12 = h[++r2=-6] (u)`, which the emulator read at +506.
    - Bit 2 means a sign-extending load or, for stores, writing the upper
      halfword (`ed55 0f2b` = `h[r2+506] = r0.h`).
    - The glyph alpha blender reads its right-edge background pixels with
      these forms, which explains the garbling at glyph right edges.
  - **Bit-field forms.** `e1bX` x bit 0 means `sextra`. `e1aX`/`e1bX` with
    other low bits are different forms.
- **LCD controller left unchanged.**
  - MADCTL/COLMOD: both firmwares send MADCTL 0 and COLMOD 0x55, already
    handled.
  - The round-3 rule (window addresses beyond the frame memory accepted,
    their data dropped) is still unverified against the ST7789V datasheet,
    which I don't have. In these runs the only out-of-range window is the
    stock CASET 0..240 used to clear the screen at init.
  - I also tried streaming interrupt-mode DMA bytes from RAM as the shift
    time passed. It changed nothing visible and there is no evidence about
    the DMA fetch behaviour, so it was removed.
- **Screens.** In `~/GitHub/fm1-firmware/screens/`:
  `baudgirl-192mhz-main-{before,after}-lcdfix.png` and
  `stock-v15-192mhz-main-{before,after}-lcdfix.png`. After the fix:
  - Stock shows a clean grey "OSC" screen with a flat yellow trace, "001"
    and the battery.
  - Baud Girl shows clean "100" meters and "001".
