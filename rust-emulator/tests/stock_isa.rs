// SPDX-License-Identifier: GPL-3.0-only
// Instruction forms first reached by the stock FM-1_015 and Baud Girl FM-1_093
// applications. Each test encodes the exact halfwords seen in those images.
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

#[test]
fn byte_load_with_negative_nine_bit_offset_reads_below_the_base() {
    // 0x020035c4: r3 = b[r3 + -18] (u), bytes 51 ee 3e 3e.
    let mut c = cpu(&[0xee51, 0x3e3e]);
    c.r[3] = RAM + 0x40;
    c.bus.write(RAM + 0x40 - 18, 0xa5, 1).unwrap();
    c.bus
        .write(RAM + 0x40 + 0x1ee - 0x200 + 1, 0x11, 1)
        .unwrap();
    c.step().unwrap();
    assert_eq!(c.r[3], 0xa5);
    assert_eq!(c.pc, XIP + 4);
}

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
fn word_postincrement_load_advances_by_the_encoded_immediate() {
    // 0x020347fc: r0 = [r4++=28], bytes d8 ec 4c 01 (device table walk).
    let mut c = cpu(&[0xecd8, 0x014c]);
    c.r[4] = RAM + 0x100;
    c.r[1] = 0x55; // not an index register in this form
    c.bus.write(RAM + 0x100, 0x0203_4b86, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[0], 0x0203_4b86);
    assert_eq!(c.r[4], RAM + 0x100 + 28);
    assert_eq!(c.pc, XIP + 4);
}

#[test]
fn word_indexed_load_and_store_keep_their_register_index() {
    let mut c = cpu(&[0xecd8, 0x214a, 0xecd8, 0x214b]); // r2=[r4+r1<<2]; [r4+r1<<2]=r2
    c.r[4] = RAM + 0x100;
    c.r[1] = 3;
    c.bus.write(RAM + 0x10c, 0xdead_beef, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[2], 0xdead_beef);
    assert_eq!(c.r[4], RAM + 0x100);
    c.r[2] = 0x1234_5678;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x10c, 4).unwrap(), 0x1234_5678);
}

#[test]
fn long_multiply_low_partial_product_is_unsigned_by_default() {
    // 0x020021da: r1:r0 = r0 * r1, bytes f8 e1 10 00.
    let mut c = cpu(&[0xe1f8, 0x0010]);
    c.r[0] = 0xffff_fffe;
    c.r[1] = 3;
    c.step().unwrap();
    assert_eq!((c.r[0], c.r[1]), (0xffff_fffa, 2));
    let mut c = cpu(&[0xe1f8, 0x1010]); // bit 12: signed
    c.r[0] = 0xffff_fffe; // -2
    c.r[1] = 3;
    c.step().unwrap();
    assert_eq!((c.r[0], c.r[1]), (0xffff_fffa, 0xffff_ffff));
    let mut c = cpu(&[0xe1f8, 0x6420]); // r7:r6 = r2 * r4
    c.r[2] = 0x1234_5678;
    c.r[4] = 1_000_000;
    c.step().unwrap();
    let product = 0x1234_5678u64 * 1_000_000;
    assert_eq!((c.r[6], c.r[7]), (product as u32, (product >> 32) as u32));
}

#[test]
fn long_divide_and_carry_chain_rebuild_a_64_bit_remainder() {
    // Stock sequence at 0x020021e4..0x020021f8 (microsecond conversion):
    // r3:r2 = r1:r0 / r4; r5 = r3*r4; r7:r6 = r2*r4; r7 += r5;
    // r4 = r0 - r6; r5 = r1 - r7 - !C.
    let n: u64 = 0x0000_0123_8765_4321;
    let mut c = cpu(&[
        0xe1f6, 0x2400, 0xe1f0, 0x5430, 0xe1f8, 0x6420, 0x1857, 0x1f84, 0xe0b8, 0x5712,
    ]);
    c.r[0] = n as u32;
    c.r[1] = (n >> 32) as u32;
    c.r[4] = 1_000_000;
    for _ in 0..6 {
        c.step().unwrap();
    }
    assert_eq!(c.r[2] as u64 | (c.r[3] as u64) << 32, n / 1_000_000);
    assert_eq!(c.r[4] as u64 | (c.r[5] as u64) << 32, n % 1_000_000);
}

#[test]
fn add_with_carry_propagates_the_low_word_carry() {
    let mut c = cpu(&[0x1c10, 0xe0b8, 0x5230]); // r0 = r1 + r0 ; r5 = r3 + r2 + C
    c.r[0] = 0xffff_ffff;
    c.r[1] = 2;
    c.r[2] = 7;
    c.r[3] = 1;
    c.step().unwrap();
    c.step().unwrap();
    assert_eq!((c.r[0], c.r[5]), (1, 9));
}

#[test]
fn stack_adjust_by_signed_thirteen_bit_immediate_allocates_and_frees_frames() {
    // Stock FM-1_015 0x0200c97c prologue: push, then e8f0 1d98 (sp -= 616);
    // its epilogues e8f0 0268 (sp += 616), then pop. Jangada 0x0200de16:
    // e8f0 1cc8 (sp -= 824).
    for (operand, delta) in [(0x1d98u16, -616i32), (0x0268, 616), (0x1cc8, -824)] {
        let mut c = cpu(&[0xe8f0, operand]);
        c.sr[14] = RAM + 0x4000;
        let before = c.r;
        c.step().unwrap();
        assert_eq!(c.sr[14], (RAM + 0x4000).wrapping_add(delta as u32));
        assert_eq!(c.r, before);
        assert_eq!(c.pc, XIP + 4);
    }
}
