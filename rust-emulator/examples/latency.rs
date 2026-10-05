// SPDX-License-Identifier: GPL-3.0-only
// Input-to-audio latency of a Felucca-family firmware, in guest time (exact,
// independent of the host): an event is injected at a chosen oscillator tick,
// and the time is the tick at which the audio DMA transfers the first frame
// that rises out of silence (|sample| >= threshold). Every trial lands at a
// different point of the 256-frame half buffer (a pseudo-random offset).
//
//   cargo run --release --example latency -- FWSC MODE [--cpu MHz] [--trials N] [--bpm B] [--jitter MS]
//   MODE  key   a note key (matrix 3:4) on the drum track
//         usb   USB-MIDI note-on, channel 10 (the drum track), note 36
//         trs   the same note-on as 3 bytes on the TRS MIDI IN (UART1, 31250 baud);
//               the time runs from the end of the last byte (when it is received)
//   Firmware with MIDI clock (SLOOP clock_sync.c; GLO > SYSTEM SYNC, global 13): a hat on every
//   16th of the drum pattern, the transport run by
//         ext-usb / ext-trs  an external clock (F8 at --bpm, +- --jitter ms uniform; 2 beats of
//               pre-roll, FA, --trials beats; with --ramp B2 the tempo goes to B2 over the 2nd
//               half): the onset of each hit - the arrival of the F8 of its step
//         out   SYNC OUT, PLAY pressed: the F8 the firmware sends (USB endpoint write) - the
//               onset of the hit of its step
//
// The DAC itself (codec digital filter, analog path) is not modelled: these
// are times to the I2S/DMA transfer.
use fm1_emu::{cpu::Cpu, firmware::Firmware, usb_midi::encode};
use std::{env, path::Path};

pub const TICKS_PER_MS: f64 = 24_000.0;
const THRESHOLD: i32 = 1 << 12; // 24-bit output: about -66 dBFS
const QUIET: u32 = 441; // 10 ms below threshold / 8 before an onset counts

pub struct Rig {
    pub cpu: Cpu,
    seed: u64,
}

impl Rig {
    pub fn boot(path: &str, mhz: u32) -> Rig {
        let firmware = Firmware::load(Path::new(path)).unwrap();
        let mut bus = firmware.bus().unwrap();
        bus.usb.enable_midi_host();
        let mut cpu = Cpu::new(bus, firmware.entry);
        cpu.r[0] = 0x01c7fe08;
        cpu.set_cpu_mhz(mhz).unwrap();
        let mut rig = Rig {
            cpu,
            seed: 0x9e3779b97f4a7c15,
        };
        rig.run_ms(2500.0);
        assert!(rig.cpu.bus.usb.midi_ready(), "USB MIDI not enumerated");
        rig
    }
    pub fn now(&self) -> u64 {
        self.cpu.bus.now()
    }
    /// Run until guest tick `t` (exactly: the last call steps one tick's instructions).
    pub fn run_until(&mut self, t: u64) {
        while self.now() < t {
            let left = (t - self.now()) * self.cpu.instructions_per_tick as u64;
            self.cpu.run_steps(left.clamp(1, 4_000_000)).unwrap();
        }
    }
    pub fn run_ms(&mut self, ms: f64) {
        let t = self.now() + (ms * TICKS_PER_MS) as u64;
        self.run_until(t);
    }
    pub fn random(&mut self, n: u64) -> u64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed % n
    }
    pub fn sysex(&mut self, bytes: &[u8]) {
        self.cpu.bus.usb_midi_send(&encode(bytes, 0));
        self.run_ms(60.0);
    }
    /// Editor SET of a global parameter (web/EDITOR_PROTOCOL.md: F0 7D 46 4C 03 1 id v14 F7).
    pub fn set_global(&mut self, id: u8, v: i32) {
        let u = (v + 8192) as u32;
        self.sysex(&[
            0xF0,
            0x7D,
            0x46,
            0x4C,
            3,
            1,
            id,
            (u & 127) as u8,
            (u >> 7) as u8,
            0xF7,
        ]);
    }
    pub fn arm(&mut self) {
        self.cpu.bus.audio.probe.arm(THRESHOLD, QUIET);
    }
    pub fn onsets(&self) -> &[(u64, u64)] {
        &self.cpu.bus.audio.probe.onsets
    }
}

