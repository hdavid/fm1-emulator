// SPDX-License-Identifier: GPL-3.0-only
// Instruction forms first reached by X0X 0.9-beta (charlesvestal/fm1-x0x
// 80b7d40, built from source). JieLi objdump: 0x0800-0x13ff are loads and
// stores with a register post-increment, rD = bits 0-2, store = bit 3,
// rB = bits 4-6, rI = r8 + bits 7-9; 0x08-0x0b words, 0x0c-0x0f halfwords
// (loads zero-extend), 0x10-0x13 bytes:
//   0881 = r1 = [r0++=r9]        (X0X seq_advance, 0x0202148c)
//   0889 = [r0++=r9] = r1        0b7f = [r7++=r14] = r7
//   0c81 = r1 = h[r0++=r9] (u)   0c89 = h[r0++=r9] = r1
//   1081 = r1 = b[r0++=r9] (u)
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

#[test]
fn word_load_reads_then_advances_by_the_index_register() {
    let mut c = cpu(&[0x0881]);
    c.r[0] = RAM + 0x40;
    c.r[9] = 0x34;
    c.bus.write(RAM + 0x40, 0xdead_beef, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], 0xdead_beef);
    assert_eq!(c.r[0], RAM + 0x74);
    assert_eq!(c.pc, XIP + 2);
}

#[test]
fn word_store_and_the_highest_index_register() {
    let mut c = cpu(&[0x0889, 0x0b7f]);
    c.r[0] = RAM + 0x10;
    c.r[9] = (-4i32) as u32;
    c.r[1] = 0x1234_5678;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x10, 4).unwrap(), 0x1234_5678);
    assert_eq!(c.r[0], RAM + 0x0c);
    c.r[7] = RAM + 0x80;
    c.r[14] = 8;
    c.step().unwrap();
    // The stored value is r7 before the post-increment.
    assert_eq!(c.bus.read(RAM + 0x80, 4).unwrap(), RAM + 0x80);
    assert_eq!(c.r[7], RAM + 0x88);
}

#[test]
fn halfword_forms_zero_extend_and_store_the_low_half() {
    let mut c = cpu(&[0x0c81, 0x0c89]);
    c.r[0] = RAM + 0x20;
    c.r[9] = 2;
    c.bus.write(RAM + 0x20, 0xffff_8001, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], 0x8001);
    assert_eq!(c.r[0], RAM + 0x22);
    c.r[1] = 0xabcd_1234;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x20, 4).unwrap(), 0x1234_8001);
    assert_eq!(c.r[0], RAM + 0x24);
}

#[test]
fn byte_form_is_unchanged() {
    let mut c = cpu(&[0x1081]);
    c.r[0] = RAM + 0x31;
    c.r[9] = 1;
    c.bus.write(RAM + 0x30, 0x0000_9900, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], 0x99);
    assert_eq!(c.r[0], RAM + 0x32);
}
