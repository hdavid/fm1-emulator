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
//   Firmware with MIDI clock (SLOOP clock_sync.c; GLO > SYSTEM SYNC, global 13, set to --sync):
//   a closed hat on every beat (every 2nd above 145 BPM), the transport run by
//         ext-usb / ext-trs  an external clock: F8 at --bpm (+- --jitter ms uniform), 2 beats of
//               pre-roll, FA (--fa ms before the downbeat F8; default: just after the F8 before it,
//               a tick ahead, as hosts send it), --trials beats; --to B2 over --ramp beats from the
//               downbeat (--ramp 0: a jump); --drop K: F8 K lost; --spike K --spike-ms M: F8 K late.
//               Onset of each hit - the F8 of its beat (ideal time, and as it arrived)
//         transport / transport-trs  START a tick ahead, STOP after a bar, SPP 16th 136 + CONTINUE a
//               tick ahead (USB / TRS)
//   NEST=0: interrupts do not nest (the emulator's default; the firmware assumes they do).
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
    pub sym: std::collections::BTreeMap<String, u32>,
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
            sym: match env::var("SYMS") {
                Ok(elf) => Firmware::load(Path::new(&elf)).unwrap().symbols,
                Err(_) => firmware.symbols.clone(),
            },
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
    // interrupts nest (TIMER5 above ALNK0), as the firmware is written for; NEST=0: they wait
    rig.cpu.nested_irqs = env::var("NEST").map(|v| v != "0").unwrap_or(true);
    if mode.starts_with("ext") || mode.starts_with("transport") {
        let f = |name: &str, def: f64| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .map(|v| v.parse().unwrap())
                .unwrap_or(def)
        };
        let c = Clock {
            usb: mode != "ext-trs" && mode != "transport-trs",
            sync: f("--sync", 2.0) as i32,
            beats: trials,
            bpm: f("--bpm", 120.0),
            to: f("--to", 0.0),
            ramp: f("--ramp", 0.0),
            jitter: f("--jitter", 0.0),
            fa: f("--fa", -1.0),
            drop: f("--drop", 0.0) as usize,
            spike: f("--spike", 0.0) as usize,
            spike_ms: f("--spike-ms", 3.0),
        };
        if mode.starts_with("transport") {
            transport(&mut rig, &c);
        } else {
            clock(&mut rig, &c);
        }
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

pub struct Clock {
    usb: bool,
    sync: i32,
    beats: u32,
    bpm: f64,
    to: f64,
    ramp: f64,
    jitter: f64,
    fa: f64,
    drop: usize,
    spike: usize,
    spike_ms: f64,
}

/// A closed hat (lane 4) on steps 0, 4, 8, 12 (`every` 2: on 0 and 8; above 145 BPM), no reverb.
fn hats(rig: &mut Rig, every: u8) {
    rig.set_global(26, 0);
    for i in 0..16u8 {
        // ED_DRUM_STEP (33): index, on (3 x 7 bits), lvl (5), rat (5)
        let on = if i % (4 * every) == 0 { 1 << 4 } else { 0 };
        let mut m = vec![0xF0, 0x7D, 0x46, 0x4C, 33, i, on, 0, 0];
        m.extend([0u8; 10]);
        m.push(0xF7);
        rig.sysex(&m);
    }
}

fn line(name: &str, v: &[f64]) -> String {
    if v.is_empty() {
        return format!("{name:<26} (none)");
    }
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let p95 = a[(a.len() * 95 / 100).min(a.len() - 1)];
    let min = v.iter().cloned().fold(f64::MAX, f64::min);
    let max = v.iter().cloned().fold(f64::MIN, f64::max);
    format!(
        "{name:<26} n={:<4} mean {mean:+7.3}  p95 {p95:6.3}  min {min:+7.3}  max {max:+7.3} ms",
        v.len()
    )
}