pub struct Stats {
    pub v: Vec<f64>,
}
impl Stats {
    pub fn line(&self, name: &str) -> String {
        let n = self.v.len() as f64;
        let mean = self.v.iter().sum::<f64>() / n;
        let min = self.v.iter().cloned().fold(f64::MAX, f64::min);
        let max = self.v.iter().cloned().fold(f64::MIN, f64::max);
        let sd = (self.v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
        format!(
            "{name:<34} n={:<4} min {min:7.3}  mean {mean:7.3}  max {max:7.3}  sd {sd:6.3} ms",
            self.v.len()
        )
    }
}

#[allow(dead_code)]
fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: latency FWSC key|usb|trs [--cpu MHz] [--trials N]");
        std::process::exit(2);
    }
    let opt = |name: &str, def: u32| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(|v| v.parse().unwrap())
            .unwrap_or(def)
    };
    let (mhz, trials) = (opt("--cpu", 96), opt("--trials", 100));
    let mode = args[2].as_str();
    let mut rig = Rig::boot(&args[1], mhz);
    if mode.starts_with("ext") || mode == "out" {
        let f = |name: &str, def: f64| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .map(|v| v.parse().unwrap())
                .unwrap_or(def)
        };
        clock(
            &mut rig,
            mode,
            trials,
            f("--bpm", 120.0),
            f("--ramp", 0.0),
            f("--jitter", 0.0),
        );
        return;
    }
    rig.set_global(26, 0); // G_DRREV: no reverb tail between trials
    rig.sysex(&[0xF0, 0x7D, 0x46, 0x4C, 27, 3, 0xF7]); // ED_TRACK: select the drum track
    rig.arm();
    rig.run_ms(300.0);
    let mut lat = Stats { v: vec![] };
    let mut phase = Stats { v: vec![] };
    for _ in 0..trials {
        let t = rig.now() + 24_000 + rig.random(2 * 256 * 24_000_000 / 44_100);
        rig.run_until(t);
        let before = rig.onsets().len();
        let t0 = match mode {
            "key" => {
                rig.cpu.bus.devices.gpio.press(3, 4, true).unwrap();
                rig.now()
            }
            "usb" => {
                rig.cpu.bus.usb_midi_send(&encode(&[0x99, 36, 110], 0));
                rig.now()
            }
            "trs" => {
                rig.cpu.bus.uart_midi_send(&[0x99, 36, 110]);
                rig.now() + 3 * fm1_emu::uart1::BYTE_TICKS
            }
            _ => panic!("mode {mode}"),
        };
        let deadline = t0 + 60 * 24_000;
        while rig.onsets().len() == before && rig.now() < deadline {
            rig.run_ms(0.5);
        }
        let Some(&(frame, tick)) = rig.onsets().get(before) else {
            panic!("no onset within 60 ms of the event at tick {t0}");
        };
        lat.v.push((tick - t0) as f64 / TICKS_PER_MS);
        // where the event fell in the half buffer being played (0..256 frames)
        let halves = &rig.cpu.bus.audio.probe.halves;
        if let Some(&(_, ht)) = halves.iter().rev().find(|(_, ht)| *ht <= t0) {
            phase.v.push((t0 - ht) as f64 / TICKS_PER_MS);
        }
        let _ = frame;
        match mode {
            "key" => rig.cpu.bus.devices.gpio.press(3, 4, false).unwrap(),
            "usb" => rig.cpu.bus.usb_midi_send(&encode(&[0x89, 36, 0], 0)),
            _ => rig.cpu.bus.uart_midi_send(&[0x89, 36, 0]),
        }
        rig.run_ms(700.0);
    }
    println!("{mode} at {mhz} MHz:");
    println!("{}", lat.line("event -> first DMA frame"));
    println!("{}", phase.line("event offset into the playing half"));
    let mut pairs: Vec<(f64, f64)> = phase.v.iter().cloned().zip(lat.v.iter().cloned()).collect();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let fit = pairs.iter().map(|(p, l)| l + p).collect::<Vec<_>>();
    println!(
        "{}",
        Stats { v: fit }.line("latency + offset (should be flat)")
    );
}

