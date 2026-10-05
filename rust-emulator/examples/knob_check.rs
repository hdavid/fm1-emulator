// SPDX-License-Identifier: GPL-3.0-only
// Headless check that a firmware reacts to a turned encoder: boot, save the
// LCD, play `detents` quadrature clicks into encoder `e` exactly as fm1-ui
// does, save the LCD again and report how many pixels changed.
//   knob_check FIRMWARE KNOB DETENTS OUT_PREFIX   (KNOB: SELECT ALGORITHM PRESETS KNOB1..KNOB4)
#[path = "../src/ui_knobs.rs"]
mod ui_knobs;
use fm1_emu::{cpu::Cpu, firmware::Firmware, png};
use std::{env, path::Path};

const BOOT_STEPS: u64 = 96_000_000; // 4 s of guest time
const SETTLE_STEPS: u64 = 72_000_000; // 3 s for the clicks to play and draw

fn run_for(cpu: &mut Cpu, encoders: &mut ui_knobs::Encoders, steps: u64) -> Result<(), String> {
    let end = cpu.steps + steps;
    while cpu.steps < end {
        if cpu.steps % 1024 == 0 {
            let contacts = encoders.update(|column| cpu.bus.devices.gpio.column_scans(column));
            for (e, (a, b)) in contacts.into_iter().enumerate() {
                let [ac, ar, bc, br] = ui_knobs::CONTACTS[e];
                cpu.bus.devices.gpio.press(ac, ar, a)?;
                cpu.bus.devices.gpio.press(bc, br, b)?;
            }
        }
        cpu.step().map_err(|fault| fault.to_string())?;
    }
    Ok(())
}

fn save(cpu: &Cpu, path: &str) -> Result<(), String> {
    let bytes = png::encode_rgb(240, 240, &cpu.bus.lcd.pixels)?;
    std::fs::write(path, bytes).map_err(|error| format!("{path}: {error}"))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [firmware, encoder, detents, prefix] = args.as_slice() else {
        return Err("usage: knob_check FIRMWARE KNOB DETENTS OUT_PREFIX".into());
    };
    use ui_knobs::knob::*;
    let encoder = match encoder.as_str() {
        "SELECT" => SELECT,
        "ALGORITHM" => ALGORITHM,
        "PRESETS" => PRESETS,
        "KNOB1" => KNOB1,
        "KNOB2" => KNOB1 + 1,
        "KNOB3" => KNOB1 + 2,
        "KNOB4" => KNOB1 + 3,
        _ => return Err("KNOB is SELECT ALGORITHM PRESETS KNOB1..KNOB4".into()),
    };
    let detents: i32 = detents.parse().map_err(|_| "DETENTS is an integer")?;
    let firmware = Firmware::load(Path::new(firmware))?;
    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    let mut encoders = ui_knobs::Encoders::default();
    run_for(&mut cpu, &mut encoders, BOOT_STEPS)?;
    *cpu.bus.mmio_stats.borrow_mut() = Some(Default::default());
    run_for(&mut cpu, &mut encoders, 24_000_000)?;
    if let Some(stats) = cpu.bus.mmio_stats.borrow_mut().take() {
        for port in [0x50004u32, 0x50044] {
            let reads: u64 = stats.iter().filter(|((a, _), _)| *a == port).map(|(_, c)| c[0]).sum();
            println!("GPIO IN 0x{port:x}: {reads} reads in 1 s of guest time");
        }
    }
    let before = cpu.bus.lcd.pixels.clone();
    save(&cpu, &format!("{prefix}-before.png"))?;
    encoders.turn(encoder, detents);
    run_for(&mut cpu, &mut encoders, SETTLE_STEPS)?;
    save(&cpu, &format!("{prefix}-after.png"))?;
    let changed = before
        .iter()
        .zip(&cpu.bus.lcd.pixels)
        .filter(|(a, b)| a != b)
        .count();
    println!("encoder {encoder}, {detents} clicks: {changed} of 57600 pixels changed");
    Ok(())
}
