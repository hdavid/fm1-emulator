// SPDX-License-Identifier: GPL-3.0-only
// Six-byte compare-branches (ff00-ff7f): `if (rD op imm12/packed/rC) goto`.
// JieLi objdump -mattr=+fprev1:
//   ff42 7300 0172 = if (r7 >= r3) goto ...      (unsigned integer)
//   ff42 7380 0172 = iff (r7 u>= r3) goto ...    (x bit 7: IEEE single)
//   ff40..ff4d with x bit 7: iff ==, u!=, u>=, <, u>, <=, >=, u<, >, u<=
//   ff00 0fff = if (r0 == -1); ff0a 0fff = ifs (r0 >= -1);
//   ff02 0fff = if (r0 >= 4095); ff0d 0800 = ifs (r0 <= -2048).
// X0X 0.9's master limiter (master_process, 0x0201ffa2) skips its negative
// soft clip with ff42 7380 0172 when s >= -0.89f: read as an unsigned
// integer compare, +0.0 < 0xbf63d70a and every silent sample became -0.78.
use fm1_emu::{bus::Bus, cpu::Cpu, XIP};

fn cpu(words: &[u16]) -> Cpu {
    let mut image = words.to_vec();
    image.resize(1024, 0);
    Cpu::new(
        Bus::new(image.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// Whether the compare-branch `words` (offset +16 bytes) is taken with
/// r7 = lhs and r3 = rhs (or r0 = lhs for the immediate forms).
fn taken(words: [u16; 2], lhs: u32, rhs: u32) -> bool {
    let mut c = cpu(&[words[0], words[1], 0x0008]);
    c.r[7] = lhs;
    c.r[0] = lhs;
    c.r[3] = rhs;
    c.step().unwrap();
    match c.pc - XIP {
        22 => true,
        6 => false,
        other => panic!("unexpected PC offset {other}"),
    }
}

#[test]
fn x0x_limiter_skips_the_clip_for_silence() {
    let iff_uge = [0xff42, 0x7380];
    assert!(taken(iff_uge, f(0.0), f(-0.89)));
    assert!(taken(iff_uge, f(-0.5), f(-0.89)));
    assert!(!taken(iff_uge, f(-1.0), f(-0.89)));
    // The same word without x bit 7 is the unsigned integer compare.
    assert!(!taken([0xff42, 0x7300], f(0.0), f(-0.89)));
}

#[test]
fn float_register_compare_branches_follow_the_objdump_order() {
    // (h low nibble, then: a<b, a==b, a>b, unordered)
    let rows: [(u16, [bool; 4]); 10] = [
        (0x0, [false, true, false, false]), // ==
        (0x1, [true, false, true, true]),   // u!=
        (0x2, [false, true, true, true]),   // u>=
        (0x3, [true, false, false, false]), // <
        (0x8, [false, false, true, true]),  // u>
        (0x9, [true, true, false, false]),  // <=
        (0xa, [false, true, true, false]),  // >=
        (0xb, [true, false, false, true]),  // u<
        (0xc, [false, false, true, false]), // >
        (0xd, [true, true, false, true]),   // u<=
    ];
    for (nibble, expected) in rows {
        let words = [0xff40 | nibble, 0x7380];
        let cases = [(f(-2.0), f(1.5)), (f(-0.0), f(0.0)), (f(3.0), f(-3.0))];
        // The objdump's unordered column (u prefix) is unmeasured on
        // hardware: a NaN operand faults (upstream's policy) instead.
        let mut c = cpu(&[words[0], words[1], 0x0008]);
        c.r[7] = f32::NAN.to_bits();
        c.r[3] = f(1.0);
        assert!(c.step().is_err(), "ff4{nibble:x} 7380 with NaN");
        for ((lhs, rhs), want) in cases.into_iter().zip(expected) {
            assert_eq!(
                taken(words, lhs, rhs),
                want,
                "ff4{nibble:x} 7380 with {} vs {}",
                f32::from_bits(lhs),
                f32::from_bits(rhs)
            );
        }
    }
}

#[test]
fn equality_and_signed_immediates_are_sign_extended() {
    assert!(taken([0xff00, 0x0fff], u32::MAX, 0)); // == -1
    assert!(!taken([0xff00, 0x0fff], 4095, 0));
    assert!(taken([0xff01, 0x0801], 0, 0)); // != -2047
    assert!(!taken([0xff01, 0x0801], (-2047i32) as u32, 0));
    assert!(taken([0xff0a, 0x0fff], 0, 0)); // ifs >= -1
    assert!(!taken([0xff0a, 0x0fff], (-2i32) as u32, 0));
    assert!(taken([0xff0d, 0x0800], (-2048i32) as u32, 0)); // ifs <= -2048
    assert!(!taken([0xff0d, 0x0800], (-2047i32) as u32, 0));
}

#[test]
fn unsigned_immediates_keep_twelve_bits() {
    assert!(taken([0xff02, 0x0fff], 4095, 0)); // >= 4095
    assert!(!taken([0xff02, 0x0fff], 4094, 0));
    assert!(taken([0xff03, 0x0fff], 4094, 0)); // < 4095
}
