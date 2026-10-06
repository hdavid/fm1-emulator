// SPDX-License-Identifier: GPL-3.0-only
// Headless check that a firmware reacts to a turned encoder: boot, save the
// LCD, play `detents` quadrature clicks into encoder KNOB exactly as fm1-ui
// does (the headless player drives the same encoder model), save the LCD
// again and report how many pixels changed.
//   knob_check FIRMWARE KNOB DETENTS OUT_PREFIX   (KNOB: SELECT ALGORITHM PRESETS KNOB1..KNOB4)
// FM1_CPU_MHZ=N sets the instruction clock (default: the firmware's).
use fm1_emu::{
    player::{knob, Player},
    png,
};
use std::{env, path::Path};

const BOOT_SECONDS: f64 = 4.0;
const SETTLE_SECONDS: f64 = 3.0; // for the clicks to play and draw

fn save(player: &Player, path: &str) -> Result<(), String> {
    let bytes = png::encode_rgb(240, 240, &player.cpu.bus.lcd.pixels)?;
    std::fs::write(path, bytes).map_err(|error| format!("{path}: {error}"))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [firmware, encoder, detents, prefix] = args.as_slice() else {
        return Err("usage: knob_check FIRMWARE KNOB DETENTS OUT_PREFIX".into());
    };
    let encoder = knob(encoder)?;
    let detents: i32 = detents.parse().map_err(|_| "DETENTS is an integer")?;
    let mut player = Player::boot(Path::new(firmware), 8)?;
    if let Ok(mhz) = env::var("FM1_CPU_MHZ") {
        player
            .cpu
            .set_cpu_mhz(mhz.parse().map_err(|_| "invalid FM1_CPU_MHZ")?)?;
    }
    player.run_seconds(BOOT_SECONDS)?;
    player.cpu.bus.start_mmio_stats();
    player.run_seconds(1.0)?;
    if let Some(stats) = player.cpu.bus.take_mmio_stats() {
        for port in [0x50004u32, 0x50044] {
            let reads: u64 = stats
                .iter()
                .filter(|((a, _), _)| *a == port)
                .map(|(_, c)| c[0])
                .sum();
            println!("GPIO IN 0x{port:x}: {reads} reads in 1 s of guest time");
        }
    }
    let before = player.cpu.bus.lcd.pixels.clone();
    save(&player, &format!("{prefix}-before.png"))?;
    player.encoders.turn(encoder, detents);
    player.run_seconds(SETTLE_SECONDS)?;
    save(&player, &format!("{prefix}-after.png"))?;
    let changed = before
        .iter()
        .zip(&player.cpu.bus.lcd.pixels)
        .filter(|(a, b)| a != b)
        .count();
    println!("{changed} LCD pixels changed after {detents} detents");
    Ok(())
}
