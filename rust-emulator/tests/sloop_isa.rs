// SPDX-License-Identifier: GPL-3.0-only
// Instruction forms first reached by the SLOOP 2.2 application. Each test
// encodes the exact halfwords seen in that image; SLOOP's C source (GPL-3.0)
// says what each one must compute.
use fm1_emu::{bus::Bus, cpu::Cpu, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

#[test]
fn signed_less_equal_packed_conditional_runs_its_arm_at_the_bound() {
    // 0x020035d2 (fx.c gain_next, MUTE_STEP 4096): r3 = |to - a|, then
    // `ifs (r3 <= #0x1000) { r4 = r1 }`, bytes a3 ee 80 0d, 14 16.
    for (distance, taken) in [(4096u32, true), (4097, false), (0xffff_f000, true)] {
        let mut c = cpu(&[0xeea3, 0x0d80, 0x1614, 0x0000]);
        c.r[3] = distance;
        c.r[1] = 0;
        c.r[4] = 0x1234;
        c.step().unwrap();
        if taken {
            assert_eq!(c.pc, XIP + 4, "distance {distance:#x}");
            c.step().unwrap();
            assert_eq!(c.r[4], 0, "distance {distance:#x}");
        } else {
            assert_eq!(c.pc, XIP + 6, "distance {distance:#x}");
            assert_eq!(c.r[4], 0x1234);
        }
    }
}

#[test]
fn punch_filter_sweeps_floor_at_their_packed_bounds() {
    // 0x02006ee0 / 0x02006f04 (punch.c): cut = cut > (34 << 8) ? cut - 96 :
    // 34 << 8, and the HPF one at 88 << 8. The packed immediates 0d08 and
    // 0cb0 decode to 0x2200 and 0x5800, the values the arms then load.
    for (words, bound) in [([0xeea2, 0x0d08, 0xe041, 0x2200], 0x2200u32), ([0xeea2, 0x0cb0, 0xe041, 0x5800], 0x5800)] {
        let mut c = cpu(&words);
        c.r[2] = bound;
        c.r[1] = bound - 96;
        c.step().unwrap();
        c.step().unwrap();
        assert_eq!(c.r[1], bound);
        let mut c = cpu(&words);
        c.r[2] = bound + 1;
        c.r[1] = 7;
        c.step().unwrap();
        assert_eq!(c.pc, XIP + 8);
        assert_eq!(c.r[1], 7);
    }
}
