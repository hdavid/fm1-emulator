// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{cpu::Cpu, firmware::Firmware, XIP};
use std::path::Path;

#[test]
fn full_package_preserves_application_and_supplies_the_spl_handoff() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/display");
    let package = Firmware::load(&root.join("firmware.fwsc")).unwrap();
    let elf = Firmware::load(&root.join("firmware.elf")).unwrap();
    assert!(package.image.starts_with(&elf.image));
    let mut bus = package.bus().unwrap();
    assert_eq!(bus.read(0x10200, 4).unwrap(), 0x6f01);
    assert!(bus.write(0x10200, 0, 4).is_err());
    for (address, value) in [
        (0x40200, 0x009803b5),
        (0x40204, 1),
        (0x40208, 0x8e17),
        (0x4020c, 0x4000),
        (0x40300, 1),
        (0x40304, 0),
        (0x16a04, 0x8881c3),
    ] {
        assert_eq!(bus.read(address, 4).unwrap(), value);
    }
    bus.write(0x4020c, 0x4004, 4).unwrap();
    assert_eq!(
        bus.read(XIP, 2).unwrap(),
        u16::from_le_bytes([package.image[4], package.image[5]]) as u32
    );
    bus.write(0x4020c, 0x4000, 4).unwrap();
    assert!(bus.write(0x40304, 1, 4).is_err());
    // Measured FM-1 handoff before application peripheral initialization.
    for (address, value) in [
        (0x10008, 0x10200),
        (0x1000c, 0x1c1),
        (0x10010, 0x10000),
        (0x10014, 6),
        (0x10018, 2),
        (0x119a0, 0x45400203),
        (0x119a4, 0x3f503026),
        (0x119a8, 0x0940022b),
        (0x119ac, 0x0750310c),
        (0x13e00, 0x100),
        (0x13e04, 0xe0),
    ] {
        assert_eq!(bus.read(address, 4).unwrap(), value);
    }
    let head = bus.read(0x01c7fe08, 4).unwrap();
    assert_eq!(bus.read(head + 8, 4).unwrap(), 0xff000);
    assert_eq!(bus.read(0x01c7fe0c, 4).unwrap(), 0x4000);
    assert_eq!(bus.read(0x01c7fe10, 4).unwrap(), 0x02000000);
    assert_eq!(bus.read(0x01c7fe14, 2).unwrap(), 0x980f);
    assert_eq!(
        bus.read(XIP, 2).unwrap(),
        u16::from_le_bytes([package.image[0], package.image[1]]) as u32
    );
    // Directory bytes preceding app.bin must also be available through SFC.
    assert_eq!(
        bus.read(0x02000010, 4).unwrap(),
        u32::from_le_bytes(*b"app_")
    );
    let mut cpu = Cpu::new(bus, package.entry);
    cpu.r[0] = 0x01c7fe08;
    for _ in 0..200_000_000 {
        cpu.step().unwrap();
    }
    assert!(cpu.bus.screen_visible());
    assert!(cpu.bus.lcd.pixels_written >= 240 * 240);
    assert!(cpu.bus.system.watchdog_feeds > 0);
    assert_eq!(cpu.bus.usb.setups, 5);
}
