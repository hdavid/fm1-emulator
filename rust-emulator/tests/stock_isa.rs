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

#[test]
fn unsigned_compare_branches_take_an_unsigned_ten_bit_immediate() {
    // SLOOP 2.2 fm1_delay_us at 0x02017002: f9f1 81fc = jb r1, #960, pc - 4
    // (loop while elapsed TIMER4 ticks < 960). Sign-extending made it
    // r1 < 0xffffffc0: a wait that never ended.
    for (elapsed, taken) in [(959u32, true), (960, false), (49_799_320, false)] {
        let mut c = cpu(&[0xf9f1, 0x81fc]);
        c.r[1] = elapsed;
        c.step().unwrap();
        assert_eq!(c.pc, if taken { XIP - 4 } else { XIP + 4 }, "r1 = {elapsed}");
    }
}

#[test]
fn equality_compare_branches_sign_extend_their_immediate() {
    // Baud Girl FM-1_093 0x02001820: f874 fc04 = je r?, #-2, pc + 8 with the
    // register holding 0xfffffffe; an unsigned imm10 (1022) sent the boot
    // down a path that read address 0.
    let mut c = cpu(&[0xf874, 0xfc04]);
    let n = (0xf874 & 15) as usize;
    c.r[n] = 0xffff_fffe;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 4 + 8);
}

#[test]
fn register_shift_covers_both_left_forms_and_arithmetic_right() {
    // Felucca 0.9-beta 0x02011b36 (a synth engine's DSP): e1c8 2253 =
    // r2 = r5 >>> r2 (arithmetic). Quarkslab pi32v2: imm1619 0 and 1 lsl,
    // 2 lsr, 3 asr.
    let mut c = cpu(&[0xe1c8, 0x2253]);
    c.r[5] = 0xffff_f000; // -4096
    c.r[2] = 4;
    c.step().unwrap();
    assert_eq!(c.r[2], 0xffff_ff00); // -256
    let mut c = cpu(&[0xe1c8, 0x2251]); // r2 = r5 << r2
    c.r[5] = 3;
    c.r[2] = 4;
    c.step().unwrap();
    assert_eq!(c.r[2], 48);
    let mut c = cpu(&[0xe1c8, 0x2253]); // shifts past 31 keep the sign
    c.r[5] = 0x8000_0000;
    c.r[2] = 40;
    c.step().unwrap();
    assert_eq!(c.r[2], 0xffff_ffff);
}

#[test]
fn register_list_load_puts_the_lowest_register_at_the_base() {
    // Felucca 0.9-beta 0x02011fcc (GRAIN, gr_run inlined): eb04 8004 loads
    // g->z (offset 0) and g->pos (offset 4) of a gr_grain_t; the next
    // instructions read z->n through r2. Descending order put pos in r2 and
    // faulted on the read of [pos + 4].
    let mut c = cpu(&[0xeb04, 0x8004]);
    c.r[4] = RAM;
    c.bus.write(RAM, 0x0201_2da4, 4).unwrap(); // z
    c.bus.write(RAM + 4, 0x256c, 4).unwrap(); // pos
    c.step().unwrap();
    assert_eq!(c.r[2], 0x0201_2da4);
    assert_eq!(c.r[15], 0x256c);
    assert_eq!(c.r[4], RAM);
    assert_eq!(c.pc, XIP + 4);
}

#[test]
fn register_list_store_and_load_keep_stock_linked_lists_consistent() {
    // Stock FM-1 0x02002284: list insertion before the head r1:
    // 6112 r2 = [r1+4]; 6190 [r1+4] = r0; eb20 0006 {r1, r2} -> [r0];
    // 60a0 [r2] = r0. Then 0x0201fb7a removes it again:
    // eb00 0006 {r1, r2} <- [r0]; 6192 [r1+4] = r2; 60a1 [r2] = r1.
    // Nodes are {next, prev}; the list starts as head <-> a.
    let (head, a, node) = (RAM + 0x100, RAM + 0x180, RAM + 0x200);
    let mut c = cpu(&[
        0x6112, 0x6190, 0xeb20, 0x0006, 0x60a0, 0xeb00, 0x0006, 0x6192, 0x60a1,
    ]);
    for (at, value) in [(head, a), (head + 4, a), (a, head), (a + 4, head)] {
        c.bus.write(at, value, 4).unwrap();
    }
    c.r[0] = node;
    c.r[1] = head;
    for _ in 0..4 {
        c.step().unwrap();
    }
    // head <-> a <-> node <-> head
    assert_eq!(c.bus.read(head + 4, 4).unwrap(), node); // head.prev
    assert_eq!(c.bus.read(a, 4).unwrap(), node); // a.next
    assert_eq!(c.bus.read(node, 4).unwrap(), head); // node.next
    assert_eq!(c.bus.read(node + 4, 4).unwrap(), a); // node.prev
    for _ in 0..3 {
        c.step().unwrap();
    }
    for (at, value) in [(head, a), (head + 4, a), (a, head), (a + 4, head)] {
        assert_eq!(c.bus.read(at, 4).unwrap(), value);
    }
}

