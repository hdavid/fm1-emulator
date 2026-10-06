// SPDX-License-Identifier: GPL-3.0-only
// Interpreter throughput and determinism check.
//   bench FIRMWARE LIMIT          step only, report instructions per second
//   bench FIRMWARE LIMIT batch    the same through Cpu::run_steps
//   bench FIRMWARE LIMIT hash     also fold PC, registers, special registers
//                                 and the interrupt count into a hash after
//                                 every step
//   bench FIRMWARE LIMIT gui      run_steps in the GUI worker's batches of
//                                 1024 calls, draining the audio into a
//                                 playback queue and copying a changed LCD
//                                 frame every 16 ms of host time, as fm1-ui
// batch, hash and gui also print hashes of the final state, SRAM, the audio
// samples and the LCD pixels: two builds that execute alike print the same.
// FM1_CPU_MHZ=N sets the instruction clock as in diagnose; FM1_IDLE_SKIP=0
// steps halted idle slots one by one in batch mode; FM1_NESTED_IRQ=1 lets
// interrupts nest; FM1_BLOCK_CACHE=1 executes through the block cache and JIT.
// FM1_SCENARIO=FILE first plays the panel steps in FILE (whitespace
// separated, as play_check: run:SECONDS, hold:ID,ID.., release,
// turn:KNOB:DETENTS) without timing them, e.g. to measure a busy song:
// examples/scenarios/busy.steps (made by busy_song.py there) drives an
// Optimist build into three synth tracks, drums and FX.
// Everything runs on a spawned thread at fm1-ui's worker class
// (host_thread); FM1_QOS=default leaves the class a spawned thread gets.
use fm1_emu::{
    cpu::Cpu,
    player::{knob, Player},
};
use std::{
    collections::VecDeque,
    env,
    hint::black_box,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fold(hash: u64, value: u32) -> u64 {
    (hash ^ value as u64).wrapping_mul(FNV_PRIME)
}

fn state_hash(cpu: &Cpu, hash: u64) -> u64 {
    let hash = fold(hash, cpu.pc);
    let hash = cpu.r.iter().fold(hash, |h, &v| fold(h, v));
    let hash = cpu.sr.iter().fold(hash, |h, &v| fold(h, v));
    fold(hash, cpu.irq_entries as u32)
}

/// CPU time of this thread in seconds: on a loaded host, less disturbed
/// than wall time by the waits for a core. None where not available.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn thread_cpu_seconds() -> Option<f64> {
    #[repr(C)]
    struct Timespec {
        seconds: i64,
        nanoseconds: i64,
    }
    extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    #[cfg(target_os = "macos")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
    #[cfg(target_os = "linux")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // SAFETY: clock_gettime writes one timespec through a valid pointer.
    let status = unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut time) };
    (status == 0).then_some(time.seconds as f64 + time.nanoseconds as f64 * 1e-9)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn thread_cpu_seconds() -> Option<f64> {
    None
}

#[derive(PartialEq)]
enum Mode {
    Step,
    Hash,
    Batch,
    Gui,
}

/// The panel steps of FM1_SCENARIO, played through `Player`.
fn play_scenario(player: &mut Player, path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    for step in text.split_whitespace() {
        let parts: Vec<&str> = step.split(':').collect();
        let bad = || format!("{path}: bad step {step}");
        match parts.as_slice() {
            ["run", seconds] => player.run_seconds(seconds.parse().map_err(|_| bad())?)?,
            ["hold", ids] => {
                for id in ids.split(',') {
                    let id: usize = id.parse().map_err(|_| bad())?;
                    *player.held.get_mut(id).ok_or_else(bad)? = true;
                }
            }
            ["release"] => player.held = [false; 41],
            ["turn", name, detents] => {
                let detents = detents.parse().map_err(|_| bad())?;
                player.encoders.turn(knob(name)?, detents);
            }
            _ => return Err(bad()),
        }
    }
    Ok(())
}