struct Feed {
    seed: u64,
}
impl Feed {
    fn jit(&mut self, ms: f64) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        ((self.seed % 2_000_001) as f64 / 1e6 - 1.0) * ms * TICKS_PER_MS
    }
}
fn send(rig: &mut Rig, usb: bool, b: &[u8]) {
    if usb {
        rig.cpu.bus.usb_midi_send(&encode(b, 0));
    } else {
        rig.cpu.bus.uart_midi_send(b);
    }
}

fn clock(rig: &mut Rig, c: &Clock) {
    let mhz = rig.cpu.instructions_per_tick * 24;
    let every = if c.bpm > 145.0 || c.to > 145.0 { 2usize } else { 1 }; // (the hat rings ~0.4 s)
    hats(rig, every as u8);
    rig.set_global(13, c.sync);
    rig.cpu.bus.audio.probe.arm(2048, 220); // the hits: |x| >= 2048 (Q15 16) after 5 ms below 256
    rig.run_ms(100.0);
    let mut feed = Feed { seed: 12345 };
    let byte = fm1_emu::uart1::BYTE_TICKS as f64;
    let down = 48usize;
    let n = down + c.beats as usize * 24;
    let (mut ideal, mut arrival) = (vec![], vec![]);
    let mut t = rig.now() as f64 + 10.0 * TICKS_PER_MS;
    for k in 0..n {
        ideal.push(t);
        let b = if c.to > 0.0 && k >= down {
            let f = if c.ramp <= 0.0 {
                1.0
            } else {
                ((k - down) as f64 / (c.ramp * 24.0)).min(1.0)
            };
            c.bpm + (c.to - c.bpm) * f
        } else {
            c.bpm
        };
        t += 60.0 / (24.0 * b) * 24e6;
    }
    let start = rig.onsets().len();
    for k in 0..n {
        let mut at = ideal[k] + feed.jit(c.jitter);
        if k > 0 && k == c.spike {
            at += c.spike_ms * TICKS_PER_MS;
        }
        if k == down {
            let fa = if c.fa < 0.0 {
                arrival[k - 1] + 0.1 * TICKS_PER_MS
            } else {
                at - c.fa * TICKS_PER_MS
            };
            // (TRS: the FA byte must end before the F8 starts)
            let fa = if c.usb { fa } else { fa.min(at - 2.0 * byte) };
            rig.run_until(fa as u64);
            send(rig, c.usb, &[0xFA]);
        }
        if env::var("DBGSY").is_ok() && k + 2 >= down && k <= down + 1 {
            let f = |rig: &Rig, i: u32, size: usize| {
                rig.sym
                    .get(&format!("sy.{i}"))
                    .map(|a| rig.cpu.bus.read(*a, size).unwrap() as i64)
                    .unwrap_or(-1)
            };
            println!(
                "k {k} t {:.2} ms: n {} n0 {} pos {} have {} arm {} src {} stop {}",
                rig.now() as f64 / TICKS_PER_MS,
                f(rig, 5, 4),
                f(rig, 6, 4),
                f(rig, 7, 4),
                f(rig, 12, 1),
                f(rig, 17, 1),
                f(rig, 20, 1),
                f(rig, 18, 1)
            );
        }
        if env::var("DBGSY").is_ok() && k >= down && k <= down + 2 {
            while rig.now() + 24_000 < at as u64 {
                let g = |rig: &Rig, n: &str, sz: usize| {
                    rig.sym
                        .get(n)
                        .map(|a| rig.cpu.bus.read(*a, sz).unwrap() as i64)
                        .unwrap_or(-1)
                };
                println!(
                    "  t {:+.2} ms: beat {} pos {} out_t {:+.2} n {} n0 {} arm {} have {} tq {:+.3} (lo {:x} hi {:x}) p {:.3}",
                    (rig.now() as f64 - ideal[down]) / TICKS_PER_MS,
                    g(rig, "clk_beat", 4),
                    g(rig, "clk_pos", 4),
                    (g(rig, "sync_out_t", 4) as f64 - (ideal[down] as u64 as u32) as f64) / TICKS_PER_MS,
                    g(rig, "sy.5", 4),
                    g(rig, "sy.6", 4),
                    g(rig, "sy.17", 1),
                    g(rig, "sy.12", 1),
                    ((((g(rig, "sy.0", 4) as u64) >> 8) | ((g(rig, "sy.0", 4) as u64 >> 0) & 0) | (((rig.sym.get("sy.0").map(|a| rig.cpu.bus.read(*a + 4, 4).unwrap()).unwrap_or(0) as u64) << 24) & 0xffff_ffff)) as f64 - (ideal[down] as u64 as u32) as f64) / TICKS_PER_MS,
                    g(rig, "sy.0", 4),
                    rig.sym.get("sy.0").map(|a| rig.cpu.bus.read(*a + 4, 4).unwrap()).unwrap_or(0),
                    g(rig, "sy.1", 4) as f64 / 256.0 / TICKS_PER_MS
                );
                rig.run_ms(1.0);
            }
        }
        if let (Ok(w), true) = (env::var("PCW"), k == down) {
            let pcs: Vec<u32> = w.split(',').map(|v| u32::from_str_radix(v, 16).unwrap()).collect();
            let mut shown = 0;
            while rig.now() < at as u64 && shown < 12 {
                rig.cpu.step().unwrap();
                if pcs.contains(&rig.cpu.pc) {
                    println!("pc {:08x} r {:08x?} psr {:x}", rig.cpu.pc, rig.cpu.r, rig.cpu.sr[5]);
                    shown += 1;
                }
            }
        }
        rig.run_until(at as u64);
        if k == 0 || k != c.drop {
            send(rig, c.usb, &[0xF8]);
        }
        arrival.push(if c.usb {
            rig.now() as f64
        } else {
            rig.now() as f64 + byte
        });
    }
    rig.run_ms(30.0);
    let on: Vec<u64> = rig.onsets()[start..].iter().map(|(_, t)| *t).collect();
    if env::var("DUMP").is_ok() {
        let all: Vec<String> = rig
            .onsets()
            .iter()
            .rev()
            .take(12)
            .rev()
            .map(|(_, o)| format!("{:.2}", (*o as f64 - ideal[down]) / TICKS_PER_MS))
            .collect();
        println!("last onsets overall: {}", all.join(" "));
        let rel: Vec<String> = on
            .iter()
            .take(8)
            .map(|o| format!("{:.2}", (*o as f64 - ideal[down]) / TICKS_PER_MS))
            .collect();
        println!("onsets from the downbeat F8 (ms): {}", rel.join(" "));
        let fr: Vec<String> = rig.onsets()[start..].iter().take(8).map(|(f, _)| format!("{}", f % 32)).collect();
        println!("their frame in the 32-sample block: {}", fr.join(" "));
    }
    let (mut e_ideal, mut e_arr, mut first) = (vec![], vec![], None);
    let mut worst_after_bar: f64 = 0.0;
    for (j, k) in (down..n).step_by(24 * every).enumerate() {
        let target = ideal[k] + if c.usb { 0.0 } else { byte };
        let near = on
            .iter()
            .min_by_key(|o| (**o as i64 - target as i64).unsigned_abs());
        let Some(o) = near else { continue };
        let d = (*o as f64 - target) / TICKS_PER_MS;
        if d.abs() > 30.0 {
            continue;
        }
        if j == 0 {
            first = Some(d);
            continue;
        }
        e_ideal.push(d);
        e_arr.push((*o as f64 - arrival[k]) / TICKS_PER_MS);
        if j * every >= 4 {
            worst_after_bar = worst_after_bar.max(d.abs());
        }
    }
    let tempo = if c.to > 0.0 {
        format!(
            " -> {} ({})",
            c.to,
            if c.ramp > 0.0 {
                format!("over {} beats", c.ramp)
            } else {
                "jump".into()
            }
        )
    } else {
        String::new()
    };
    println!(
        "{} at {mhz} MHz{}, {} BPM{tempo}{}{}: {} beats, {} onsets, the first hit {:+.3} ms from its F8, worst after bar 1 {:.3} ms",
        if c.usb { "USB" } else { "TRS" },
        if rig.cpu.nested_irqs { "" } else { " (no nesting)" },
        c.bpm,
        if c.jitter > 0.0 { format!(" +-{} ms", c.jitter) } else { String::new() },
        if c.fa >= 0.0 { format!(", FA {} ms ahead", c.fa) } else { String::new() },
        c.beats,
        on.len(),
        first.unwrap_or(f64::NAN),
        worst_after_bar
    );
    println!("{}", line("onset - F8 ideal time", &e_ideal));
    println!("{}", line("onset - F8 arrival", &e_arr));
}

