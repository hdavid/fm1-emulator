// SPDX-License-Identifier: GPL-3.0-only
// Stock firmware is external and must be supplied unchanged as a full FWSC.
use fm1_emu::{cpu::Cpu, firmware::Firmware};
use std::{env, path::Path};

fn advance(cpu: &mut Cpu, instructions: u64) {
    for _ in 0..instructions {
        cpu.step()
            .unwrap_or_else(|error| panic!("after {} instructions: {error}", cpu.steps));
    }
}

fn button(cpu: &mut Cpu, column: usize, row: usize) {
    let before = cpu.bus.lcd.pixels.clone();
    let pixels = cpu.bus.lcd.pixels_written;
    cpu.bus.devices.gpio.press(column, row, true).unwrap();
    advance(cpu, 36_000_000);
    cpu.bus.devices.gpio.press(column, row, false).unwrap();
    advance(cpu, 100_000_000);
    assert_ne!(
        cpu.bus.lcd.pixels, before,
        "the button must change the page"
    );
    assert!(cpu.bus.lcd.pixels_written > pixels + 240 * 240);
}

#[test]
#[ignore = "requires FM1_STOCK_FWSC (official FM-1_015); run in release mode with --ignored"]
fn official_package_boots_changes_pages_and_renders_a_factory_note() {
    let path = env::var("FM1_STOCK_FWSC").expect("set FM1_STOCK_FWSC to unchanged FM-1.fwsc");
    let firmware = Firmware::load(Path::new(&path)).unwrap();
    let mut cpu = Cpu::new(firmware.bus().unwrap(), firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    // Go beyond the late temperature-ADC subscription at about three seconds.
    advance(&mut cpu, 1_200_000_000);
    assert!(cpu.bus.screen_visible());
    assert!(cpu.bus.lcd.pixels_written > 150_000);
    assert!(cpu.bus.devices.adc.conversions > 1_000);
    assert!(cpu.bus.audio.halves > 1_000);
    assert!(cpu.bus.system.watchdog_feeds > 100);
    assert!(cpu.irq_entries > 1_000);
    assert!(cpu.bus.audio.samples.iter().all(|frame| *frame == [0, 0]));

    button(&mut cpu, 6, 1); // FX.
    button(&mut cpu, 7, 1); // HOME.
    let audio = cpu.bus.audio.halves;
    let watchdog = cpu.bus.system.watchdog_feeds;
    cpu.bus.audio.samples.clear();
    cpu.bus.devices.gpio.press(3, 4, true).unwrap(); // First white note.
    advance(&mut cpu, 50_000_000);
    assert!(
        cpu.bus.audio.samples.iter().any(|frame| *frame != [0, 0]),
        "the factory preset must render a note through guest stereo DMA"
    );
    cpu.bus.devices.gpio.press(3, 4, false).unwrap();
    advance(&mut cpu, 50_000_000);
    assert!(cpu.bus.audio.halves > audio + 50);
    assert!(cpu.bus.system.watchdog_feeds > watchdog);
    eprintln!(
        "Official FM-1_015: {} CPU steps, {} LCD pixels, {} audio halves, {} ADC conversions, {} watchdog feeds",
        cpu.steps, cpu.bus.lcd.pixels_written, cpu.bus.audio.halves,
        cpu.bus.devices.adc.conversions, cpu.bus.system.watchdog_feeds
    );
}