#[test]
fn halfword_postincrement_with_low_bit_set_stores() {
    // Felucca 0.9-beta 0x02011e98 (GRAIN gr_fill: rb[q - lo] = pred):
    // edd0 10f3 = h[r15 ++= 2] = r1. As in the edd8 register forms, x bit 0
    // selects the store and the increment is even; reading it as a load
    // advanced r15 by 3 and faulted on the next unaligned access.
    let mut c = cpu(&[0xedd0, 0x10f3]);
    c.r[15] = RAM + 0x3a0;
    c.r[1] = 0xffff_9271;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x3a0, 2).unwrap(), 0x9271);
    assert_eq!(c.r[15], RAM + 0x3a2);
    assert_eq!(c.r[1], 0xffff_9271);
    assert_eq!(c.pc, XIP + 4);
}

#[test]
fn pair_shift_by_immediate_shifts_the_64_bit_register_pair() {
    // Felucca 0.9-beta 0x02010462 (VOICE, eng_formant.c: f0 >> 4 with
    // f0 = ((uint64_t)inc * 705600) >> 32): e1d0 2a04 = r3:r2 >>= 36
    // (logical). Mode (x >> 10) & 3 as in e1c0: 0 lsl, 2 lsr, 3 asr; the
    // pair is x >> 12 (even), the count ((x >> 8) & 3) * 16 + (x & 15).
    let mut c = cpu(&[0xe1d0, 0x2a04]);
    c.r[2] = 0xa2dd_ec80;
    c.r[3] = 0x0000_125a;
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[3]), (0x125, 0));
    assert_eq!(c.pc, XIP + 4);
    let mut c = cpu(&[0xe1d0, 0x4e00]); // r5:r4 >>>= 32 (Felucca 0x020102a4)
    c.r[4] = 0x1234_5678;
    c.r[5] = 0x8000_0001;
    c.step().unwrap();
    assert_eq!((c.r[4], c.r[5]), (0x8000_0001, 0xffff_ffff));
    let mut c = cpu(&[0xe1d0, 0x2002]); // r3:r2 <<= 2 (Felucca 0x02010294)
    c.r[2] = 0xc000_0001;
    c.r[3] = 1;
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[3]), (4, 7));
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
fn multiply_accumulate_adds_the_signed_product_to_the_pair() {
    // Felucca 0.9-beta 0x020108cc (VOICE formant resonators, int64 sums):
    // e1fc 50a0 = r5:r4 += r10 * r0. x bit 12 set selects signed, as for
    // e1f8; the pair is (x >> 12) & 14.
    let mut c = cpu(&[0xe1fc, 0x50a0]);
    c.r[4] = 0x2000_0000;
    c.r[5] = 0;
    c.r[10] = 0xffff_fffe; // -2
    c.r[0] = 0x4000_0000;
    c.step().unwrap();
    // 0x20000000 - 0x80000000 = -0x60000000
    assert_eq!((c.r[4], c.r[5]), (0xa000_0000, 0xffff_ffff));
    assert_eq!(c.pc, XIP + 4);
}

#[test]
fn long_divide_with_bit_12_set_is_signed() {
    // Felucca 0.9-beta 0x02010524 / Jangada 0x020127be (eng_formant.c:
    // b = ((int64_t)b * (int32_t)(f0 + (f0 >> 4))) / f): e1f6 3620 =
    // r3:r2 = r3:r2 / r6, signed (bit 12, as for e1f8 and e1fc).
    let mut c = cpu(&[0xe1f6, 0x3620]);
    c.r[2] = 0xffff_ff00; // -256
    c.r[3] = 0xffff_ffff;
    c.r[6] = 16;
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[3]), (0xffff_fff0, 0xffff_ffff)); // -16
    let mut c = cpu(&[0xe1f6, 0x3620]);
    c.r[2] = 0x0016_0434;
    c.r[3] = 0;
    c.r[6] = 0xffff_fffe; // -2
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[3]), (0xfff4_fde6, 0xffff_ffff));
}
