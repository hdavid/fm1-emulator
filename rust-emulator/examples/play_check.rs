// SPDX-License-Identifier: GPL-3.0-only
// Headless playback script for reproducing what a user does in fm1-ui:
//   play_check FIRMWARE STEP [STEP...]
// Steps: run:SECONDS (guest time), turn:KNOB:DETENTS (SELECT ALGORITHM
// PRESETS KNOB1..KNOB4), hold:ID,ID.. / release (matrix key ids, notes 14..40,
// as fm1-ui's KEYMAP), level:SECONDS (run and print the audio level),
// png:PATH. Stops on a guest fault and prints the last instructions with
// registers (PLAY_TRACE=N for the last N, default 40).
use fm1_emu::{
    player::{knob, Player},
    png,
};
use std::{env, path::Path};

fn seconds(step: &str, text: &str) -> Result<f64, String> {
    text.parse().map_err(|_| format!("bad {step}"))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let (firmware, steps) = args
        .split_first()
        .ok_or("usage: play_check FIRMWARE STEP...")?;
    let trace = env::var("PLAY_TRACE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let mut player = Player::boot(Path::new(firmware), trace)?;
    // Optional: FM1_CPU_MHZ=N emulates an N MHz CPU (default 24, real time).
    if let Ok(mhz) = env::var("FM1_CPU_MHZ") {
        player.cpu.set_cpu_mhz(mhz.parse().map_err(|_| "invalid FM1_CPU_MHZ")?)?;
    }
    for step in steps {
        let parts: Vec<&str> = step.split(':').collect();
        let result = match parts.as_slice() {
            ["run", s] => player.run_seconds(seconds(step, s)?),
            ["turn", name, detents] => {
                let detents: i32 = detents.parse().map_err(|_| format!("bad {step}"))?;
                player.encoders.turn(knob(name)?, detents);
                Ok(())
            }
            ["hold", ids] => {
                for id in ids.split(',') {
                    let id: usize = id.parse().map_err(|_| format!("bad {step}"))?;
                    *player.held.get_mut(id).ok_or(format!("bad key {id}"))? = true;
                }
                Ok(())
            }
            ["release"] => {
                player.held = [false; 41];
                Ok(())
            }
            ["level", s] => player.level(seconds(step, s)?).map(|level| {
                println!(
                    "  audio: {} frames, rms {:.4}, peak {:.4} (full scale 1.0)",
                    level.frames,
                    level.rms(),
                    level.peak
                );
            }),
            ["png", path] => {
                let bytes = png::encode_rgb(240, 240, &player.cpu.bus.lcd.pixels)?;
                std::fs::write(path, bytes).map_err(|error| format!("{path}: {error}"))
            }
            _ => return Err(format!("unknown step {step}")),
        };
        if let Err(report) = result {
            // The trace goes to stdout; the fault line is the error.
            let (trace, fault) = report.rsplit_once('\n').unwrap_or(("", &report));
            println!("{trace}");
            return Err(fault.to_string());
        }
        println!("{step}: ok ({} instructions)", player.cpu.steps);
    }
    Ok(())
}
