// SPDX-License-Identifier: GPL-3.0-only
// Headless playback script for reproducing what a user does in fm1-ui:
//   play_check FIRMWARE STEP [STEP...]
// Steps: run:SECONDS (guest time), turn:KNOB:DETENTS (SELECT ALGORITHM
// PRESETS KNOB1..KNOB4), hold:ID,ID.. / release (matrix key ids, notes 14..40,
// as fm1-ui's KEYMAP), png:PATH. Stops on a guest fault and prints the last
// instructions with registers (PLAY_TRACE=N for the last N, default 40).
#[path = "../src/ui_knobs.rs"]
mod ui_knobs;
use fm1_emu::{cpu::Cpu, firmware::Firmware, png};
use std::{collections::VecDeque, env, path::Path};

// Matrix IDs per (row - 1, column), from fm1_input.h; same table as fm1-ui.
const KEYMAP: [[i8; 11]; 4] = [
    [5, 11, 4, 10, 3, 9, 2, 8, -1, -1, -1],
    [34, 35, 36, 37, 38, 40, 39, 13, 7, 6, 12],
    [23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33],
    [0, 1, 15, 14, 17, 16, 19, 18, 20, 21, 22],
];

struct Player {
    cpu: Cpu,
    encoders: ui_knobs::Encoders,
    held: [bool; 41],
    recent: VecDeque<(u32, &'static str, [u32; 16])>,
    trace: usize,
}

impl Player {
    fn run(&mut self, steps: u64) -> Result<(), String> {
        let end = self.cpu.steps + steps;
        while self.cpu.steps < end {
            if self.cpu.steps % 1024 == 0 {
                let gpio = &mut self.cpu.bus.devices.gpio;
                for (row, ids) in KEYMAP.iter().enumerate() {
                    for (column, &id) in ids.iter().enumerate() {
                        if id >= 0 {
                            gpio.press(column, row + 1, self.held[id as usize])?;
                        }
                    }
                }
                let contacts = self.encoders.update(|column| gpio.column_scans(column));
                for (e, (a, b)) in contacts.into_iter().enumerate() {
                    let [ac, ar, bc, br] = ui_knobs::CONTACTS[e];
                    gpio.press(ac, ar, a)?;
                    gpio.press(bc, br, b)?;
                }
            }
            let pc = self.cpu.pc;
            match self.cpu.step() {
                Ok(op) => {
                    if self.recent.len() >= self.trace {
                        self.recent.pop_front();
                    }
                    self.recent.push_back((pc, op, self.cpu.r));
                }
                Err(fault) => {
                    for (pc, op, r) in &self.recent {
                        let regs: Vec<_> = r.iter().map(|v| format!("{v:x}")).collect();
                        println!("  0x{pc:08x} {op:<24} [{}]", regs.join(" "));
                    }
                    return Err(format!("fault after {} instructions: {fault}", self.cpu.steps));
                }
            }
        }
        Ok(())
    }
}

fn knob(name: &str) -> Result<usize, String> {
    use ui_knobs::knob::*;
    Ok(match name {
        "SELECT" => SELECT,
        "ALGORITHM" => ALGORITHM,
        "PRESETS" => PRESETS,
        "KNOB1" => KNOB1,
        "KNOB2" => KNOB1 + 1,
        "KNOB3" => KNOB1 + 2,
        "KNOB4" => KNOB1 + 3,
        _ => return Err(format!("unknown knob {name}")),
    })
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let (firmware, steps) = args.split_first().ok_or("usage: play_check FIRMWARE STEP...")?;
    let firmware = Firmware::load(Path::new(firmware))?;
    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    let mut player = Player {
        cpu,
        encoders: ui_knobs::Encoders::default(),
        held: [false; 41],
        recent: VecDeque::new(),
        trace: env::var("PLAY_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(40),
    };
    for step in steps {
        let parts: Vec<&str> = step.split(':').collect();
        match parts.as_slice() {
            ["run", seconds] => {
                let seconds: f64 = seconds.parse().map_err(|_| format!("bad {step}"))?;
                player.run((seconds * 24e6) as u64)?;
            }
            ["turn", name, detents] => {
                let detents: i32 = detents.parse().map_err(|_| format!("bad {step}"))?;
                player.encoders.turn(knob(name)?, detents);
            }
            ["hold", ids] => {
                for id in ids.split(',') {
                    let id: usize = id.parse().map_err(|_| format!("bad {step}"))?;
                    *player.held.get_mut(id).ok_or(format!("bad key {id}"))? = true;
                }
            }
            ["release"] => player.held = [false; 41],
            ["level", seconds] => {
                let seconds: f64 = seconds.parse().map_err(|_| format!("bad {step}"))?;
                player.cpu.bus.audio.samples.clear();
                player.run((seconds * 24e6) as u64)?;
                let samples = &player.cpu.bus.audio.samples;
                let (mut sum, mut peak) = (0f64, 0i64);
                for [l, r] in samples.iter() {
                    for v in [*l as i64, *r as i64] {
                        sum += (v as f64 / 8_388_608.0).powi(2);
                        peak = peak.max(v.abs());
                    }
                }
                let rms = (sum / (samples.len().max(1) * 2) as f64).sqrt();
                println!("  audio: {} frames, rms {:.4}, peak {:.4} (full scale 1.0)", samples.len(), rms, peak as f64 / 8_388_608.0);
            }
            ["png", path] => {
                let bytes = png::encode_rgb(240, 240, &player.cpu.bus.lcd.pixels)?;
                std::fs::write(path, bytes).map_err(|error| format!("{path}: {error}"))?;
            }
            _ => return Err(format!("unknown step {step}")),
        }
        println!("{step}: ok ({} instructions)", player.cpu.steps);
    }
    Ok(())
}
