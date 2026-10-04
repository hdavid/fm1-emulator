// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{cpu::Cpu, firmware::Firmware, XIP};
use std::path::Path;

#[test]
fn full_package_preserves_application_and_supplies_the_spl_handoff() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/display");
    let package = Firmware::load(&root.join("firmware.fwsc")).unwrap();
    let elf = Firmware::load(&root.join("firmware.elf")).unwrap();
    assert!(package.image.starts_with(&elf.image));
    let bus = package.bus().unwrap();
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
    for _ in 0..35_000_000 {
        cpu.step().unwrap();
    }
    assert!(cpu.bus.screen_visible());
    assert!(cpu.bus.lcd.pixels_written >= 240 * 240);
    assert!(cpu.bus.system.watchdog_feeds > 0);
    assert_eq!(cpu.bus.usb.setups, 5);
}
