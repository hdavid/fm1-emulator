// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    devices::TIMER4,
    firmware::Firmware,
    lcd::{IOMAP, SPI},
    PROBE_RETURN, USER_STACK,
};
use std::path::Path;

fn load(path: &str) -> Firmware {
    Firmware::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path)).unwrap()
}
fn call(cpu: &mut Cpu, entry: u32) {
    cpu.pc = entry;
    cpu.sr[3] = PROBE_RETURN;
    cpu.sr[14] = USER_STACK;
    let registers = cpu.r;
    cpu.run(Some(PROBE_RETURN), cpu.steps + 200_000, None)
        .unwrap();
    assert_eq!(cpu.sr[14], USER_STACK);
    assert_eq!(
        cpu.r, registers,
        "display C ABI must preserve working registers"
    );
}
#[test]
fn hardware_renderer_returns_and_matches_the_emulator_screen() {
    let demo = load("tests/fixtures/display/firmware.elf");
    let mut reference = Cpu::new(Bus::new(demo.image).unwrap(), demo.entry);
    reference.bus.devices.gpio.press(0, 4, true).unwrap();
    reference
        .run(Some(demo.symbols["display_frame_done"]), 200_000, None)
        .unwrap();
    let hardware = load("build/display/firmware.elf");
    let mut cpu = Cpu::new(Bus::new(hardware.image).unwrap(), hardware.entry);
    // State produced by the inherited hardware LCD init. No application
    // startup substitution is claimed: this calls only the shared renderer.
    for (address, value) in [
        (IOMAP, 0x10),
        (SPI, 0x4021),
        (SPI + 4, 4),
        (0x50088, !0x780),
        (0x50008, !4),
        (0x50080, 0),
        (SPI + 8, 1),
        (SPI + 8, 0x11),
        (SPI + 8, 0x3a),
        (0x50080, 0x100),
        (SPI + 8, 0x55),
        (0x50080, 0xa5000000),
    ] {
        cpu.bus.write(address, value, 4).unwrap();
    }
    for (i, register) in cpu.r.iter_mut().enumerate() {
        *register = 0xface0000 + i as u32;
    }
    call(&mut cpu, hardware.symbols["display_init"]);
    let ticks = reference
        .bus
        .read(demo.symbols["display_ticks"], 4)
        .unwrap();
    cpu.bus.write(TIMER4 + 4, ticks, 4).unwrap(); // stopped timer: exact displayed value
    for column in 0..11 {
        let rows = reference
            .bus
            .read(demo.symbols["matrix_results"] + column * 4, 4)
            .unwrap();
        cpu.bus
            .write(hardware.symbols["matrix_results"] + column * 4, rows, 4)
            .unwrap();
    }
    call(&mut cpu, hardware.symbols["display_frame"]);
    assert_eq!(cpu.bus.lcd.pixels, reference.bus.lcd.pixels);
    assert!(cpu.bus.screen_visible());
    assert_eq!(cpu.bus.read(0x50080, 4).unwrap() & !0x180, 0xa5000000);
    assert_eq!(
        cpu.bus.read(hardware.symbols["display_frames"], 4).unwrap(),
        1
    );
}

#[test]
fn hardware_application_boots_usb_and_updates_the_display() {
    let firmware = load("build/display/firmware.elf");
    let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
    cpu.r[0] = 0x01c7fe08;
    for _ in 0..35_000_000 {
        cpu.step().unwrap();
    }
    assert!(cpu.bus.screen_visible());
    assert!(cpu.bus.read(firmware.symbols["display_frames"], 4).unwrap() > 1);
    assert_eq!(cpu.bus.usb.setups, 5);
    assert!(String::from_utf8(cpu.bus.usb.serial.drain(..).collect())
        .unwrap()
        .contains("FM-1 DIAG"));
    assert_eq!(cpu.bus.read(firmware.symbols["fm1_in"] + 4, 4).unwrap(), 0);
    let previous_screen = cpu.bus.lcd.pixels.clone();
    cpu.bus.devices.gpio.press(0, 4, true).unwrap();
    for _ in 0..3_000_000 {
        cpu.step().unwrap();
    }
    let down = String::from_utf8(cpu.bus.usb.serial.drain(..).collect()).unwrap();
    assert_eq!(down, "KEY 0 down\r\n");
    assert_ne!(cpu.bus.lcd.pixels, previous_screen);
    cpu.bus.devices.gpio.press(0, 4, false).unwrap();
    for _ in 0..3_000_000 {
        cpu.step().unwrap();
    }
    assert_eq!(
        String::from_utf8(cpu.bus.usb.serial.drain(..).collect()).unwrap(),
        "KEY 0 up\r\n"
    );
    assert!(cpu.bus.system.watchdog_feeds > 100);
}
