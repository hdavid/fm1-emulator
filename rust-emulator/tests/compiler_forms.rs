// SPDX-License-Identifier: GPL-3.0-only
// Compiler forms reached by Felucca, Jangada and SLOOP (GPL sources) and by the
// stock and Baud Girl images. Encodings and operand texts are the vendor
// objdump's (JieLi clang toolchain, objdump -mattr=+fprev1).
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

// Signed offsets and post/pre-increment forms of byte, halfword, word and pair accesses.

#[test]
fn byte_load_sign_and_store_variants_share_the_negative_offset() {
    let mut c = cpu(&[0xee55, 0x2e3e]); // r2 = b[r3 + -18] (s)
    c.r[3] = RAM + 0x40;
    c.bus.write(RAM + 0x40 - 18, 0x80, 1).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[2], 0xffff_ff80);
    let mut c = cpu(&[0xee53, 0x2e3e]); // b[r3 + -18] = r2
    c.r[3] = RAM + 0x40;
    c.r[2] = 0x1234_5677;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x40 - 18, 1).unwrap(), 0x77);
    assert_eq!(c.r[3], RAM + 0x40);
    let mut c = cpu(&[0xee59, 0x2e3e]); // r2 = b[++r3 = -18]
    c.r[3] = RAM + 0x40;
    c.bus.write(RAM + 0x40 - 18, 0x42, 1).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[2], 0x42);
    assert_eq!(c.r[3], RAM + 0x40 - 18);
}

#[test]
fn signed_byte_preincrement_immediate_writes_the_address_back() {
    // Stock 0x020015ee: ee5c 2051, r2 = b[++r5=1] (s); ee5d 2051 is
    // r2 = b[++r5=-255] (s).
    let mut c = cpu(&[0xee5c, 0x2051, 0xee5d, 0x2051]);
    c.r[5] = RAM + 0x100;
    c.bus.write(RAM + 0x101, 0x80, 1).unwrap();
    c.bus.write(RAM + 0x2, 0x7f, 1).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[5]), (0xffff_ff80, RAM + 0x101));
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[5]), (0x7f, RAM + 2));
}

#[test]
fn byte_postincrement_immediates_are_signed_nine_bit() {
    // eed0-eed5: offset (h & 1) : x[11:8] : x[3:0], signed. FM-1_093
    // 0x020a7de6: eed1 3f28 r3 = b[r2++=-8] (u); eed3 1230 b[r3++=-224] = r1;
    // eed5 3f28 r3 = b[r2++=-8] (s).
    let mut c = cpu(&[0xeed1, 0x3f28, 0xeed3, 0x1230, 0xeed5, 0x3f28]);
    c.r[2] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0xf0, 1).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[3], c.r[2]), (0xf0, RAM + 0x100 - 8));
    c.r[1] = 0x1234_5678;
    c.r[3] = RAM + 0x200;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x200, 1).unwrap(), 0x78);
    assert_eq!(c.r[3], RAM + 0x200 - 224);
    c.r[2] = RAM + 0x100;
    c.step().unwrap();
    assert_eq!((c.r[3], c.r[2]), (0xffff_fff0, RAM + 0x100 - 8));
}

#[test]
fn halfword_offset_forms_take_a_signed_ten_bit_offset() {
    // JieLi objdump: ed5b cf2a = r12 = h[++r2=-6] (u) (Baud Girl glyph
    // blender 0x02012788); ed59 0f2a = r0 = h[++r2=506] (u);
    // ed52 0f2a = r0 = h[r2+-262] (u); ed53 0f2b = h[r2+-6] = r0;
    // ed55 0f2b = h[r2+506] = r0.h (bit 2 stores the upper halfword).
    let mut c = cpu(&[
        0xed5b, 0xcf2a, 0xed59, 0x0f2a, 0xed52, 0x0f2a, 0xed53, 0x0f2b, 0xed55, 0x0f2b,
    ]);
    c.r[2] = RAM + 0x400;
    c.bus.write(RAM + 0x400 - 6, 0xbeef, 2).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[12], 0xbeef);
    assert_eq!(c.r[2], RAM + 0x400 - 6);
    c.r[2] = RAM + 0x400;
    c.bus.write(RAM + 0x400 + 506, 0x1234, 2).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[0], c.r[2]), (0x1234, RAM + 0x400 + 506));
    c.r[2] = RAM + 0x400;
    c.bus.write(RAM + 0x400 - 262, 0x5678, 2).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[0], c.r[2]), (0x5678, RAM + 0x400));
    c.r[0] = 0xaaaa_5555;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x400 - 6, 2).unwrap(), 0x5555);
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x400 + 506, 2).unwrap(), 0xaaaa);
    assert_eq!(c.r[2], RAM + 0x400);
}

