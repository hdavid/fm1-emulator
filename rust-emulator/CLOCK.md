# CPU clock: what the FM-1 runs at, and what the emulator does

Written 2026-10-05. Tags: **[R]** reported by the named project and not
re-checked here, **[M]** measured here in an emulator, **[V]** verified here in
source, **[I]** inference.

## Summary

- The FM-1's CPU runs at **360 MHz** after the boot loader (SPL) hands over.
  Two independent register captures on physical FM-1s agree [R].
- The "240 MHz" from static analysis of the stock firmware (AL-255/FM-1-RE) is
  the PLL of the high-speed USB PHY block at 0x16A00, not the CPU clock [V, I].
  The stock application leaves the CPU clock selection untouched [M].
- The SDK's `sys_clock_table` lists up to 320 MHz, plus a 396 MHz
  "overclocking" entry. Our older notes said "≤320 MHz" from that table. That
  was a guess about the configuration, not a measurement.
- On this branch the instruction clock defaults to the firmware's own system
  clock, derived from these registers as upstream does (360 MHz at the
  handoff). It can be overridden with `FM1_CPU_MHZ` in the tools and
  `--cpu-mhz N` in `fm1-ui` (a multiple of 24 MHz). The older fork baselines
  were measured at a fixed 24 MHz, which is why they differ from default runs.

## The registers and how they decode

Addresses from the AC79 SDK `include_lib/driver/cpu/wl82/asm/WL82.h`. The
names follow `JL_CLOCK`: PWR_CON 0x10000, SYS_DIV 0x10008, CLK_CON0..3
0x1000C..0x10018. `JL_ANA`: PLL_CON0/1 0x119A0/4, PLL2_CON0/1 0x119A8/C. The
bit fields are not in the SDK: its clock driver is the closed `cpu.a`.

The decode, from X0X's research note [R] and the same formula in upstream's
`clock.rs` [V]:

| Step | Formula | Value at handoff |
|---|---|---|
| Reference | 24 MHz when `PLL_CON0 & 0x0C000000 != 0x08000000` | 24 MHz |
| PLL | 24 / (((PLL2_CON0 >> 2) & 31) + 2) × ((PLL2_CON1 & 0xFFF) + 2) | 24 / 12 × 270 = **540 MHz** |
| System | `CLK_CON2` (0x10014) & 0xF = 6: 2/3 of the PLL (5: 1/2, 7: 1/1) | **360 MHz** |
| HSB | system / (((SYS_DIV >> 16) & 3) + 1) | 180 MHz |
| LSB | HSB / (((SYS_DIV >> 8) & 7) + 1) | 60 MHz (the LCD/timer peripheral clock Felucca assumes) |

Upstream's code calls the 0x10014 selector `clk_con3`. In WL82.h's layout
0x10014 is CLK_CON2. Only the name differs; the address is the same.

## Evidence

1. **X0X (charlesvestal/fm1-x0x)**, `docs/plans/2026-10-04-dual-core-and-clock.md`
   [R]. The registers were read on a real FM-1 running a debug build of X0X,
   which never programs the clock, so they show what the SPL left. PLL 540 MHz,
   system selector 6, so 360 MHz. HSB 180 MHz, LSB 60 MHz.
2. **Upstream emulator (simonjohansson/fm1-emulator)**, `rust-emulator/ATOMIC.md`
   "SPL clock handoff" [R]. A Felucca-based diagnostic captured these values
   on FM-1_982 before peripheral initialization:
   `10008=00010200 1000c=000001c1 10010=00010000 10014=00000006 10018=00000002`
   `119a0=45400203 119a4=3f503026 119a8=0940022b 119ac=0750310c`.
   By the decode above that is 360 MHz. Upstream seeds packaged boots with
   these values and issues one instruction per system-clock cycle.
3. **Stock application.** Booted for 3–4 s of guest time in both this fork and
   upstream's model [M], it leaves CLK_CON2 = 6 and PLL2_CON0/1 unchanged, so
   the CPU stays at 360 MHz. In upstream's model it writes HUSB_COM_CON0
   (0x16A00) = 0x07797EB3. Bits 18 and up of that value equal the feedback
   field 0x7780000 that AL-255 derives for
   `pll_clock_init(24000000, 240000000, 0, 0)`.
4. **AL-255/FM-1-RE**, `docs/io/01-boot.md` §5.1 [R]. `pll_clock_init`
   "programs the PLL SFR block at 0x16A00". WL82.h names that block
   `husb_phy_base` (HUSB_COM_CON0..2, HUSB_PHY0_CON0..2) [V]. So the 240 MHz
   is a USB PHY PLL setting [I], which matches item 3.
5. **Throughput is not the clock** [R, X0X]. A one-instruction loop
   (`if (--r != 0) goto`) runs at 51.43 iterations per µs on hardware: 7 cycles
   per iteration at 360 MHz. A taken branch, or execution from flash, costs far
   more than one cycle. The emulator issues one instruction per cycle, so its
   load figures for branchy or XIP-heavy code are optimistic.

## Counters a firmware can read

- **TIMER4** (24 MHz crystal) is independent of the PLL. It is the reference
  for timing loops.
- **corex2 TL_CKCNT** (C0 at 0x1EEE218/21C, C1 at 0x1EEE238/23C) counts CPU
  clock cycles only while DBG_CON (0x1EEE344) bits 0–2 (core 0) or 8–10
  (core 1) are set. X0X measured that it does not count with DBG_CON 0 [R].
  The emulator models this (`src/perf.rs`): cycles are guest oscillator ticks
  times the CPU clock multiple, and the stall counters hold what is written.

## Our firmware's clock readout (SLOOP `perf/speed-top3` d4e5d0e, in the integration)

`hal/fm1_clock.h` `fm1_cpu_khz()` times 1000 passes of a 34-instruction loop
in `.ram_text` (32 dependent adds, a decrement and a branch) against TIMER4,
takes the shortest of four tries, and assumes one cycle per instruction.

| Model | Result (sloop-int 467478d, `cpu_khz`) |
|---|---|
| This fork, `FM1_CPU_MHZ=24` | 23,995 kHz |
| This fork, `FM1_CPU_MHZ=240` | 240,000 kHz |
| This fork, `FM1_CPU_MHZ=360` | 359,947 kHz |
| Upstream model + PR stack (`test/upstream-plus-prs`), firmware clock from the handoff registers | 359,947 kHz |

[M] in both. The arithmetic and the timer path are correct in both models.

**On hardware it will read low** if the taken branch costs more than one cycle
[I]. With X0X's 7-cycle iteration for a decrement and branch, a pass costs
about 32 + 7 = 39 cycles instead of 34, and the readout would show about
360 × 34 / 39 ≈ 314 MHz. Two ways to make it robust:

- Time two chain lengths (for example 32 and 64 adds) and use the difference.
  That cancels the loop overhead, provided dependent adds issue one per cycle.
- Or decode the PLL registers, which the console's `clk` line already prints:
  `fm1_cpu_khz` could show both values.

## For the emulator

- Run firmwares at `--cpu 360` (or `FM1_CPU_MHZ=360`) for budget work that
  should match the physical clock. Keep the 24 MHz baselines for regression
  counts.
- Instruction counts understate hardware cost (item 5). Calibrate against a
  hardware probe: TIMER4 around a known loop, and TL_CKCNT with DBG_CON set.
