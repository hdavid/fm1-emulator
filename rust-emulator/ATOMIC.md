# FM-1 TESTSET measurement

On 2026-10-04, `tools/build_atomic_probe.py` built diagnostic FM-1_982 with
the existing USB updater, watchdog service and original instruction probe.
The physical FM-1 returned these values through its USB CDC endpoint:

| Initial byte | PSR low nibble | Resulting byte | IFEQ taken |
| --- | --- | --- | --- |
| 00 | 0 | ff | no |
| 01 | 1 | ff | no |
| 80 | 0 | ff | no |
| ff | f | ff | yes |
| 02 | 2 | ff | no |
| 04 | 4 | ff | yes |
| 08 | 8 | ff | no |
| 0f | f | ff | yes |
| 7f | f | ff | yes |
| fe | e | ff | yes |
| f0 | 0 | ff | no |
| 10 | 0 | ff | no |

For the measured `testset b[r1]` encoding, the old byte supplies PSR bits 0–3
and the destination becomes ff. IFEQ tests bit 2. The emulator preserves the
other PSR bits; these measurements do not establish their hardware behavior.
The runtime regression checks all twelve rows and adjacent-byte preservation.
Stock firmware's spinlock at 01c00fbe now acquires its initially clear lock.

Local captures and build outputs are ignored under `.deps/firmware-trial/atomic/`.
The original 114-byte instruction probe still passed hardware comparison after
each installation. Firmware packages and device-specific captures are not
redistributed in the repository.

## SPL clock handoff

The same compatible diagnostic captures the WL82 clock/PLL and P33 bridge
registers before its application peripheral initialization. The `clocks`
command returned:

```text
10000=00000000 10008=00010200 1000c=000001c1
10010=00010000 10014=00000006 10018=00000002
119a0=45400203 119a4=3f503026 119a8=0940022b 119ac=0750310c
13e00=00000100 13e04=000000e0
```

These seed the packaged application's clock configuration. The diagnostic
uses Felucca startup before capture; this is evidence for its SPL handoff,
not a direct capture of the stock application. The model retains documented
clock/PLL and USB common PHY configuration registers. Analog lock timing,
dynamic CPU frequency and high-speed USB traffic are not modeled.

Expanded capture: SFC CON=809803b5, BAUD=1, CODE=8e17, BASEADR=4000;
SFCENC CON=1, KEY=0 and all four window-bound registers zero. HUSB common
PHY configuration is 0, 008881c3, 0. KEY=0 is the physical register value;
the package's chip key still supplies application decoding. Dynamic key
and encrypted-window changes fault explicitly. SFC base remapping selects
the corresponding physical flash bytes rather than moving the application.
CON bit 31 is the busy status seen during the hardware capture. Functional
emulator reads report idle after completed accesses and retain 009803b5.