#[test]
fn halfword_postincrement_immediates_are_signed_ten_bit() {
    // edd0-edd7: offset (h & 3) : x[11:8] : x[3:1], signed; h bit 2 is a
    // signed load or a store of the high half. Stock 0x0204068c: edd3 3f0d
    // h[r0++=-4] = r3; edd4 1231 h[r3++=32] = r1.h; edd6 1232
    // r1 = h[r3++=-478] (s).
    let mut c = cpu(&[0xedd3, 0x3f0d, 0xedd4, 0x1231, 0xedd6, 0x1232]);
    c.r[0] = RAM + 0x100;
    c.r[3] = 0x1234_5678;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100, 2).unwrap(), 0x5678);
    assert_eq!(c.r[0], RAM + 0x100 - 4);
    c.r[1] = 0xbeef_cafe;
    c.r[3] = RAM + 0x200;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x200, 2).unwrap(), 0xbeef);
    assert_eq!(c.r[3], RAM + 0x200 + 32);
    c.r[3] = RAM + 0x400;
    c.bus.write(RAM + 0x400, 0xfedc, 2).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[1], c.r[3]), (0xffff_fedc, RAM + 0x400 - 478));
}

#[test]
fn halfword_register_preincrement_stores_the_low_or_high_half() {
    // Vendor objdump: eddc 4651 is h[++r5=r6] = r4, eddc 4653 is h[++r5=r6] = r4.h
    // (SLOOP's sequencer event ring, PC 0x02002e86 of sloop-plus).
    for (x, expected) in [(0x4651u16, 0x5678u32), (0x4653, 0x1234)] {
        let mut c = cpu(&[0xeddc, x]);
        c.r[5] = 6;
        c.r[6] = RAM;
        c.r[4] = 0x1234_5678;
        c.bus.write(RAM + 4, 0xaaaa_aaaa, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[5], RAM + 6);
        assert_eq!(c.bus.read(RAM + 6, 2).unwrap(), expected);
        assert_eq!(c.bus.read(RAM + 4, 2).unwrap(), 0xaaaa); // the neighbour untouched
    }
}

#[test]
fn word_postincrement_immediates_are_signed_eleven_bit() {
    // ecd8-ecdf: offset (h & 7) : x[11:8] : x[3:2], signed; x bit 0 store.
    // Stock 0x02071dae: ecdf 4f00 r4 = [r0++=-16]; 0x020407fc: ecdf 3f29
    // [r2++=-8] = r3; ecda 1238 r1 = [r3++=552].
    let mut c = cpu(&[0xecdf, 0x4f00, 0xecdf, 0x3f29, 0xecda, 0x1238]);
    c.r[0] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x1111_2222, 4).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[4], c.r[0]), (0x1111_2222, RAM + 0x100 - 16));
    c.r[2] = RAM + 0x200;
    c.r[3] = 0xabcd_0123;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x200, 4).unwrap(), 0xabcd_0123);
    assert_eq!(c.r[2], RAM + 0x200 - 8);
    c.r[3] = RAM + 0x300;
    c.bus.write(RAM + 0x300, 77, 4).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[1], c.r[3]), (77, RAM + 0x300 + 552));
}

