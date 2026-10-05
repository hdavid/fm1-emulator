# Preset sweep

`examples/preset_sweep.rs` boots a firmware, then clicks the PRESETS encoder
one detent at a time. For each preset it holds a four-note chord (matrix keys
18, 20, 22, 23) for 1.5 s of guest time, releases it, and records:

- the hash of the LCD footer (the preset name row),
- the output peak in the 0.3 s before the chord (`idle`),
- the chord's frames, RMS and peak, or the fault with a 200-instruction
  trace in `OUT_DIR/fault-K.txt`.

The sweep stops when the footer shows preset 0's name again.
`OUT_DIR/footers.png` stacks every footer so the names can be read.

```
cargo build --release --examples
target/release/examples/preset_sweep ~/GitHub/fm1-firmware/felucca-0.9-beta.fwsc /tmp/sweep-felucca
```

## Faults found and fixed

| Engine | First seen | Instruction | Cause | Commit |
|---|---|---|---|---|
| GRAIN | Felucca 0x02011fcc | `eb04 8004` load register list | Registers were transferred highest-first. The real order puts the lowest register at the base. `{r2, r15}` loads gr_grain_t `{z, pos}`, so r2 got `pos` (0x256c) and the read of `z->n` at `[r2+4]` faulted. | 899c884 |
| GRAIN | Felucca 0x02011e98 | `edd0 10f3` | x bit 0 = store (`h[r15 ++= 2] = r1`, gr_fill writing the reverse window). It was decoded as a load with increment 3. | 6439a4d |
| VOICE | Felucca 0x02010462 | `e1d0 2a04` | Unsupported: shift of the 64-bit register pair r3:r2 by 36 (eng_formant.c `f0 >> 4`) | 203d500 |
| TRIO | Felucca 0x02010ac0 | `ec50 2213` | Unsupported: pair store with base write-back (trio_note_on `ph[0..1]`) | 203d500 |
| VOICE | Felucca 0x020108cc | `e1fc 50a0` | Unsupported: signed 64-bit multiply-accumulate | 789d73e |
| VOICE | Jangada 0x020127be | `e1f6 3620` | Signed 64/32 divide (x bit 12). It was rejected as an odd destination register. | 789d73e |

Jangada 0.1-alpha has the same engine code, so it had the same faults. In
the first sweep after the two GRAIN fixes, 9 of Felucca's 54 presets and 13
of Jangada's 74 presets faulted: every VOICE and every TRIO preset.

Each fix has a test in `tests/stock_isa.rs` that uses the exact halfwords
from the image. Each test failed before its fix.

The register-list order also fixes the stock FM-1 linked-list code. Its
insert (0x02002284) and remove (0x0201fb7a) are only consistent with
`{next, prev}` = `{r1, r2}`. The two runtime tests that asserted the old
order were wrong and are corrected.

## Results (after the fixes, branch fix/preset-crashes)

| Firmware | Presets played | Faults | Silent (peak < 0.001) |
|---|---|---|---|
| felucca-0.9-beta | 54 (wrapped to preset 0) | 0 | 0 |
| jangada-0.1-alpha | 74 (wrapped to preset 0) | 0 | 0 |
| sloop-2.2 | 60 (click limit, see below) | 0 | 0 |

Felucca order: ANALOG ×8, DIGITAL ×8, PHASE ×6, LOFI ×5, SAMPLE ×5,
VOICE ×4, TRIO ×5, WHEEL ×5, GRAIN ×4, ANALOG ×4 (SAW LEAD .. PWM STR).

Jangada adds presets to most engines: ANALOG ×14, DIGITAL ×10, PHASE ×7,
LOFI ×6, SAMPLE ×7, VOICE ×5, TRIO ×8, WHEEL ×7, GRAIN ×6, ANALOG ×4.

Quietest chords (low peak, but not silent):

- Jangada 69, GRAIN DRONE DUST: peak 0.0068 (idle 0.0152 from the previous
  preset's tail).
- Jangada 23, DIGITAL DRONE FM: peak 0.0200.
- Jangada 48, VOICE DRONE VOX: peak 0.0239.

All three are drones, which plausibly have slow attacks. That they sound
right on hardware is not verified. Lowest on Felucca: peak 0.045 (presets
10 DIGITAL BASS, 23 LOFI WAVE BASS and 25 LOFI WAVE LEAD).

SLOOP: its LCD footer does not show the preset name. The footer alternates
between two images, so the sweep cannot tell when it wraps, and I did not
verify that each click selected a different preset. 60 clicks with a chord
each: no faults, every chord audible.

Only "no fault" and "audible" were checked. The sweep cannot tell whether
an engine sounds right.

## Regression checks

- `cargo test --release --features gui`: 137 passed, 2 ignored.
  - 130 passed after merging feat/boot-fixes.
  - The extra 7 are new stock_isa tests.
- `diagnose` at 200000000 instructions, unchanged by every fix:
  - Felucca: LCD 4235962, audio 318151.
  - Jangada: LCD 4211962, audio 318136.
  - SLOOP: LCD 284160, audio 315581.
  - FM-1 and FM-1_093: executed 729174.
