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
