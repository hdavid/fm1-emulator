// SPDX-License-Identifier: GPL-3.0-only
// Packed 16-bit forms, with the encodings and operand texts of the vendor
// objdump. The semantics are inferred (see src/simd.rs); each case uses
// inputs that tell the alternatives apart.
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

/// One wide instruction on r1 = a, r2 = b, r0 = old; returns r0.
fn simd(h: u16, x: u16, a: u32, b: u32, old: u32) -> u32 {
    let mut c = cpu(&[h, x]);
    c.r[0] = old;
    c.r[1] = a;
    c.r[2] = b;
    c.step().unwrap();
    assert_eq!((c.r[1], c.r[2]), (a, b), "sources unchanged");
    c.r[0]
}

#[test]
fn half_add_and_subtract_wrap_and_keep_the_other_half() {
    // e500 0218: r0.h = r1.l + r2.l; e500 0213: r0.l = r1.l - r2.h
    assert_eq!(
        simd(0xe500, 0x0218, 0x1111_7fff, 0x2222_0001, 0xaaaa_bbbb),
        0x8000_bbbb
    );
    assert_eq!(
        simd(0xe500, 0x0213, 0x0000_0005, 0x0007_0000, 0xaaaa_bbbb),
        0xaaaa_fffe
    );
}

#[test]
fn half_multiply_is_integer_or_q15_with_signed_saturation() {
    // e541 0214: r0.l = r1.h * r2.l (ssat): the product saturated to 16 bits
    assert_eq!(
        simd(0xe541, 0x0214, 0x00c8_0000, 0x0000_0003, 0xaaaa_bbbb),
        0xaaaa_0258
    );
    assert_eq!(simd(0xe541, 0x0214, 0x012c_0000, 0x0000_00c8, 0), 0x7fff);
    assert_eq!(simd(0xe541, 0x0214, 0xfed4_0000, 0x0000_00c8, 0), 0x8000);
    // e543 6654: r6.l = r5.h * r6.l (ssat,x2): (a * b) >> 15, truncated
    let mut c = cpu(&[0xe543, 0x6654]);
    c.r[5] = 0x00ca_1234; // d = 202
    c.r[6] = 0x0000_7fff; // f = 32767
    c.step().unwrap();
    assert_eq!(c.r[6], 0x0000_00c9); // 202 * 32767 >> 15 = 201
                                     // negative products floor; -32768 * -32768 saturates
    assert_eq!(
        simd(0xe543, 0x0218, 0xffff, 0x4001, 0x1234_5678),
        0xffff_5678
    );
    assert_eq!(simd(0xe543, 0x0218, 0x8000, 0x8000, 0), 0x7fff_0000);
}

#[test]
fn half_multiply_into_a_word_keeps_the_full_product() {
    // e551 0212: r0 = r1.l * r2.h (ssat); e553: (ssat,x2)
    assert_eq!(
        simd(0xe551, 0x0212, 0xffff_8000, 0x7fff_0000, 0),
        (-32768i32 * 32767) as u32
    );
    assert_eq!(
        simd(0xe553, 0x0212, 0x0000_c000, 0x4000_0000, 0),
        0xe000_0000
    );
    assert_eq!(
        simd(0xe553, 0x0212, 0x0000_8000, 0x8000_0000, 0),
        0x7fff_ffff
    );
}

#[test]
fn pack_puts_the_first_half_high() {
    // e404 0212: r0 = pack(r1.l, r2.h)
    assert_eq!(
        simd(0xe404, 0x0212, 0x1111_2222, 0x3333_4444, 0),
        0x2222_3333
    );
}

#[test]
fn dual_lanes_take_the_first_element_as_the_high_lane() {
    // e519 0212: r0 = r1.h,r1.l +|+ r2.h,r2.l (ssat); 0216: +|-
    assert_eq!(
        simd(0xe519, 0x0212, 0x7000_0001, 0x2000_fffe, 0),
        0x7fff_ffff
    );
    assert_eq!(
        simd(0xe519, 0x0216, 0x7000_8001, 0x2000_0002, 0),
        0x7fff_8000
    );
    // e569 0212: r0 = r1.h,r1.l *|* r2.h,r2.l (ssat); e56b: (ssat,x2)
    assert_eq!(
        simd(0xe569, 0x0212, 0x0003_0100, 0xfffe_0200, 0),
        0xfffa_7fff
    );
    assert_eq!(
        simd(0xe56b, 0x0212, 0x4000_8000, 0x4000_8000, 0),
        0x2000_7fff
    );
}

#[test]
fn packed_table_sine_kernel_matches_the_c_interpolation() {
    // hal/fm1_simd.h asm_sine_pk body: ix = ph >> 22; w = [tab + ix << 2];
    // f = uextra(ph, p:7, l:15); f.l = w.h * f.l (ssat,x2); w.l += f.l; y = w.l (s)
    let words = [
        0xb694u16, 0xecd8, 0x542a, 0xe1b6, 0x13bc, 0xe543, 0x6654, 0xe500, 0x5650, 0x17d8,
    ];
    let table: Vec<i32> = (0..1024)
        .map(|i| (32767.0 * (2.0 * std::f64::consts::PI * i as f64 / 1024.0).sin()).round() as i32)
        .collect();
    for ph in [
        0u32,
        0x0040_0000,
        0x1234_5678,
        0x8000_0001,
        0xffff_ff80,
        0xc0de_1234,
    ] {
        let mut c = cpu(&words);
        for (i, a) in table.iter().enumerate() {
            let d = table[(i + 1) & 1023] - a;
            c.bus
                .write(
                    RAM + 4 * i as u32,
                    ((d as u32) << 16) | (*a as u32 & 0xffff),
                    4,
                )
                .unwrap();
        }
        c.r[1] = ph;
        c.r[2] = RAM;
        for _ in 0..6 {
            c.step().unwrap();
        }
        let i = (ph >> 22) as usize;
        let (a, b) = (table[i], table[(i + 1) & 1023]);
        let expected = a + (((b - a) * ((ph >> 7) & 0x7fff) as i32) >> 15);
        assert_eq!(c.r[0] as i32, expected, "ph {ph:#x}");
    }
}
