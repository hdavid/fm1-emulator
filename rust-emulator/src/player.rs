// SPDX-License-Identifier: GPL-3.0-only
// Headless "user at the panel" for play_check and preset_sweep: boots a
// firmware, holds matrix keys, turns encoders (the same model as fm1-ui) and
// keeps the last instructions for a fault report.
use crate::{
    cpu::Cpu,
    encoders::{self, Encoders},
    firmware::Firmware,
    profile::Profile,
};
use std::{collections::VecDeque, path::Path};

/// Oscillator ticks per second of guest time.
pub const OSCILLATOR_HZ: f64 = 24e6;

/// Instructions between two scans of the held keys and turned encoders.
const PANEL_SCAN_STEPS: u64 = 1024;

// Matrix IDs per (row - 1, column), from fm1_input.h; same table as fm1-ui.
const KEYMAP: [[i8; 11]; 4] = [
    [5, 11, 4, 10, 3, 9, 2, 8, -1, -1, -1],
    [34, 35, 36, 37, 38, 40, 39, 13, 7, 6, 12],
    [23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33],
    [0, 1, 15, 14, 17, 16, 19, 18, 20, 21, 22],
];

pub struct Player {
    pub cpu: Cpu,
    pub encoders: Encoders,
    pub held: [bool; 41],
    /// When set, every primary-core instruction run by `run` is counted.
    pub profile: Option<Profile>,
    recent: VecDeque<(u32, &'static str, [u32; 16])>,
    trace: usize,
    /// End of the package's code area in the NOR (`Firmware::code_end`).
    pub code_end: usize,
}

/// Audio level of the guest's output since the last `Level::take`.
#[derive(Clone, Copy, Default)]
pub struct Level {
    pub frames: usize,
    pub sum: f64,
    pub peak: f64,
}

impl Level {
    pub fn rms(&self) -> f64 {
        (self.sum / (self.frames.max(1) * 2) as f64).sqrt()
    }

    /// Add the samples the guest produced (24-bit full scale) and clear them.
    pub fn take(&mut self, cpu: &mut Cpu) {
        let samples = &mut cpu.bus.audio.samples;
        for [l, r] in samples.iter() {
            for v in [*l, *r] {
                let v = v as f64 / 8_388_608.0;
                self.sum += v * v;
                self.peak = self.peak.max(v.abs());
            }
        }
        self.frames += samples.len();
        samples.clear();
    }
}

impl Player {
    pub fn boot(firmware: &Path, trace: usize) -> Result<Self, String> {
        let firmware = Firmware::load(firmware)?;
        let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
        cpu.r[0] = 0x01c7_fe08;
        Ok(Self {
            cpu,
            encoders: Encoders::default(),
            held: [false; 41],
            profile: None,
            recent: VecDeque::new(),
            trace,
            code_end: firmware.code_end(),
        })
    }

    /// Run `steps` instructions. A fault returns its message with the last
    /// instructions (PC, form, registers after it) as a trace.
    pub fn run(&mut self, steps: u64) -> Result<(), String> {
        let end = self.cpu.steps + steps;
        while self.cpu.steps < end {
            self.scan_panel()?;
            // Two instructions per call while the secondary core runs.
            let cores = 1 + u64::from(self.cpu.secondary_pc().is_some());
            let calls = (end - self.cpu.steps).div_ceil(cores);
            self.run_calls(calls.min(PANEL_SCAN_STEPS))?;
        }
        Ok(())
    }

    /// `calls` calls of `Cpu::step`, jumping over halted spans
    /// (`Cpu::skip_idle_calls`); instructions that issue are stepped one
    /// by one for the profile and the fault trace.
    fn run_calls(&mut self, calls: u64) -> Result<(), String> {
        // The panel scan before this changed device state directly.
        self.cpu.bus.devices_changed();
        let mut done = 0;
        while done < calls {
            if self.cpu.may_skip() {
                let skipped = self.cpu.skip_idle_calls(calls - done);
                if skipped > 0 {
                    if let Some(profile) = self.profile.as_mut() {
                        profile.record_idle(skipped);
                    }
                    done += skipped;
                    continue;
                }
            }
            self.step()?;
            done += 1;
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), String> {
        let pc = self.cpu.pc;
        if let Some(profile) = self.profile.as_mut() {
            if self.cpu.halted() {
                profile.record_idle(1);
            } else {
                profile.record(pc, self.cpu.in_interrupt());
            }
        }
        match self.cpu.step_next() {
            Ok(op) => {
                if self.recent.len() >= self.trace {
                    self.recent.pop_front();
                }
                self.recent.push_back((pc, op, self.cpu.r));
                Ok(())
            }
            Err(fault) => {
                let mut report = String::new();
                for (pc, op, r) in &self.recent {
                    let regs: Vec<_> = r.iter().map(|v| format!("{v:x}")).collect();
                    report += &format!("  0x{pc:08x} {op:<24} [{}]\n", regs.join(" "));
                }
                report += &format!("fault after {} instructions: {fault}", self.cpu.steps);
                Err(report)
            }
        }
    }

    /// Run `seconds` of guest time: oscillator ticks, whatever the
    /// instruction clock.
    pub fn run_seconds(&mut self, seconds: f64) -> Result<(), String> {
        let end = self.cpu.bus.oscillator_ticks() + (seconds * OSCILLATOR_HZ) as u64;
        while self.cpu.bus.oscillator_ticks() < end {
            self.scan_panel()?;
            // A call takes at least one 24 MHz tick's share of time; close
            // to `end`, take fewer calls between checks.
            let left = end - self.cpu.bus.oscillator_ticks();
            self.run_calls(left.clamp(1, PANEL_SCAN_STEPS))?;
        }
        Ok(())
    }

    /// Run `seconds` of guest time and measure the audio produced, in slices
    /// shorter than the one second the audio buffer keeps.
    pub fn level(&mut self, seconds: f64) -> Result<Level, String> {
        let mut level = Level::default();
        self.cpu.bus.audio.samples.clear();
        let slices = (seconds / 0.25).ceil().max(1.0) as u32;
        for _ in 0..slices {
            self.run_seconds(seconds / slices as f64)?;
            level.take(&mut self.cpu);
        }
        Ok(level)
    }

    /// Run until the encoders delivered every pending detent (at most
    /// `limit_seconds`).
    pub fn settle_encoders(&mut self, limit_seconds: f64) -> Result<(), String> {
        let slice = 0.01;
        let mut waited = 0.0;
        while self.encoders.busy() && waited < limit_seconds {
            self.run_seconds(slice)?;
            waited += slice;
        }
        Ok(())
    }

    fn scan_panel(&mut self) -> Result<(), String> {
        let gpio = &mut self.cpu.bus.devices.gpio;
        for (row, ids) in KEYMAP.iter().enumerate() {
            for (column, &id) in ids.iter().enumerate() {
                if id >= 0 {
                    gpio.press(column, row + 1, self.held[id as usize])?;
                }
            }
        }
        self.encoders.drive(gpio);
        Ok(())
    }
}

pub fn knob(name: &str) -> Result<usize, String> {
    use encoders::knob::*;
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