/// Drum steps 0..15: a closed hat (lane 6) on each; no drum reverb.
fn hats(rig: &mut Rig) {
    rig.set_global(26, 0);
    for i in 0..16u8 {
        // ED_DRUM_STEP (33): index, on (3 x 7 bits), lvl (5), rat (5)
        let mut m = vec![0xF0, 0x7D, 0x46, 0x4C, 33, i, 1 << 6, 0, 0];
        m.extend([0u8; 10]);
        m.push(0xF7);
        rig.sysex(&m);
    }
}

fn clock(rig: &mut Rig, mode: &str, beats: u32, bpm: f64, ramp: f64, jitter: f64) {
    let mhz = rig.cpu.instructions_per_tick * 24;
    hats(rig);
    rig.arm();
    let mut seed = 12345u64;
    let mut jit = |ms: f64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ((seed % 2_000_001) as f64 / 1e6 - 1.0) * ms * TICKS_PER_MS
    };
    if mode == "out" {
        rig.set_global(13, 1); // SYNC OUT
        rig.run_ms(200.0);
        rig.cpu.bus.usb.midi_received.clear();
        rig.cpu.bus.usb.midi_received_ticks.clear();
        let stop = rig.onsets().len();
        rig.cpu.bus.devices.gpio.press(10, 2, true).unwrap(); // PLAY (matrix id 12, SLOOP panel default)
        rig.run_ms(60.0);
        rig.cpu.bus.devices.gpio.press(10, 2, false).unwrap();
        rig.run_ms(beats as f64 * 667.0); // the firmware tempo: 90 BPM, a beat is 667 ms
        let ticks: Vec<(u64, u8)> = rig
            .cpu
            .bus
            .usb
            .midi_received
            .iter()
            .zip(rig.cpu.bus.usb.midi_received_ticks.iter())
            .map(|(p, t)| (*t, p[1]))
            .collect();
        let fa = ticks
            .iter()
            .position(|(_, b)| *b == 0xFA)
            .expect("no FA sent");
        let f8: Vec<u64> = ticks[fa..]
            .iter()
            .filter(|(_, b)| *b == 0xF8)
            .map(|(t, _)| *t)
            .collect();
        let on: Vec<u64> = rig.onsets()[stop..].iter().map(|(_, t)| *t).collect();
        let mut e = vec![];
        for t in f8.iter().step_by(6) {
            if let Some(o) = on
                .iter()
                .min_by_key(|o| (**o as i64 - *t as i64).unsigned_abs())
            {
                let d = (*t as f64 - *o as f64) / TICKS_PER_MS;
                if d.abs() < 20.0 {
                    e.push(d);
                }
            }
        }
        println!(
            "out at {mhz} MHz, firmware tempo (90 BPM unless set): {} F8 after FA, {} onsets",
            f8.len(),
            on.len()
        );
        if f8.len() > 24 {
            let iv: Vec<f64> = f8
                .windows(2)
                .map(|w| (w[1] - w[0]) as f64 / TICKS_PER_MS)
                .collect();
            println!("{}", Stats { v: iv.clone() }.line("F8 interval"));
            if env::var("DUMP").is_ok() {
                println!("{:?}", &iv[..60]);
                let oi: Vec<f64> = on
                    .windows(2)
                    .map(|w| (w[1] - w[0]) as f64 / TICKS_PER_MS)
                    .collect();
                println!("onset intervals {:?}", oi);
                let rel: Vec<f64> = f8
                    .iter()
                    .map(|t| {
                        (*t as f64 - f8[0] as f64) / TICKS_PER_MS
                            - 27.777777
                                * ((*t as f64 - f8[0] as f64) / TICKS_PER_MS / 27.777777).round()
                    })
                    .collect();
                println!("F8 phase vs grid {:?}", &rel[..60]);
            }
        }
        println!("{}", Stats { v: e }.line("F8 sent - onset of its step"));
        return;
    }
    rig.set_global(13, if mode == "ext-usb" { 2 } else { 3 });
    rig.run_ms(100.0);
    let usb = mode == "ext-usb";
    let send = |rig: &mut Rig, b: &[u8]| {
        if usb {
            rig.cpu.bus.usb_midi_send(&encode(b, 0));
        } else {
            rig.cpu.bus.uart_midi_send(b);
        }
    };
    // tick times (ideal), arrival = ideal + jitter (+ the byte time on TRS)
    let n = 48 + beats as usize * 24;
    let mut ideal = vec![];
    let mut t = rig.now() as f64 + 10.0 * TICKS_PER_MS;
    for k in 0..n {
        ideal.push(t);
        let b = if ramp > 0.0 && k >= 48 + beats as usize * 12 {
            let f = ((k - 48 - beats as usize * 12) as f64 / (beats as f64 * 12.0)).min(1.0);
            bpm + (ramp - bpm) * f
        } else {
            bpm
        };
        t += 60.0 / (24.0 * b) * 24e6;
    }
    let byte = fm1_emu::uart1::BYTE_TICKS as f64;
    let mut arrival = vec![];
    let start = rig.onsets().len();
    for (k, ideal_t) in ideal.iter().enumerate() {
        let at = ideal_t + jit(jitter);
        if k == 48 {
            // FA just before the downbeat F8
            rig.run_until((at - if usb { 0.1 * TICKS_PER_MS } else { byte }) as u64 - 1);
            send(rig, &[0xFA]);
        }
        rig.run_until(at as u64);
        send(rig, &[0xF8]);
        arrival.push(if usb {
            rig.now() as f64
        } else {
            rig.now() as f64 + byte
        });
    }
    rig.run_ms(30.0);
    let on: Vec<u64> = rig.onsets()[start..].iter().map(|(_, t)| *t).collect();
    let mut e_arr = vec![];
    let mut e_ideal = vec![];
    let mut first = None;
    for (j, k) in (48..n).step_by(6).enumerate() {
        let near = on
            .iter()
            .min_by_key(|o| (**o as i64 - arrival[k] as i64).unsigned_abs());
        if let Some(o) = near {
            let d = (*o as f64 - arrival[k]) / TICKS_PER_MS;
            if d.abs() > 20.0 {
                continue;
            }
            if j == 0 {
                first = Some(d);
                continue;
            }
            e_arr.push(d);
            e_ideal.push((*o as f64 - ideal[k] - if usb { 0.0 } else { byte }) / TICKS_PER_MS);
        }
    }
    println!(
        "{mode} at {mhz} MHz, {bpm} BPM{}{}: {} steps, {} onsets, the first hit {:+.3} ms after its F8",
        if ramp > 0.0 { format!(" -> {ramp}") } else { String::new() },
        if jitter > 0.0 { format!(" +-{jitter} ms") } else { String::new() },
        (n - 48) / 6,
        on.len(),
        first.unwrap_or(f64::NAN)
    );
    println!("{}", Stats { v: e_arr }.line("onset - F8 arrival"));
    println!("{}", Stats { v: e_ideal }.line("onset - F8 ideal time"));
}