#[test]
fn register_postincrement_accesses_advance_by_a_register() {
    // x bits 0-1 of ecde/edde/eede: word 2 load, 3 store; halfword 0 load,
    // 1 store, 2 signed load, 3 store of the high half; byte 0 load,
    // 1 store, 2 signed load. Stock 0x0204335a: edde 6120
    // r6 = h[r2++=r1] (u); 0x0200f442: edde 0251 h[r5++=r2] = r0;
    // 0x02021508: eede 2731 b[r3++=r7] = r2.
    let mut c = cpu(&[0xecde, 0x1232, 0xecde, 0x1233]);
    c.r[2] = 12;
    c.r[3] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x5555_aaaa, 4).unwrap();
    c.step().unwrap(); // r1 = [r3++=r2]
    assert_eq!((c.r[1], c.r[3]), (0x5555_aaaa, RAM + 0x10c));
    c.r[1] = 99;
    c.step().unwrap(); // [r3++=r2] = r1
    assert_eq!(c.bus.read(RAM + 0x10c, 4).unwrap(), 99);
    assert_eq!(c.r[3], RAM + 0x118);

    let mut c = cpu(&[
        0xedde, 0x6120, 0xedde, 0x0251, 0xedde, 0x1232, 0xedde, 0x1233,
    ]);
    c.r[1] = 6;
    c.r[2] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x8001, 2).unwrap();
    c.step().unwrap(); // r6 = h[r2++=r1] (u)
    assert_eq!((c.r[6], c.r[2]), (0x8001, RAM + 0x106));
    c.r[0] = 0x1234_5678;
    c.r[2] = 2;
    c.r[5] = RAM + 0x200;
    c.step().unwrap(); // h[r5++=r2] = r0
    assert_eq!(c.bus.read(RAM + 0x200, 2).unwrap(), 0x5678);
    assert_eq!(c.r[5], RAM + 0x202);
    c.r[3] = RAM + 0x100;
    c.r[2] = 4;
    c.step().unwrap(); // r1 = h[r3++=r2] (s)
    assert_eq!((c.r[1], c.r[3]), (0xffff_8001, RAM + 0x104));
    c.r[1] = 0xbeef_cafe;
    c.step().unwrap(); // h[r3++=r2] = r1.h
    assert_eq!(c.bus.read(RAM + 0x104, 2).unwrap(), 0xbeef);
    assert_eq!(c.r[3], RAM + 0x108);

    let mut c = cpu(&[0xeede, 0x2731, 0xeede, 0x1232]);
    c.r[2] = 0x1234_56f0;
    c.r[3] = RAM + 0x100;
    c.r[7] = 3;
    c.step().unwrap(); // b[r3++=r7] = r2
    assert_eq!(c.bus.read(RAM + 0x100, 1).unwrap(), 0xf0);
    assert_eq!(c.r[3], RAM + 0x103);
    c.r[3] = RAM + 0x100;
    c.r[2] = 1;
    c.step().unwrap(); // r1 = b[r3++=r2] (s)
    assert_eq!((c.r[1], c.r[3]), (0xffff_fff0, RAM + 0x101));
}

#[test]
fn pair_store_with_low_bits_three_writes_the_base_back() {
    // Felucca 0.9-beta 0x02010ac0 (TRIO trio_note_on): ec50 2213 stores
    // v->ph[0], v->ph[1] = r2:r3 at [r1 + 0x20] and leaves r1 = v + 0x20;
    // the following stores (ec50 2019 at +8 = ph[2], 6490 at +16 = s[1])
    // and loads (6512 s[2], 6b12 age) only hit their fields from there.
    let mut c = cpu(&[0xec50, 0x2213, 0xec50, 0x2019]);
    c.r[1] = RAM + 0x100;
    c.r[2] = 0;
    c.r[3] = 0x1c00_0000;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x120, 4).unwrap(), 0);
    assert_eq!(c.bus.read(RAM + 0x124, 4).unwrap(), 0x1c00_0000);
    assert_eq!(c.r[1], RAM + 0x120);
    c.r[2] = 0x3800_0000;
    c.r[3] = 0;
    c.bus.write(RAM + 0x12c, 0xffff_ffff, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x128, 4).unwrap(), 0x3800_0000);
    assert_eq!(c.bus.read(RAM + 0x12c, 4).unwrap(), 0);
    assert_eq!(c.r[1], RAM + 0x120);
}

