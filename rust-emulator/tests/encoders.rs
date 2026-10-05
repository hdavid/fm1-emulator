// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    encoders::{knob, Encoders, CONTACTS, SCANS_PER_PHASE},
    firmware::Firmware,
    gpio::{DIE, DIR, GPIO, IN, OUT, PU},
};
use std::{env, path::Path};

fn select(bus: &mut Bus, word: u16) {
    for bit in (0..16).rev() {
        let serial = if word & (1 << bit) != 0 { 16 } else { 0 };
        bus.write(GPIO + OUT, serial, 4).unwrap();
        bus.write(GPIO + OUT, serial | 8, 4).unwrap();
        bus.write(GPIO + OUT, serial, 4).unwrap();
    }
    bus.write(GPIO + OUT, 2, 4).unwrap();
    bus.write(GPIO + OUT, 0, 4).unwrap();
}

#[test]
fn a_detent_waits_for_the_guest_to_scan_the_encoder_columns() {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    bus.write(GPIO + DIR, 0x1e1, 4).unwrap();
    bus.write(GPIO + DIE, 0x1fb, 4).unwrap();
    bus.write(GPIO + PU, 0x1e1, 4).unwrap();
    let mut encoders = Encoders::default();
    encoders.turn(knob::KNOB1, 1);
    let [a_column, a_row, b_column, _] = CONTACTS[knob::KNOB1];
    assert_eq!((a_row, b_column), (1, a_column + 1));
    // Without guest reads of the columns, the contacts stay on the detent.
    for _ in 0..10 {
        encoders.drive(&mut bus.devices.gpio);
    }
    // Each phase holds for SCANS_PER_PHASE reads of both columns; the A
    // contact (PA4 with active-low row 1) closes in the second half of a click.
    let mut a_closed = Vec::new();
    for _ in 0..5 * SCANS_PER_PHASE {
        encoders.drive(&mut bus.devices.gpio);
        for column in [a_column, b_column] {
            select(&mut bus, !(1 << column));
            let rows = bus.read(GPIO + IN, 4).unwrap();
            if column == a_column {
                a_closed.push(rows & 0x20 == 0);
            }
        }
    }
    assert!(a_closed.iter().any(|&closed| closed));
    assert!(!a_closed[0], "the click starts from the detent");
    assert!(!encoders.busy(), "one click is one quadrature cycle");
    assert!(bus.devices.gpio.column_scans(a_column) >= 5 * SCANS_PER_PHASE);
}

#[test]
#[ignore = "requires FELUCCA_FWSC; run in release mode with --ignored"]
fn felucca_follows_a_turned_encoder() {
    let path =
        env::var("FELUCCA_FWSC").expect("set FELUCCA_FWSC to the published firmware package");
    let firmware = Firmware::load(Path::new(&path)).unwrap();
    let mut cpu = Cpu::new(firmware.bus().unwrap(), firmware.entry);
    cpu.r[0] = 0x01c7fe08;
    let mut encoders = Encoders::default();
    let run = |cpu: &mut Cpu, encoders: &mut Encoders, instructions: u64| {
        for i in 0..instructions {
            if i % 1024 == 0 {
                encoders.drive(&mut cpu.bus.devices.gpio);
            }
            cpu.step()
                .unwrap_or_else(|error| panic!("after {} instructions: {error}", cpu.steps));
        }
    };
    run(&mut cpu, &mut encoders, 900_000_000);
    assert!(cpu.bus.screen_visible());
    let before = cpu.bus.lcd.pixels.clone();
    encoders.turn(knob::PRESETS, 2);
    run(&mut cpu, &mut encoders, 600_000_000);
    assert!(
        !encoders.busy(),
        "the guest scanned every phase of both clicks"
    );
    let changed = before
        .iter()
        .zip(&cpu.bus.lcd.pixels)
        .filter(|(a, b)| a != b)
        .count();
    eprintln!("PRESETS +2: {changed} of 57600 LCD pixels changed");
    assert!(changed > 100, "the preset page must redraw");
}
