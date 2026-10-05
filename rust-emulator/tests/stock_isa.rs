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
        assert_eq!(
            c.pc,
            if taken { XIP - 4 } else { XIP + 4 },
            "r1 = {elapsed}"
        );
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
fn register_repeat_runs_its_block_count_times_even_when_the_body_reuses_the_register() {
    // FM-1_093 memcpy tail at 0x02044596: 0312 0712 07b2 0456 =
    // rep 4 bytes, r2 { r2 = b[r1++]; b[r3++] = r2 }; pop pc. The count
    // register is the byte temporary inside the body and no branch follows,
    // so the count must be latched: sdfile copying "btif" (4 bytes) got only
    // "b" and the BTIF directory lookup failed.
    let mut c = cpu(&[0x0312, 0x0712, 0x07b2, 0x0000]);
    c.r[1] = RAM;
    c.r[2] = 4;
    c.r[3] = RAM + 0x40;
    for (i, byte) in b"btif".iter().enumerate() {
        c.bus.write(RAM + i as u32, *byte as u32, 1).unwrap();
    }
    while c.pc != XIP + 6 {
        c.step().unwrap();
        assert!(c.steps <= 10);
    }
    for (i, byte) in b"btif".iter().enumerate() {
        assert_eq!(c.bus.read(RAM + 0x40 + i as u32, 1).unwrap(), *byte as u32);
    }
    assert_eq!(c.bus.read(RAM + 0x44, 1).unwrap(), 0);
    assert_eq!(c.r[1], RAM + 4);
    assert_eq!(c.r[3], RAM + 0x44);
}

#[test]
fn signed_greater_than_register_conditional_guards_its_then_block() {
    // FM-1_093 0x02002bea: ee15 4200 ed50 5097 1652 =
    // if (r5 > r2) { h[r9+6] = r5; mov } with r2 = h[r9+6] (s) = -1 just
    // loaded. Kind 0xe1 is the register form of the signed greater-than
    // test, matching the compare-branch family where 0xee00 is signed >.
    for (r5, taken) in [(3u32, true), (0xffff_fffe, false), (0xffff_ffff, false)] {
        let mut c = cpu(&[0xee15, 0x4200, 0xed50, 0x5097, 0x1652, 0x0000]);
        c.r[2] = 0xffff_ffff;
        c.r[5] = r5;
        c.r[9] = RAM;
        c.bus.write(RAM + 6, 0x7777, 2).unwrap();
        while c.pc < XIP + 10 {
            c.step().unwrap();
            assert!(c.steps <= 3);
        }
        assert_eq!(c.pc, XIP + 10, "r5 = {r5:#x}");
        let stored = c.bus.read(RAM + 6, 2).unwrap();
        assert_eq!(
            stored,
            if taken { r5 & 0xffff } else { 0x7777 },
            "r5 = {r5:#x}"
        );
    }
}

#[test]
fn special_register_pop_with_only_the_rets_bit_restores_rets() {
    // FM-1_093 variadic wrapper at 0x0200322a: push {r3..r0}; push rets; ...
    // call; sp += 16; 0488; sp += 16; rts. 0x0480 | mask pops special
    // registers by bitmap (0x04a9 = {psr, rets, reti}: bits 5, 3, 0), so
    // 0x0488 is pop {rets}.
    let mut c = cpu(&[0x0488, 0x0000]);
    c.sr[14] = RAM + 0x100;
    c.sr[5] = 0x55;
    c.bus.write(RAM + 0x100, 0x0200_3334, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.sr[3], 0x0200_3334);
    assert_eq!(c.sr[14], RAM + 0x104);
    assert_eq!(c.sr[5], 0x55);
    assert_eq!(c.pc, XIP + 2);
}

#[test]
fn register_list_store_puts_the_lowest_register_at_the_lowest_address() {
    // FM-1_093 list_add_tail at 0x02003338: eb20 0006 = [r0] = {r2, r1} with
    // r1 = head (new->next, offset 0) and r2 = old tail (new->prev,
    // offset 4). Storing r2 first linked the second entry to the first in a
    // loop that lost the head; list removal then spun until the watchdog.
    let mut c = cpu(&[0xeb20, 0x0006]);
    c.r[0] = RAM + 0x20;
    c.r[1] = 0x01c0_92c8;
    c.r[2] = 0x01c2_4b20;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x20, 4).unwrap(), 0x01c0_92c8);
    assert_eq!(c.bus.read(RAM + 0x24, 4).unwrap(), 0x01c2_4b20);
    assert_eq!(c.r[0], RAM + 0x20);
}

#[test]
fn register_list_load_fills_the_lowest_register_from_the_lowest_address() {
    // Vendor-compiled Felucca fm1_fault_c (fm1_irq.h) at 0x020006f2:
    // eb04 0022 = {r5, r1} = [r4+] reads fm1_crash.magic into r1 (compared
    // with "CRSH") and fm1_crash.count into r5 (incremented).
    let mut c = cpu(&[0xeb04, 0x0022]);
    c.r[4] = RAM + 0x40;
    c.bus.write(RAM + 0x40, 0x4352_5348, 4).unwrap();
    c.bus.write(RAM + 0x44, 7, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], 0x4352_5348);
    assert_eq!(c.r[5], 7);
    assert_eq!(c.r[4], RAM + 0x40);
}

#[test]
fn pair_shift_moves_the_high_word_down_for_a_forty_bit_field() {
    // FM-1_093 0x0207493e: f1d0 2a00 || 4489, then 478a. Bytes 3..6 of a
    // record come from r2 and byte 7 from r2 after r3:r2 >>= 32 (r3 holds
    // the byte-7 value read earlier), so the shift works on the pair.
    let mut c = cpu(&[0xe1d0, 0x2a00]);
    c.r[2] = 0x2308_1811;
    c.r[3] = 0x0000_00a5;
    c.step().unwrap();
    assert_eq!(c.r[2], 0xa5);
    assert_eq!(c.r[3], 0);
    assert_eq!(c.pc, XIP + 4);
}

#[test]
fn pair_shifts_sign_extend_and_scale_sixty_four_bit_products() {
    // Vendor-compiled Felucca 0x020102a8: e1d0 4200; e1d0 4e00 is
    // (int64)r4 in r5:r4 (shift left 32, arithmetic right 32), and
    // e1d0 490e after a signed 64-bit multiply is the Q30 >> 30.
    let mut c = cpu(&[0xe1d0, 0x4200, 0xe1d0, 0x4e00]);
    c.r[4] = 0xffff_fff0;
    c.r[5] = 0x1234_5678;
    c.step().unwrap();
    assert_eq!((c.r[5], c.r[4]), (0xffff_fff0, 0));
    c.step().unwrap();
    assert_eq!((c.r[5], c.r[4]), (0xffff_ffff, 0xffff_fff0));
    let mut c = cpu(&[0xe1d0, 0x490e]);
    let product = (0x3000_0000i64 * 0x2000_0000i64) as u64;
    c.r[4] = product as u32;
    c.r[5] = (product >> 32) as u32;
    c.step().unwrap();
    assert_eq!(c.r[4], (product >> 30) as u32);
    assert_eq!(c.r[5], (product >> 62) as u32);
}

#[test]
fn register_repeat_with_a_zero_count_skips_its_block() {
    let mut c = cpu(&[0x0312, 0x0712, 0x07b2, 0x0000]);
    c.r[1] = RAM;
    c.r[3] = RAM + 0x40;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 6);
    assert_eq!(c.r[1], RAM);
}
