// SPDX-License-Identifier: GPL-3.0-only
// Full firmware is external: use the unchanged ELF built from ~/src/Felucca.
use fm1_emu::{bus::Bus, cpu::Cpu, firmware::Firmware};
use std::{env, path::Path};

fn advance(cpu: &mut Cpu, instructions: u64) {
    for _ in 0..instructions {
        cpu.step()
            .unwrap_or_else(|error| panic!("after {} instructions: {error}", cpu.steps));
    }
}

#[test]
#[ignore = "requires FELUCCA_ELF; run in release mode with --ignored"]
fn unchanged_felucca_boots_and_responds_to_a_matrix_note() {
    let path = env::var("FELUCCA_ELF").expect("set FELUCCA_ELF to the full firmware ELF");
    let firmware = Firmware::load(Path::new(&path)).unwrap();
    let dbg = firmware.symbols["felucca_dbg"];
    let inputs = firmware.symbols["fm1_in"];
    let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
    cpu.r[0] = 0x01c7fe08;
    advance(&mut cpu, 100_000_000);
    assert_eq!(cpu.bus.read(dbg, 4).unwrap(), 0x44424731);
    assert!(cpu.bus.read(dbg + 28, 4).unwrap() > 10); // UI frames.
    assert!(cpu.bus.read(dbg + 4, 4).unwrap() > 10); // Guest-rendered audio halves.
    assert!(cpu.bus.devices.adc.conversions > 10);
    assert!(cpu.bus.system.watchdog_feeds > 10);
    assert!(cpu.bus.screen_visible());
    assert!(cpu.bus.lcd.pixels_written > 1_000_000);
    let serial: Vec<_> = cpu.bus.usb.serial.drain(..).collect();
    assert!(String::from_utf8_lossy(&serial).contains("Felucca 0.9-BETA console"));

    let before = cpu.bus.lcd.pixels.clone();
    cpu.bus.audio.samples.clear();
    cpu.bus.devices.gpio.press(3, 4, true).unwrap(); // First white note, panel ID 14.
    advance(&mut cpu, 5_000_000);
    assert_eq!(cpu.bus.read(inputs, 4).unwrap() & 1, 1);
    assert_ne!(cpu.bus.lcd.pixels, before);
    assert!(
        cpu.bus.audio.samples.iter().any(|frame| *frame != [0, 0]),
        "the note must produce guest stereo DMA samples"
    );
    cpu.bus.devices.gpio.press(3, 4, false).unwrap();
    advance(&mut cpu, 5_000_000);
    assert_eq!(cpu.bus.read(inputs, 4).unwrap() & 1, 0);
    cpu.bus.devices.gpio.press(2, 1, true).unwrap(); // ENV, panel ID 4.
    advance(&mut cpu, 1_000_000);
    cpu.bus.devices.gpio.press(2, 1, false).unwrap();
    advance(&mut cpu, 1_000_000);
    assert_eq!(
        cpu.bus.read(dbg + 52, 4).unwrap(),
        0,
        "ENV must leave the home page"
    );
    assert!(cpu.bus.system.watchdog_feeds > 10);
    eprintln!(
        "Felucca: {} instructions, {} UI frames, {} audio halves, {} watchdog feeds",
        cpu.steps,
        cpu.bus.read(dbg + 28, 4).unwrap(),
        cpu.bus.read(dbg + 4, 4).unwrap(),
        cpu.bus.system.watchdog_feeds
    );
}
