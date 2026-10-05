# SLOOP 2.2 boot log

SLOOP is a GPL-3.0 fork of Felucca; its full source (`sloop-ref/firmware`)
lets each emulator gap be checked against the C it was compiled from.

Run: `target/release/examples/diagnose ~/GitHub/fm1-firmware/sloop-2.2.fwsc 200000000`

Regression set after every fix (200M instructions): Felucca `LCD: 4235962`,
`audio: 318151`; Jangada `LCD: 4211962`, `audio: 318136`; stock and Baud Girl
not below `executed: 648199`.

## Starting point

`executed: 28976636`, `LCD: 168960 pixels written`, `audio: 1325 stereo
frames`, then `unsupported instruction 0xeea3 at PC 0x020035d2`.

## 1. `eea3 0d80` at 0x020035d2: `ifs (rA <= #packed)`

Bytes `a3 ee 80 0d`; the three real uses in SLOOP are 0x020035d2
`eea3 0d80`, 0x02006ee0 `eea2 0d08` and 0x02006f04 `eea2 0cb0`. Every other
`eeaX` hit in the five images is the second halfword of a `call` (`eabf`,
`ff80`) or of a `mov_imm32`, or data (Jangada's 0x02003468 / 0x0200bb98
follow `eabf`), so only SLOOP executes this form.

Analysis: kind `(h >> 4) & 255 = 0xea` sits in the conditional-block family
whose low three bits pick the right operand (1 register, 2 packed
immediate, 3 signed imm12) and whose `0xe8` row is signed `<=` (`0xe9`,
`0xeb` already decoded). So `0xea` = `ifs (rA <= #packed)`.

Evidence:
- `packed(0x0d08) = 0x2200` and `packed(0x0cb0) = 0x5800`; the single
  instruction in each arm is `e041 2200` / `e041 5800` (r1 = the same
  value). punch.c: `punch.cut = punch.cut > (34 << 8) ? punch.cut - 96 :
  34 << 8` and the HPF `> (88 << 8) ? cut - 64 : 88 << 8`; `cut` is
  `int32_t`, hence the signed compare. The preceding `e131 2fa0` / `2fc0`
  are the `- 96` / `- 64`.
- `packed(0x0d80) = 0x1000`: fx.c `gain_next`, `MUTE_STEP 4096`:
  `a - to > MUTE_STEP ? a - MUTE_STEP : to` (both directions folded into
  r3 = |to - a|, r4 = a -/+ 4096; the arm `1614` sets r4 = to); the function
  then stores r4 to `t->att` (`60a4`) and returns `0x7fff - r4` (`1f00`),
  i.e. `32767 - t->att`.

Fix: decode kind `0xea` as a conditional block (execution already handled
the signed `<=` with a packed operand). Tests: `tests/sloop_isa.rs`.

Result: `executed: 193392754`, `audio: 303440 stereo frames, 1185 DMA
halves`, `LCD: 168960 pixels written` (unchanged), then `watchdog expired`
at 0x0200508e. `ADC: 0 conversions` (Felucca: 672).

## 2. Watchdog at 0x0200508e: reverb index never wrapped (`ecb1 0ae6`)

Symptom: a 12-instruction loop at 0x02005082..0x0200517e (seq.c's USB-MIDI
drain, `while (mi_r != mi_w)`) with `mi_r = 0x12050ee`, `mi_w = 0xc0021`:
it would run ~4G times, so the watchdog fired. `FM1_MEMWATCH=0x1c177b4`
showed the queue words written at step 37177740 by 0x02006a96, a halfword
store `edd8 c019` to `rev_line + 2 * (0x1705 + line_i[3])`: fx.c
`fx_buses`, `c[B3 + fx.line_i[3]]` with B3 = 1573 + 1931 + 2389 = 0x1705
(the constants 0x625, 0xdb0, 0x1705 are all in the routine). All four
`line_i` were 0x3b0c, far past `REV_LINE` (1559 + 14, 1931, 2389, 2791), so
the delay lines had run off the end of `rev_line` across .bss.

Analysis: the wraps `if (++fx.line_i[k] >= REV_LINE[k]) fx.line_i[k] = 0`
compile to `r7 = 0; if (r1 <= #N-1) { r7 = r0 }` with kind 0xcb, e.g.
`ecb1 0ae6` = `<= 2790`. The emulator (following Quarkslab, which lists 0xcb
as `packedimm12`) read `packed(0x0ae6) = 0x7300`, so the index ran to 29440.
The operand is the plain 12 bits (0xae6 = 2790 = REV_LINE[3] - 1), matching
the family rule that low bits 3 select imm12. Zero-extended: a sign-extended
0xfffffae6 would never wrap either.

Fix: kind 0xcb compares against `x & 0xfff`.

Open: kind 0xc3 (`>`, imm12 by the same rule) is still read as packed; the
only image use where the two readings differ is SLOOP 0x0200451a
`ec30 a600` (0x600 vs 0x08000000), not reached in 200M instructions.

Result: `executed: 200000000`, `LCD: 4897854`, `audio: 315581`. Screens:
the splash at 40M, then (120M) the REC-armed screen "play freely / then rec
on the 1", (160M) "RECORDING", (200M) black: the UI saw REC presses that no
key made (the GPIO matrix never returned a closed row: checked with a
temporary probe on port A reads).

## 3. Phantom REC presses: `e190 xxx3` is and-not, not `~rC`

`FM1_MEMWATCH=0x1c17760` (rec_wait is the byte at +3, found from the
`b[r11 + 3] = 0` store in rec_toggle's "REC OFF" branch next to the string
reference at 0x0200f212) showed rec_wait set at step 94000713 by
0x020104cc. The path came from ui_input.c with `pressed = 0xfffff133`,
computed at 0x0200f17c by `e190 c133`: r12 = r3 (op) r1 with r1 = 0xecc,
the seven layer-button bits. The emulator implemented mode 3 as `~r1`,
ignoring r3, which made every other button (REC = bit 13) "pressed".

Evidence for and-not (`rS & ~rC`): it is the mode-3 meaning in
memory_logic (`old & !value`) and logic_immediate mode 7; and the stock
images use `2141 ; e190 0013` / `2144 ; e190 1143` (r1 = 1; r0 = r1 & ~r0),
which is C's `!flag` for a 0/1 flag only under and-not (243 uses of
`e190 xxx3` across the five images).

Fix: mode 3 = `r[s] & !r[c]`. Felucca and Jangada unchanged; stock and Baud
Girl now run 729174 instructions (was 648199) to the same stop at
0x02034b3c.

Result: SLOOP `executed: 95961853`, then `unsupported instruction 0x0488 at
PC 0x020108cc`.
