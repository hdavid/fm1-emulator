// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    bus::Bus,
    lcd::{IOMAP, SPI},
    RAM, XIP,
};

fn write(bus: &mut Bus, address: u32, value: u32) {
    bus.write(address, value, 4).unwrap();
}
fn cmd(bus: &mut Bus, command: u32) {
    write(bus, 0x50080, 0);
    write(bus, SPI, 0x4021);
    write(bus, SPI + 8, command);
}
fn data(bus: &mut Bus, bytes: &[u8]) {
    for (i, &byte) in bytes.iter().enumerate() {
        bus.write(RAM + i as u32, byte as u32, 1).unwrap();
    }
    write(bus, 0x50080, 0x100);
    write(bus, SPI + 12, RAM);
    write(bus, SPI + 16, bytes.len() as u32);
}
fn init() -> Bus {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    write(&mut bus, IOMAP, 0x10);
    write(&mut bus, 0x50088, !0x780);
    write(&mut bus, 0x50008, !4);
    write(&mut bus, SPI + 4, 4);
    cmd(&mut bus, 1);
    cmd(&mut bus, 0x11);
    cmd(&mut bus, 0x3a);
    data(&mut bus, &[0x55]);
    cmd(&mut bus, 0x36);
    data(&mut bus, &[0]);
    cmd(&mut bus, 0x21);
    cmd(&mut bus, 0x13);
    cmd(&mut bus, 0x29);
    bus
}
#[test]
fn rgb565_dma_respects_window_wrap_and_split_pixels() {
    let mut bus = init();
    cmd(&mut bus, 0x2a);
    data(&mut bus, &[0, 7, 0, 8]);
    cmd(&mut bus, 0x2b);
    data(&mut bus, &[0, 9, 0, 10]);
    cmd(&mut bus, 0x2c);
    data(&mut bus, &[0xf8]); // high byte survives separate DMA transfers
    data(&mut bus, &[0, 7, 0xe0, 0, 0x1f, 0xff, 0xff]);
    assert_eq!(bus.lcd.pixels[9 * 240 + 7], 0xff0000);
    assert_eq!(bus.lcd.pixels[9 * 240 + 8], 0x00ff00);
    assert_eq!(bus.lcd.pixels[10 * 240 + 7], 0x0000ff);
    assert_eq!(bus.lcd.pixels[10 * 240 + 8], 0xffffff);
    assert_eq!(bus.lcd.pixels[0], 0);
    data(&mut bus, &[0, 0]);
    assert_eq!(bus.lcd.pixels[9 * 240 + 7], 0);
    assert_eq!(bus.lcd.pixels_written, 5);
    assert_ne!(bus.read(SPI, 4).unwrap() & 0x8000, 0);
    write(&mut bus, SPI, 0x4021);
    assert_eq!(bus.read(SPI, 4).unwrap() & 0x8000, 0);
}
#[test]
fn display_sleep_and_backlight_gate_visibility() {
    let mut bus = init();
    assert!(bus.screen_visible());
    write(&mut bus, 0x50000, 4);
    assert!(!bus.screen_visible());
    write(&mut bus, 0x50000, 0);
    assert!(bus.screen_visible());
    cmd(&mut bus, 0x28);
    assert!(!bus.screen_visible());
    cmd(&mut bus, 0x29);
    cmd(&mut bus, 0x10);
    assert!(!bus.screen_visible());
    cmd(&mut bus, 0x11);
    assert!(bus.screen_visible());
    cmd(&mut bus, 1);
    assert!(!bus.screen_visible());
}
#[test]
fn chip_select_suppresses_panel_writes() {
    let mut bus = init();
    write(&mut bus, 0x50080, 0x80);
    write(&mut bus, SPI + 8, 0x28);
    assert!(bus.screen_visible());
}
#[test]
fn invalid_dma_and_unsupported_commands_fault() {
    let mut bus = init();
    write(&mut bus, SPI + 12, RAM + 512 * 1024 - 1);
    assert!(bus.write(SPI + 16, 2, 4).is_err());
    write(&mut bus, SPI + 12, 0x02000120);
    assert!(bus.write(SPI + 16, 3, 4).is_err()); // Past the loaded image.
    write(&mut bus, SPI + 12, SPI);
    assert!(bus.write(SPI + 16, 1, 4).is_err()); // MMIO is not a DMA source.
    assert!(bus.write(SPI, 0, 1).is_err());
    assert!(bus.read(SPI + 1, 1).is_err());
    write(&mut bus, 0x50080, 0);
    assert!(bus.write(SPI + 8, 0xff, 4).is_err());
    cmd(&mut bus, 0x2a);
    write(&mut bus, 0x50080, 0x100);
    for byte in [0, 0, 0] {
        write(&mut bus, SPI + 8, byte);
    }
    assert!(bus.write(SPI + 8, 240, 4).is_err());
}

#[test]
fn lcd_dma_can_read_flash_constants_at_a_byte_aligned_address() {
    let mut bus = init();
    bus.flash = vec![0, 0xf8, 0, 0x07, 0xe0];
    cmd(&mut bus, 0x2c);
    write(&mut bus, 0x50080, 0x100);
    write(&mut bus, SPI + 12, XIP + 1);
    write(&mut bus, SPI + 16, 4);
    assert_eq!(bus.lcd.pixels_written, 2);
    assert_eq!(&bus.lcd.pixels[..2], &[0xff0000, 0x00ff00]);
    assert_ne!(bus.read(SPI, 4).unwrap() & 0x8000, 0);
    assert_eq!(bus.read(XIP + 1, 1).unwrap(), 0xf8);
    write(&mut bus, SPI + 12, XIP + 4);
    assert!(bus.write(SPI + 16, 2, 4).is_err());
    assert_eq!(bus.lcd.pixels_written, 2);
}
