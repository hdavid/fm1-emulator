// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    player::{Player, OSCILLATOR_HZ},
    profile::Profile,
};
use std::path::PathBuf;

fn display() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/display/firmware.elf")
}

#[test]
fn guest_seconds_follow_the_oscillator_at_any_instruction_clock() {
    for mhz in [None, Some(24_000_000)] {
        let mut player = Player::boot(&display(), 8).unwrap();
        player.cpu.bus.set_instruction_clock(mhz);
        player.run_seconds(0.01).unwrap();
        let ticks = player.cpu.bus.oscillator_ticks();
        assert!(ticks >= (0.01 * OSCILLATOR_HZ) as u64, "{mhz:?}: {ticks}");
        assert!(
            ticks < (0.01 * OSCILLATOR_HZ) as u64 + 100,
            "{mhz:?}: {ticks}"
        );
        if mhz.is_some() {
            assert_eq!(player.cpu.steps, ticks, "24 MHz: one tick per instruction");
        }
    }
}

#[test]
fn the_profile_counts_every_primary_core_instruction() {
    let mut player = Player::boot(&display(), 8).unwrap();
    player.profile = Some(Profile::new());
    player.run(10_000).unwrap();
    let profile = player.profile.as_ref().unwrap();
    assert_eq!(profile.total, 10_000);
    let counted: u64 = profile.pcs().iter().map(|(_, count)| count).sum();
    assert_eq!(counted + profile.idle, 10_000);
}

#[test]
fn a_fault_reports_the_last_instructions() {
    let mut player = Player::boot(&display(), 4).unwrap();
    player.run(1000).unwrap();
    player.cpu.pc = 0; // Not executable: the next step faults.
    let report = player.run(1).unwrap_err();
    assert_eq!(report.lines().count(), 5, "{report}");
    assert!(report
        .lines()
        .last()
        .unwrap()
        .starts_with("fault after 1000 instructions"));
}