#[test]
fn register_pair_preincrement_by_a_register() {
    // Stock 0x0200940e: ec5c 8012, r9_r8 = d[++r1=r0]; ec5c 8013 stores.
    let mut c = cpu(&[0xec5c, 0x8012, 0xec5c, 0x8013]);
    c.r[0] = 8;
    c.r[1] = RAM + 0x100;
    c.bus.write(RAM + 0x108, 0x1111_1111, 4).unwrap();
    c.bus.write(RAM + 0x10c, 0x2222_2222, 4).unwrap();
    c.step().unwrap();
    assert_eq!(
        (c.r[8], c.r[9], c.r[1]),
        (0x1111_1111, 0x2222_2222, RAM + 0x108)
    );
    c.r[8] = 3;
    c.r[9] = 4;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x110, 4).unwrap(), 3);
    assert_eq!(c.bus.read(RAM + 0x114, 4).unwrap(), 4);
    assert_eq!(c.r[1], RAM + 0x110);
}

#[test]
fn register_pair_postincrement_by_an_immediate() {
    // Felucca 1.0 0x02024c88: ec58 2009, d[r0++=8] = r3_r2 (objdump). The
    // offset is signed(h & 7) << 8 | x[11:8] << 4 | x & 12: ec5f 2f09 is
    // d[r0++=-8]; ec58 2008 the load r3_r2 = d[r0++=8].
    let mut c = cpu(&[0xec58, 0x2009, 0xec5f, 0x2f08, 0xec5c, 0x2001]);
    c.r[0] = RAM + 0x100;
    c.r[2] = 0x1111_1111;
    c.r[3] = 0x2222_2222;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100, 4).unwrap(), 0x1111_1111);
    assert_eq!(c.bus.read(RAM + 0x104, 4).unwrap(), 0x2222_2222);
    assert_eq!(c.r[0], RAM + 0x108);
    c.r[0] = RAM + 0x100;
    c.r[2] = 0;
    c.r[3] = 0;
    c.step().unwrap();
    assert_eq!(
        (c.r[2], c.r[3], c.r[0]),
        (0x1111_1111, 0x2222_2222, RAM + 0xf8)
    );
    c.r[0] = RAM + 0x800;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x800, 4).unwrap(), 0x1111_1111);
    assert_eq!(c.r[0], RAM + 0x400);
}

#[test]
fn register_list_postincrement_advances_the_base_past_the_list() {
    // Stock 0x01c07e12: eb12 f800 {r15-r11} = [r2++]; eb12 00f0
    // {r7-r4} = [r2++]; 0x01c07f18: eb32 00f0 [r2++] = {r7-r4}. Lowest
    // register at the lowest address, as the other list forms; that the base
    // advances by the list's size is read from the `++` (inferred).
    let mut c = cpu(&[0xeb12, 0xf800, 0xeb12, 0x00f0, 0xeb32, 0x00f0]);
    c.r[2] = RAM + 0x40;
    for i in 0..9 {
        c.bus.write(RAM + 0x40 + 4 * i, 0x100 + i, 4).unwrap();
    }
    c.step().unwrap();
    assert_eq!(c.r[11..16], [0x100, 0x101, 0x102, 0x103, 0x104]);
    assert_eq!(c.r[2], RAM + 0x40 + 20);
    c.step().unwrap();
    assert_eq!(c.r[4..8], [0x105, 0x106, 0x107, 0x108]);
    assert_eq!(c.r[2], RAM + 0x40 + 36);
    c.r[4..8].copy_from_slice(&[0xa, 0xb, 0xc, 0xd]);
    c.step().unwrap();
    for (i, value) in [0xa, 0xb, 0xc, 0xd].into_iter().enumerate() {
        assert_eq!(
            c.bus.read(RAM + 0x40 + 36 + 4 * i as u32, 4).unwrap(),
            value
        );
    }
    assert_eq!(c.r[2], RAM + 0x40 + 52);
}

