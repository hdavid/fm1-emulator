// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{bus::Bus, XIP};

fn plain_window(bus: &mut Bus) {
    bus.write(0x4030c, 0x0208f000, 4).unwrap();
    bus.write(0x40308, 0x07ffffff, 4).unwrap();
    bus.write(0x40300, 3, 1).unwrap();
}

#[test]
fn erased_user_flash_reads_through_the_guest_plain_window() {
    let mut bus = Bus::new(vec![0x12, 0x34, 0x56, 0x78]).unwrap();
    assert!(bus.read(0x0209c000, 4).is_err());
    plain_window(&mut bus);
    assert_eq!(bus.read(XIP, 4).unwrap(), 0x78563412);
    for address in [0x0208f000, 0x0209c000, 0x020f8000, 0x020fbffc] {
        assert_eq!(bus.read(address, 4).unwrap(), u32::MAX);
        assert_eq!(bus.read(address, 2).unwrap(), 0xffff);
        assert_eq!(bus.read(address, 1).unwrap(), 0xff);
    }
    assert!(bus.read(0x020fc000, 1).is_err()); // Physical NOR is exactly 1 MiB.
    assert!(bus.read(0x0208effc, 4).is_err()); // Outside the plaintext range.
    assert!(bus.write(0x0209c000, 0, 4).is_err()); // XIP is read-only.
    bus.write(0x40200, 0, 4).unwrap();
    assert!(bus.read(XIP, 4).is_err());
    assert!(bus.read(0x0209c000, 4).is_err());
}
