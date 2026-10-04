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