/// START a tick ahead, STOP after a bar, song position 16th 136 + CONTINUE a tick ahead (USB, 120 BPM)
fn transport(rig: &mut Rig, c: &Clock) {
    hats(rig, 1);
    rig.set_global(13, c.sync);
    rig.cpu.bus.audio.probe.arm(2048, 220); // the hits: |x| >= 2048 (Q15 16) after 5 ms below 256
    rig.run_ms(100.0);
    let p = 60.0 / (24.0 * c.bpm) * 24e6;
    let mut t = rig.now() as f64 + 10.0 * TICKS_PER_MS;
    let (k_fa, k_fc, k_fb) = (48usize, 48 + 96, 48 + 96 + 72);
    let start = rig.onsets().len();
    let (mut t_down, mut t_cont, mut t_stop) = (0.0, 0.0, 0.0);
    let mut after_stop = 0;
    let lag = if c.usb { 0.0 } else { fm1_emu::uart1::BYTE_TICKS as f64 }; // TRS: the F8 lands a byte later
    for k in 0..(k_fb + 24 * 8) {
        if k == k_fa || k == k_fb {
            rig.run_until((t - p + 0.1 * TICKS_PER_MS) as u64);
            if k == k_fb {
                send(rig, c.usb, &[0xF2, 136 & 127, 1]);
            }
            send(rig, c.usb, &[if k == k_fa { 0xFA } else { 0xFB }]);
        }
        if k == k_fc {
            rig.run_until((t - 0.1 * TICKS_PER_MS) as u64);
            send(rig, c.usb, &[0xFC]);
            t_stop = t;
        }
        rig.run_until(t as u64);
        send(rig, c.usb, &[0xF8]);
        if k == k_fa {
            t_down = rig.now() as f64 + lag;
        }
        if k == k_fb {
            t_cont = rig.now() as f64 + lag;
        }
        if k == k_fc + 24 {
            after_stop = rig.onsets().len();
        }
        t += p;
    }
    rig.run_ms(30.0);
    let on: Vec<f64> = rig.onsets()[start..]
        .iter()
        .map(|(_, t)| *t as f64)
        .collect();
    let near = |x: f64| {
        on.iter()
            .map(|o| (o - x) / TICKS_PER_MS)
            .min_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap())
            .unwrap_or(f64::NAN)
    };
    println!(
        "transport ({}), {} BPM:",
        if c.usb { "USB" } else { "TRS" },
        c.bpm
    );
    println!(
        "  START a tick ahead: the downbeat hit {:+.3} ms from its F8",
        near(t_down)
    );
    println!(
        "  STOP: {} hits after it (want 0)",
        rig.onsets()[start..].len().min(after_stop) as i64
            - on.iter()
                .filter(|o| **o < t_stop + 30.0 * TICKS_PER_MS)
                .count() as i64
    );
    println!("  SPP 136 + CONTINUE a tick ahead: the first hit {:+.3} ms, the next {:+.3} ms from their F8", near(t_cont), near(t_cont + 24.0 * p));
}
