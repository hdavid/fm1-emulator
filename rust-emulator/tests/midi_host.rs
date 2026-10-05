// SPDX-License-Identifier: GPL-3.0-only
// The USB host model as a MIDI host: it enumerates the firmware, opens its
// MIDI streaming endpoints and moves USB-MIDI packets both ways. Full
// firmware is external: set MIDI_FWSC to a Felucca, Jangada or SLOOP package
// and run in release mode with --ignored. The firmware runs at a 24 MHz
// instruction clock, so the step counts below are seconds of guest time
// divided by 24 million.
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    firmware::Firmware,
    usb_midi::{encode, Decoder},
};
use std::{env, path::Path};

fn run(cpu: &mut Cpu, steps: u64) {
    for _ in 0..steps {
        cpu.step()
            .unwrap_or_else(|error| panic!("after {} instructions: {error}", cpu.steps));
    }
}

fn boot() -> Cpu {
    let path = env::var("MIDI_FWSC").expect("set MIDI_FWSC to a Felucca-family package");
    let firmware = Firmware::load(Path::new(&path)).unwrap();
    let mut bus = firmware.bus().unwrap();
    bus.usb.enable_midi_host();
    bus.set_instruction_clock(Some(24_000_000));
    let mut cpu = Cpu::new(bus, firmware.entry);
    cpu.r[0] = 0x01c7fe08;
    run(&mut cpu, 100_000_000);
    cpu
}

fn received(cpu: &mut Cpu, decoder: &mut Decoder) -> Vec<Vec<u8>> {
    let packets: Vec<_> = cpu.bus.usb.midi_received.drain(..).collect();
    packets
        .into_iter()
        .filter_map(|p| decoder.push(p))
        .collect()
}

#[test]
fn the_midi_host_is_off_by_default_and_ready_only_after_enumeration() {
    let mut bus = Bus::new(vec![0; 64]).unwrap();
    assert!(!bus.usb.midi_ready());
    bus.usb.enable_midi_host();
    assert!(!bus.usb.midi_ready());
    assert_eq!(bus.usb.product(), None);
    // Packets wait in the host until the device has configured its endpoints.
    bus.usb_midi_send(&encode(&[0x90, 60, 100], 0));
    assert_eq!(bus.usb.midi_pending(), 1);
}

#[test]
#[ignore = "requires MIDI_FWSC; run in release mode with --ignored"]
fn a_note_on_from_the_host_makes_the_firmware_sound() {
    let mut cpu = boot();
    assert!(
        cpu.bus.usb.midi_ready(),
        "the host must have opened the MIDI endpoints"
    );
    assert_eq!(cpu.bus.usb.product(), Some("Felucca"));
    cpu.bus.audio.samples.clear();
    run(&mut cpu, 5_000_000);
    let silent = cpu.bus.audio.samples.iter().all(|f| *f == [0, 0]);
    assert!(silent, "no note was played yet");
    cpu.bus.usb_midi_send(&encode(&[0x90, 60, 110], 0));
    run(&mut cpu, 10_000_000);
    assert_eq!(
        cpu.bus.usb.midi_pending(),
        0,
        "the firmware took the packet"
    );
    let peak = cpu
        .bus
        .audio
        .samples
        .iter()
        .flat_map(|f| f.iter().map(|s| s.unsigned_abs()))
        .max()
        .unwrap_or(0);
    assert!(peak > 500, "a MIDI note-on must render audio (peak {peak})");
}

#[test]
#[ignore = "requires MIDI_FWSC; run in release mode with --ignored"]
fn an_editor_sysex_request_gets_its_reply_back_over_usb() {
    let mut cpu = boot();
    let mut decoder = Decoder::default();
    received(&mut cpu, &mut decoder);
    // Felucca editor protocol: F0 7D 46 4C <cmd 1 = INFO> F7
    cpu.bus
        .usb_midi_send(&encode(&[0xF0, 0x7D, 0x46, 0x4C, 1, 0xF7], 0));
    let mut replies = Vec::new();
    for _ in 0..40 {
        run(&mut cpu, 1_000_000);
        replies.extend(received(&mut cpu, &mut decoder));
        if replies
            .iter()
            .any(|m| m.starts_with(&[0xF0, 0x7D, 0x46, 0x4C, 1]))
        {
            break;
        }
    }
    let info = replies
        .iter()
        .find(|m| m.starts_with(&[0xF0, 0x7D, 0x46, 0x4C, 1]))
        .unwrap_or_else(|| panic!("no INFO reply; got {replies:02x?}"));
    assert_eq!(info.last(), Some(&0xF7));
    assert!(info.len() > 20, "INFO carries a version and engine names");
}
