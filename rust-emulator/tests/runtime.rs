// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}
#[test]
fn compiler_parallel_store_uses_the_previous_register_value() {
    // Vendor compiler startup: r0 = 0x4009 # [r1+4] = r0.
    let mut c = cpu(&[0xf040, 0x4009, 0x6190]);
    c.r[0] = 17;
    c.r[1] = RAM;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), 17);
    assert_eq!(c.r[0], 0x4009);
    assert_eq!(c.pc, XIP + 6);
}

#[test]
fn parallel_add_reads_the_old_value_of_the_register_loaded_by_the_other_slot() {
    // cv_text: r1 += r0 # r0 = [sp].
    let mut c = cpu(&[0xd801, 0x2000]);
    c.r[0] = 0x3900;
    c.r[1] = 0x0205c9f0;
    c.sr[14] = RAM;
    c.bus.write(RAM, 0x0205c930, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], 0x020602f0);
    assert_eq!(c.r[0], 0x0205c930);
    assert_eq!(c.pc, XIP + 4);
}
#[test]
fn compiler_signed_immediates_terminate_decimal_printing() {
    let mut c = cpu(&[0xe040, 0xa240, 0xf8f5, 0xfffe]);
    c.step().unwrap();
    assert_eq!(c.r[0], (-24000i32) as u32);
    c.r[5] = u32::MAX;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 8);
}
#[test]
fn conditional_block_executes_only_the_selected_arm() {
    // if ((r3 & 0x8000)==0) { r0=1 } else { r0=2 }
    for (condition, expected) in [(0, 1), (0x8000, 2)] {
        let mut c = cpu(&[0xea23, 0x1c00, 0x2140, 0x2240, 0x0000]);
        c.r[3] = condition;
        c.step().unwrap();
        c.step().unwrap();
        c.step().unwrap();
        assert_eq!(c.r[0], expected);
        assert_eq!(c.pc, XIP + 10);
    }
}

#[test]
fn unsigned_immediate_conditional_selects_storage_header_region() {
    // Vendor form from st_head: if (r5 < 5) { r0=11 } else { r0=22 }.
    for (value, expected) in [(0, 11), (4, 11), (5, 22), (u32::MAX, 22)] {
        let mut c = cpu(&[0xe9b5, 0x1005, 0x2b40, 0x3640, 0x0000]);
        c.r[5] = value;
        for _ in 0..3 {
            c.step().unwrap();
        }
        assert_eq!(c.r[0], expected);
        assert_eq!(c.pc, XIP + 10);
    }
}

#[test]
fn register_list_loads_descend_without_changing_the_base() {
    // Vendor form: 04 eb 04 01 => {r8, r2} = [r4+].
    for upper in [4, 8, 15] {
        let mut c = cpu(&[0xeb04, (1 << upper) | (1 << 2)]);
        c.r[4] = RAM;
        c.bus.write(RAM, 0x11223344, 4).unwrap();
        c.bus.write(RAM + 4, 0xaabbccdd, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[upper], 0x11223344);
        assert_eq!(c.r[2], 0xaabbccdd);
        if upper != 4 {
            assert_eq!(c.r[4], RAM);
        }
    }
}

#[test]
fn register_pairs_move_both_words_without_clobbering_the_source() {
    for destination in (0..16).step_by(2) {
        for source in (0..16).step_by(2) {
            let mut c = cpu(&[0x1500 | (source as u16) << 4 | destination as u16]);
            c.r = std::array::from_fn(|i| 0x12345600 + i as u32);
            let before = c.r;
            c.step().unwrap();
            assert_eq!(
                c.r[destination..destination + 2],
                before[source..source + 2]
            );
            assert_eq!(c.pc, XIP + 2);
        }
    }
}

#[test]
fn signed_division_truncates_toward_zero_and_rejects_undefined_cases() {
    for (left, right, expected) in [(128, 2, 64), (-9, 2, -4), (9, -2, -4), (-9, -2, 4)] {
        let mut c = cpu(&[0xe1f4, 0x0101]); // r0 = r0 / r1 (s)
        c.r[0] = left as u32;
        c.r[1] = right as u32;
        c.step().unwrap();
        assert_eq!(c.r[0], expected as u32);
    }
    for (left, right) in [(1, 0), (i32::MIN, -1)] {
        let mut c = cpu(&[0xe1f4, 0x0101]);
        c.r[0] = left as u32;
        c.r[1] = right as u32;
        assert!(c.step().is_err());
    }
}

#[test]
fn three_operand_subtraction_preserves_the_text_canvas_address() {
    for (left, right, expected) in [(RAM + 128, 4, RAM + 124), (0, 1, u32::MAX)] {
        let mut c = cpu(&[0xe0b4, 0x00c2]); // r0 = r12 - r0
        c.r[12] = left;
        c.r[0] = right;
        c.step().unwrap();
        assert_eq!(c.r[0], expected);
        assert_eq!(c.r[12], left);
    }
}

#[test]
fn halfword_postincrement_reads_before_advancing_parameter_pointer() {
    let mut c = cpu(&[0xedd0, 0x2104]); // r2 = h[r0++=20] (u)
    c.r[0] = RAM;
    c.bus.write(RAM, 0xfedc, 2).unwrap();
    c.bus.write(RAM + 20, 0x1234, 2).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[2], 0xfedc);
    assert_eq!(c.r[0], RAM + 20);
}
fn p33(c: &mut Bus, command: u8, address: u8, value: u8) {
    c.write(0x13e08, 1, 4).unwrap();
    for byte in [command, address, value] {
        c.write(0x13e0c, byte as u32, 4).unwrap();
        c.write(0x13e08, 17, 4).unwrap();
    }
    c.write(0x13e08, 0, 4).unwrap();
}
#[test]
fn watchdog_is_fed_via_serial_p33_and_really_expires() {
    let mut b = Bus::new(vec![0, 0]).unwrap();
    p33(&mut b, 0, 0x80, 0x1a);
    b.system.advance(23_999_999).unwrap();
    p33(&mut b, 0x20, 0x80, 0x40);
    assert_eq!(b.system.watchdog_feeds, 1);
    b.system.advance(23_999_999).unwrap();
    assert!(b.system.advance(1).is_err());
}
#[test]
fn guarded_ram_rejects_writes_and_usb_dma_checks_its_address() {
    let mut b = Bus::new(vec![0, 0]).unwrap();
    b.write(0x1eee2c0, RAM, 4).unwrap();
    b.write(0x1eee280, RAM + 255, 4).unwrap();
    b.write(0x1eee348, 1, 4).unwrap();
    assert!(b.write(RAM, 0, 4).is_err());
    b.write(RAM + 256, 0, 4).unwrap();
    b.write(0x11800, 4, 4).unwrap();
    b.write(0x51000, 0x40, 4).unwrap();
    b.advance_usb(1).unwrap();
    assert!(b.advance_usb(120000).is_err());
}
