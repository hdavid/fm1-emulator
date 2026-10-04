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