#[test]
fn stack_word_accesses_reach_the_high_registers() {
    // Stock 0x01c075a6: 2709 r9 = [sp+28]; 0x01c07636: 2808 r8 = [sp+32].
    // Bit 3 selects r8-r15: 2889 [sp+32] = r9, 20af [sp+128] = r15.
    let mut c = cpu(&[0x2709, 0x2808, 0x2889, 0x20af]);
    c.sr[14] = RAM + 0x100;
    c.bus.write(RAM + 0x100 + 28, 0x1111_2222, 4).unwrap();
    c.bus.write(RAM + 0x100 + 32, 0x3333_4444, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[9], 0x1111_2222);
    assert_eq!(c.r[1], 0);
    c.step().unwrap();
    assert_eq!(c.r[8], 0x3333_4444);
    c.r[9] = 0x5555_6666;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100 + 32, 4).unwrap(), 0x5555_6666);
    c.r[15] = 0x7777_8888;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100 + 128, 4).unwrap(), 0x7777_8888);
}

#[test]
fn memory_read_modify_write_offsets_are_signed() {
    // Vendor objdump: e868 12fc is [r1+-4] += r2, e868 12fe [r1+-4] -= r2,
    // e86c 16fc [r1+-4] <<= 6 and e866 12fc [r1+-4] |= 1 << r2. SLOOP's
    // ANALOG 2 hard sync updates b[i - 1] with the first form; read as
    // +252 it wrote over the caller's frame.
    for (words, expected) in [
        ([0xe868u16, 0x12fc], 105u32),
        ([0xe868, 0x12fe], 95),
        ([0xe86c, 0x16fc], 100 << 6),
        ([0xe866, 0x12fc], 100 | 1 << 5),
    ] {
        let mut c = cpu(&words);
        c.r[1] = RAM + 0x100;
        c.r[2] = 5;
        c.bus.write(RAM + 0xfc, 100, 4).unwrap();
        c.bus.write(RAM + 0x1fc, 7, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.bus.read(RAM + 0xfc, 4).unwrap(), expected, "{words:04x?}");
        assert_eq!(c.bus.read(RAM + 0x1fc, 4).unwrap(), 7);
    }
}

#[test]
fn memory_mask_offsets_are_signed_and_xor_is_an_operation() {
    // ef00-efff: bits 7-6 or / xor / and / and-not, bits 5-0 a signed word
    // offset. Stock 0x0208110e: ef3f 0400, [r0+-4] |= 0x80000000.
    for (h, expected) in [
        (0xef3f, 0x8000_1234u32),
        (0xef7f, 0x8000_1234),
        (0xefbf, 0),
        (0xefff, 0x1234),
    ] {
        let mut c = cpu(&[h, 0x0400]);
        c.r[0] = RAM + 0x100;
        c.bus.write(RAM + 0xfc, 0x1234, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.bus.read(RAM + 0xfc, 4).unwrap(), expected, "{h:04x}");
    }
    let mut c = cpu(&[0xef60, 0x0c41]); // [r0+-128] ^= 0xC100
    c.r[0] = RAM + 0x100;
    c.bus.write(RAM + 0x80, 0xff00, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x80, 4).unwrap(), 0x3e00);
}

// Compare-branches and conditional blocks.

#[test]
fn unsigned_compare_branches_take_an_unsigned_ten_bit_immediate() {
    // SLOOP 2.2 fm1_delay_us at 0x02017002: f9f1 81fc = jb r1, #960, pc - 4
    // (loop while elapsed TIMER4 ticks < 960). Sign-extending made it
    // r1 < 0xffffffc0: a wait that never ended.
    for (elapsed, taken) in [(959u32, true), (960, false), (49_799_320, false)] {
        let mut c = cpu(&[0xf9f1, 0x81fc]);
        c.r[1] = elapsed;
        c.step().unwrap();
        assert_eq!(
            c.pc,
            if taken { XIP - 4 } else { XIP + 4 },
            "r1 = {elapsed}"
        );
    }
}

