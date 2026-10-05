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