/// What fm1-ui's worker does around `run_steps`: batches of 1024 calls, the
/// audio drained into a bounded playback queue after each, and a copy of
/// the LCD when it changed, at most every 16 ms. Returns a hash of the
/// audio drained.
fn run_like_gui(player: &mut Player, limit: u64) -> Result<u64, String> {
    const MAX_FRAMES: usize = 16_384;
    let mut queue: VecDeque<[f32; 2]> = VecDeque::new();
    let mut audio_hash = FNV_OFFSET;
    let mut last_lcd = None;
    let mut next_frame = Instant::now();
    let mut done = 0;
    while done < limit {
        let cpu = &mut player.cpu;
        player.encoders.drive(&mut cpu.bus.devices.gpio);
        let calls = (limit - done).min(1024);
        cpu.run_steps(calls).map_err(|fault| fault.to_string())?;
        done += calls;
        for [l, r] in cpu.bus.audio.samples.drain(..) {
            audio_hash = fold(fold(audio_hash, l as u32), r as u32);
            queue.push_back([l as f32 / 8_388_608.0, r as f32 / 8_388_608.0]);
        }
        let excess = queue.len().saturating_sub(MAX_FRAMES);
        queue.drain(..excess);
        if Instant::now() >= next_frame {
            let lcd = (cpu.bus.lcd.pixels_written, cpu.bus.screen_visible());
            if last_lcd != Some(lcd) {
                last_lcd = Some(lcd);
                black_box(cpu.bus.lcd.pixels.clone());
            }
            next_frame = Instant::now() + Duration::from_millis(16);
        }
    }
    black_box(&queue);
    Ok(audio_hash)
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: bench FIRMWARE LIMIT [hash|batch|gui]".into());
    }
    let limit: u64 = args[1].parse().map_err(|_| "invalid instruction limit")?;
    let mode = match args.get(2).map(String::as_str) {
        None => Mode::Step,
        Some("hash") => Mode::Hash,
        Some("batch") => Mode::Batch,
        Some("gui") => Mode::Gui,
        Some(other) => return Err(format!("unknown mode {other}")),
    };
    let mut player = Player::boot(Path::new(&args[0]), 0)?;
    let cpu = &mut player.cpu;
    if let Ok(mhz) = env::var("FM1_CPU_MHZ") {
        cpu.set_cpu_mhz(mhz.parse().map_err(|_| "invalid FM1_CPU_MHZ")?)?;
    }
    cpu.idle_skip = env::var("FM1_IDLE_SKIP").map_or(true, |v| v != "0");
    cpu.nested_irqs = env::var("FM1_NESTED_IRQ").is_ok_and(|v| v == "1");
    cpu.set_block_cache(env::var("FM1_BLOCK_CACHE").is_ok_and(|v| v == "1"));
    cpu.spin_skip = env::var("FM1_SPIN_SKIP").map_or(true, |v| v != "0");
    cpu.spin_log = env::var("FM1_SPIN_LOG").is_ok_and(|v| v == "1");
    if let Ok(path) = env::var("FM1_SCENARIO") {
        let start = Instant::now();
        play_scenario(&mut player, &path)?;
        println!(
            "scenario: {:.3} s guest, {} instructions, in {:.1} s",
            player.cpu.bus.oscillator_ticks() as f64 / 24e6,
            player.cpu.steps,
            start.elapsed().as_secs_f64()
        );
    }
    let before = (
        player.cpu.steps,
        player.cpu.bus.oscillator_ticks(),
        player.cpu.idle_skipped,
    );
    let mut hash = FNV_OFFSET;
    let mut fault = None;
    let mut drained = None;
    let start = Instant::now();
    let start_cpu = thread_cpu_seconds();
    match mode {
        Mode::Hash => {
            let cpu = &mut player.cpu;
            for _ in 0..limit {
                if let Err(error) = cpu.step() {
                    fault = Some(error.to_string());
                    break;
                }
                hash = state_hash(cpu, hash);
            }
        }
        Mode::Batch => {
            if let Err(error) = player.cpu.run_steps(limit) {
                fault = Some(error.to_string());
            }
        }
        Mode::Gui => match run_like_gui(&mut player, limit) {
            Ok(audio) => drained = Some(audio),
            Err(error) => fault = Some(error),
        },
        Mode::Step => {
            let cpu = &mut player.cpu;
            for _ in 0..limit {
                if let Err(error) = cpu.step() {
                    fault = Some(error.to_string());
                    break;
                }
            }
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let cpu_seconds = start_cpu.zip(thread_cpu_seconds()).map(|(a, b)| b - a);
    let cpu = &player.cpu;
    let calls = cpu.core_steps[0].max(cpu.core_steps[1]);
    let steps = cpu.steps - before.0;
    println!(
        "{steps} instructions ({calls} calls) in {seconds:.3} s: {:.1} M instr/s",
        steps as f64 / seconds / 1e6
    );
    let guest = (cpu.bus.oscillator_ticks() - before.1) as f64 / 24e6;
    println!(
        "guest time {guest:.3} s: {:.3}x real time; {} slots halted in idle ({} jumped over); spin loops jumped over {:?}",
        guest / seconds,
        cpu.idle_slots,
        cpu.idle_skipped - before.2,
        cpu.spin_skipped
    );
    if let Some(cpu_seconds) = cpu_seconds {
        println!(
            "host CPU time {cpu_seconds:.3} s: {:.3}x real time per CPU second",
            guest / cpu_seconds
        );
    }
    if mode != Mode::Step {
        let ram_hash = (0..fm1_emu::RAM_SIZE as u32)
            .step_by(4)
            .fold(FNV_OFFSET, |h, i| {
                fold(h, cpu.bus.read(fm1_emu::RAM + i, 4).unwrap_or(0))
            });
        let audio_hash = cpu
            .bus
            .audio
            .samples
            .iter()
            .fold(FNV_OFFSET, |h, [l, r]| fold(fold(h, *l as u32), *r as u32));
        let lcd_hash = cpu
            .bus
            .lcd
            .pixels
            .iter()
            .fold(FNV_OFFSET, |h, &p| fold(h, p));
        if mode == Mode::Hash {
            println!("state hash {hash:016x}");
        }
        println!("final state {:016x}", state_hash(cpu, FNV_OFFSET));
        println!(
            "steps {} core steps {:?} oscillator ticks {} interrupts {}",
            cpu.steps,
            cpu.core_steps,
            cpu.bus.oscillator_ticks(),
            cpu.irq_entries
        );
        println!("ram hash {ram_hash:016x}");
        match drained {
            Some(drained) => println!(
                "audio hash {drained:016x} (drained; {} frames)",
                cpu.bus.audio.frames
            ),
            None => println!(
                "audio hash {audio_hash:016x} ({} frames)",
                cpu.bus.audio.frames
            ),
        }
        println!(
            "lcd hash {lcd_hash:016x} ({} pixels written)",
            cpu.bus.lcd.pixels_written
        );
        println!("fault {fault:?}");
    } else if let Some(error) = fault {
        println!("fault: {error}");
    }
    Ok(())
}

fn main() -> ExitCode {
    // On a spawned thread with the class fm1-ui's worker takes
    // (FM1_QOS=default: the class a spawned thread starts with).
    let worker = std::thread::spawn(|| {
        if env::var("FM1_QOS").map_or(true, |v| v != "default") {
            fm1_emu::host_thread::favour_performance_cores();
        }
        run()
    });
    match worker
        .join()
        .unwrap_or_else(|_| Err("bench thread panicked".into()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench: {error}");
            ExitCode::FAILURE
        }
    }
}