#[test]
fn float_compare_branches_cover_the_equality_and_unsigned_rows() {
    // Vendor objdump with x bit 11: e801 2804 is iff (r2 == r1) goto 8, and
    // e881 / e901 / e981 / ec01 / ec81 are u!=, u>=, <, u> and <= (the
    // ED00-EE80 rows are already measured). Negative operands show the float
    // ordering, which integer order would invert. Unordered operands still
    // fault, as for the measured forms.
    for (h, lhs, rhs, taken) in [
        (0xe801u16, -0.0f32, 0.0f32, true),
        (0xe801, 1.0, 2.0, false),
        (0xe881, 1.0, 2.0, true),
        (0xe881, -0.0, 0.0, false),
        (0xe901, -1.0, -2.0, true),
        (0xe901, -2.0, -1.0, false),
        (0xe981, -2.0, -1.0, true),
        (0xe981, -1.0, -1.0, false),
        (0xec01, -1.0, -2.0, true),
        (0xec01, -1.0, -1.0, false),
        (0xec81, -1.0, -1.0, true),
        (0xec81, -1.0, -2.0, false),
    ] {
        let mut c = cpu(&[h, 0x2804]);
        c.r[2] = lhs.to_bits();
        c.r[1] = rhs.to_bits();
        c.step().unwrap();
        assert_eq!(
            c.pc,
            if taken { XIP + 4 + 8 } else { XIP + 4 },
            "{h:04x} {lhs} {rhs}"
        );
    }
    let mut c = cpu(&[0xe801, 0x2804]);
    c.r[2] = f32::NAN.to_bits();
    c.r[1] = 1.0f32.to_bits();
    assert!(c.step().is_err());
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

/// Where a one-instruction conditional block `[h, x, r1 = 1]` continues.
fn conditional_target(h: u16, x: u16, register: usize, value: u32) -> u32 {
    let mut c = cpu(&[h, x, 0x2141, 0x2140]);
    c.r[register] = value;
    c.step().unwrap();
    c.pc - XIP
}

#[test]
fn conditional_blocks_compare_with_packed_immediates() {
    // ota_session (Felucca/Jangada/SLOOP): ec23 0ba0 `if (r3 > 81920) {`.
    for (value, enters) in [(81921, true), (81920, false), (0xffff_ffff, true)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xec23, 0x0ba0, 3, value), expected);
    }
    // e9a3 0ba0 `if (r3 < 81920) {` (unsigned).
    for (value, enters) in [(81919, true), (81920, false), (0xffff_ffff, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xe9a3, 0x0ba0, 3, value), expected);
    }
    // FM-1_093 0x020a3008: ee21 0e5e `ifs (r1 > 3552) {` (signed).
    for (value, enters) in [(3553, true), (3552, false), (-5i32 as u32, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xee21, 0x0e5e, 1, value), expected);
    }
}

#[test]
fn conditional_blocks_skip_six_byte_compare_branches_whole() {
    // e820 0001 `if (r0 == 1) {` around ff00 0000 0002
    // `if (r0 == 0) goto 4` (6 bytes in objdump), then r1 = 1; r0 = 1.
    let mut c = cpu(&[0xe820, 0x0001, 0xff00, 0x0000, 0x0002, 0x2141, 0x2140]);
    c.r[0] = 5;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 10);
    c.step().unwrap();
    assert_eq!(c.r[1], 1);
}

/// Where a one-instruction register conditional block `[h, x, r1 = 1]`
/// continues with rN (h & 15) = `lhs` and rC (x >> 8 & 15) = `rhs`: 4 when
/// it enters the block, 6 when it skips it.
fn register_conditional_target(h: u16, x: u16, lhs: u32, rhs: u32) -> u32 {
    let mut c = cpu(&[h, x, 0x2141, 0x2140]);
    c.r[((x >> 8) & 15) as usize] = rhs;
    c.r[(h & 15) as usize] = lhs;
    c.step().unwrap();
    c.pc - XIP
}

