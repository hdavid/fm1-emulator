// SPDX-License-Identifier: GPL-3.0-only
// Instruction forms first reached by the SLOOP 2.2 application. Each test
// encodes the exact halfwords seen in that image; SLOOP's C source (GPL-3.0)
// says what each one must compute.
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

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
    for (words, bound) in [
        ([0xeea2, 0x0d08, 0xe041, 0x2200], 0x2200u32),
        ([0xeea2, 0x0cb0, 0xe041, 0x5800], 0x5800),
    ] {
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

#[test]
fn pop_special_registers_restores_rets_before_a_tail_goto() {
    // 0x020108cc: `pop {rets}` then `goto` (a tail call), bytes 88 04,
    // ff ea 53 c4. The 0x048X bitmap is {reti, rete, retx, rets} from bit 0
    // (stock FM-1 0x02000628 ends an interrupt with pop {reti}; rti).
    let mut c = cpu(&[0x0488, 0x0000]);
    c.sr[14] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x0200_1234, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.sr[3], 0x0200_1234);
    assert_eq!(c.sr[14], RAM + 0x104);
    assert_eq!(c.pc, XIP + 2);
    let mut c = cpu(&[0x0481, 0x0000]);
    c.sr[14] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x0200_5678, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.sr[0], 0x0200_5678);
    assert_eq!(c.sr[14], RAM + 0x104);
}

#[test]
fn three_operand_logic_mode_three_clears_the_second_operands_bits() {
    // 0x0200f17c (ui_input.c): pressed with the seven layer buttons masked
    // out, r12 = r3 & ~r1, bytes 90 e1 33 c1. Read as `~r1` it turned no
    // press into 0xfffff133 and REC (bit 13) toggled by itself.
    let mut c = cpu(&[0xe190, 0xc133]);
    c.r[3] = 0;
    c.r[1] = 0xecc;
    c.step().unwrap();
    assert_eq!(c.r[12], 0);
    let mut c = cpu(&[0xe190, 0xc133]);
    c.r[3] = 0x2000 | 0x40;
    c.r[1] = 0xecc;
    c.step().unwrap();
    assert_eq!(c.r[12], 0x2000);
    // Stock FM-1 0x02018718: r1 = 1; r0 = r1 & ~r0 (a C `!flag` of a 0/1
    // flag), bytes 41 21, 90 e1 13 00.
    for (flag, inverted) in [(0u32, 1u32), (1, 0)] {
        let mut c = cpu(&[0x2141, 0xe190, 0x0013]);
        c.r[0] = flag;
        c.step().unwrap();
        c.step().unwrap();
        assert_eq!(c.r[0], inverted);
    }
}

#[test]
fn unsigned_less_equal_conditional_takes_a_plain_twelve_bit_immediate() {
    // 0x02006ada (fx.c fx_buses): r1 = (u16) r0, r7 = 0; `if (r1 <= #2790)
    // { r7 = r0 }`,
    // i.e. `if (++fx.line_i[3] >= REV_LINE[3]) fx.line_i[3] = 0` with
    // REV_LINE[3] = 2791. Bytes b1 ec e6 0a, 07 16. The immediate is
    // x & 0xfff = 0xae6, neither packed (0x7300) nor sign-extended.
    for (index, kept) in [(2790u32, true), (2791, false), (0, true), (0x3b0c, false)] {
        let mut c = cpu(&[0xecb1, 0x0ae6, 0x1607, 0x0000]);
        c.r[0] = index;
        c.r[1] = index;
        c.r[7] = 0;
        c.step().unwrap();
        if kept {
            c.step().unwrap();
            assert_eq!(c.r[7], index);
        } else {
            assert_eq!(c.pc, XIP + 6, "index {index}");
            assert_eq!(c.r[7], 0);
        }
    }
}