#[test]
fn register_conditional_blocks_with_x_bit_7_compare_floats() {
    // Vendor objdump -mattr=+fprev1: register conditional blocks whose x has
    // bit 7 set (bit 6 clear) are `iff`, IEEE single compares, in every row
    // (the ED1X-EE9X rows are already measured). Stock RAM code: ed11 0080
    // iff (r1 >= r0) {, ed92 8b80 iff (r2 u< r11) {. Unordered operands
    // fault, as for the measured rows.
    let f = |v: f32| v.to_bits();
    let cases: [(u16, f32, f32, bool); 10] = [
        // (h with rN = r3, lhs, rhs, enters)
        (0xe813, -0.0, 0.0, true),  // iff (r3 == r0)
        (0xe893, 1.0, 1.0, false),  // iff (r3 u!= r0)
        (0xe913, -1.0, -2.0, true), // iff (r3 u>= r0)
        (0xe993, -2.0, -1.0, true), // iff (r3 < r0)
        (0xec13, -1.0, -2.0, true), // iff (r3 u> r0)
        (0xec93, -2.0, -2.0, true), // iff (r3 <= r0)
        (0xed13, -1.0, -2.0, true), // iff (r3 >= r0)
        (0xed93, -2.0, -1.0, true), // iff (r3 u< r0)
        (0xee13, -1.0, -2.0, true), // iff (r3 > r0)
        (0xee93, -2.0, -1.0, true), // iff (r3 u<= r0)
    ];
    for (h, lhs, rhs, enters) in cases {
        let target = if enters { 4 } else { 6 };
        assert_eq!(
            register_conditional_target(h, 0x0080, f(lhs), f(rhs)),
            target,
            "{h:04x} {lhs} {rhs}"
        );
        let mut c = cpu(&[h, 0x0080, 0x2141, 0x2140]);
        c.r[3] = f32::NAN.to_bits();
        assert!(c.step().is_err(), "{h:04x} NaN");
    }
    // The low bits of x do not matter (objdump: 0081, 0090 and 00a0 too).
    assert_eq!(
        register_conditional_target(0xed13, 0x0090, f(-1.0), f(-2.0)),
        4
    );
    // ed92 8b80: iff (r2 u< r11) { compares r2 with r11.
    assert_eq!(
        register_conditional_target(0xed92, 0x0b80, f(-3.0), f(-1.0)),
        4
    );
    assert_eq!(
        register_conditional_target(0xed92, 0x0b80, f(1.0), f(-1.0)),
        6
    );
}

#[test]
fn register_bit_test_block_with_x_low_bits_is_a_float_not_equal() {
    // objdump -mattr=+fprev1, ea13 x: 0000 if ((r3 & r0) == 0) {,
    // 0080 if ((r3 & r0) != 0) {, 0081-00bf iff (r3 != r0) { (ordered).
    let f = |v: f32| v.to_bits();
    assert_eq!(register_conditional_target(0xea13, 0x0000, 2, 1), 4);
    assert_eq!(register_conditional_target(0xea13, 0x0080, 3, 1), 4);
    assert_eq!(register_conditional_target(0xea13, 0x0080, 2, 1), 6);
    assert_eq!(
        register_conditional_target(0xea13, 0x0081, f(1.0), f(2.0)),
        4
    );
    assert_eq!(
        register_conditional_target(0xea13, 0x0081, f(-0.0), f(0.0)),
        6
    );
}

#[test]
fn register_conditional_blocks_with_x_bits_7_and_6_are_unsupported() {
    // objdump (with or without the FPU): e813 00c0 is <unknown instruction>.
    let mut c = cpu(&[0xe813, 0x00c0, 0x2141, 0x2140]);
    assert!(c.step().is_err());
}
