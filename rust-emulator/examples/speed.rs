// SPDX-License-Identifier: GPL-3.0-only
// Side-by-side speed harness: the same source builds against upstream's
// public API and against this fork (`RUSTFLAGS="--cfg fm1_fork"`).
//   speed FIRMWARE idle|play GUEST_SECONDS   guest seconds per host second
//   speed FIRMWARE hash CALLS                CALLS cpu.step() calls, then a
//                                            hash of the machine state
// Guest time is counted in audio frames (44.1 kHz), a counter both builds
// have, over a window that starts after 3 guest seconds of boot. Each build
// is driven the way its own GUI worker drives it: upstream with a loop of
// `Cpu::step`, the fork with `Cpu::run_steps` (event-scheduled devices, idle
// and spin skip). `play` holds a chord on the note keys for 0.15 s out of
// every 0.3 s of guest time; `idle` presses nothing.
// Fork only: FM1_CPU_MHZ=N, FM1_IDLE_SKIP=0, FM1_SPIN_SKIP=0,
// FM1_BLOCK_CACHE=1, FM1_STEP=1 (a `step` loop instead of `run_steps`),
// FM1_QOS=default (the thread class a spawned thread starts with; without
// it the worker takes fm1-ui's performance-core class).
use fm1_emu::{cpu::Cpu, firmware::Firmware};
use std::{env, path::Path, process::ExitCode, time::Instant};

const RATE: f64 = 44_100.0;
const CHUNK: u64 = 20_000;
const WARMUP_FRAMES: u64 = 3 * 44_100;
const WALL_LIMIT: f64 = 240.0;
// The panel matrix (column, row) of the note keys 14, 18 and 21, as fm1-ui's KEYMAP.
const CHORD: [(usize, usize); 3] = [(3, 4), (7, 4), (9, 4)];
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fold(hash: u64, value: u32) -> u64 {
    (hash ^ value as u64).wrapping_mul(FNV_PRIME)
}

fn thread_cpu_seconds() -> f64 {
    #[repr(C)]
    struct Timespec {
        seconds: i64,
        nanoseconds: i64,
    }
    extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // 16 is CLOCK_THREAD_CPUTIME_ID on macOS.
    // SAFETY: clock_gettime writes one timespec through a valid pointer.
    unsafe { clock_gettime(16, &mut time) };
    time.seconds as f64 + time.nanoseconds as f64 * 1e-9
}

#[cfg(fm1_fork)]
fn advance(cpu: &mut Cpu, calls: u64) -> Result<(), String> {
    if env::var("FM1_STEP").is_ok_and(|v| v == "1") {
        for _ in 0..calls {
            cpu.step().map_err(|e| e.to_string())?;
        }
        Ok(())
    } else {
        cpu.run_steps(calls).map_err(|e| e.to_string())
    }
}

#[cfg(not(fm1_fork))]
fn advance(cpu: &mut Cpu, calls: u64) -> Result<(), String> {
    for _ in 0..calls {
        cpu.step().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(fm1_fork)]
fn configure(cpu: &mut Cpu) -> Result<(), String> {
    if let Ok(mhz) = env::var("FM1_CPU_MHZ") {
        cpu.set_cpu_mhz(mhz.parse().map_err(|_| "invalid FM1_CPU_MHZ")?)?;
    }
    cpu.idle_skip = env::var("FM1_IDLE_SKIP").map_or(true, |v| v != "0");
    cpu.spin_skip = env::var("FM1_SPIN_SKIP").map_or(true, |v| v != "0");
    cpu.set_block_cache(env::var("FM1_BLOCK_CACHE").is_ok_and(|v| v == "1"));
    Ok(())
}

#[cfg(not(fm1_fork))]
fn configure(_cpu: &mut Cpu) -> Result<(), String> {
    Ok(())
}

#[cfg(fm1_fork)]
fn skipped(cpu: &Cpu) -> String {
    format!(
        "idle_jumped {} spin_jumped {:?}",
        cpu.idle_skipped, cpu.spin_skipped
    )
}

#[cfg(not(fm1_fork))]
fn skipped(_cpu: &Cpu) -> String {
    "idle_jumped n/a spin_jumped n/a".into()
}

fn press(cpu: &mut Cpu, down: bool) -> Result<(), String> {
    for (column, row) in CHORD {
        cpu.bus
            .devices
            .gpio
            .press(column, row, down)
            .map_err(str::to_string)?;
    }
    Ok(())
}

fn hash_state(cpu: &Cpu) -> u64 {
    let hash = fold(FNV_OFFSET, cpu.pc);
    let hash = cpu.r.iter().fold(hash, |h, &v| fold(h, v));
    let hash = cpu.sr.iter().fold(hash, |h, &v| fold(h, v));
    let hash = fold(hash, cpu.irq_entries as u32);
    (0..fm1_emu::RAM_SIZE as u32)
        .step_by(4)
        .fold(hash, |h, i| fold(h, cpu.bus.read(fm1_emu::RAM + i, 4).unwrap_or(0)))
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: speed FIRMWARE idle|play SECONDS | speed FIRMWARE hash CALLS".into());
    }
    let firmware = Firmware::load(Path::new(&args[0]))?;
    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    configure(&mut cpu)?;
    let mode = args[1].as_str();
    let amount: f64 = args[2].parse().map_err(|_| "invalid amount")?;
    if mode == "hash" {
        for _ in 0..amount as u64 {
            cpu.step().map_err(|e| e.to_string())?;
        }
        println!(
            "hash {:016x} steps {} irq {} frames {} pc {:08x}",
            hash_state(&cpu),
            cpu.steps,
            cpu.irq_entries,
            cpu.bus.audio.frames,
            cpu.pc
        );
        return Ok(());
    }
    let playing = mode == "play";
    if !playing && mode != "idle" {
        return Err(format!("unknown mode {mode}"));
    }
    let boot = Instant::now();
    while cpu.bus.audio.frames < WARMUP_FRAMES {
        advance(&mut cpu, CHUNK)?;
        if boot.elapsed().as_secs_f64() > WALL_LIMIT {
            return Err("warm-up did not reach 3 guest seconds".into());
        }
    }
    let (frames0, steps0) = (cpu.bus.audio.frames, cpu.steps);
    let target = frames0 + (amount * RATE) as u64;
    let (wall0, cpu0) = (Instant::now(), thread_cpu_seconds());
    while cpu.bus.audio.frames < target {
        if playing {
            let phase = (cpu.bus.audio.frames - frames0) % (RATE as u64 * 3 / 10);
            press(&mut cpu, phase < RATE as u64 * 15 / 100)?;
        }
        advance(&mut cpu, CHUNK)?;
        if wall0.elapsed().as_secs_f64() > WALL_LIMIT {
            break;
        }
    }
    let wall = wall0.elapsed().as_secs_f64();
    let cpu_s = thread_cpu_seconds() - cpu0;
    let guest = (cpu.bus.audio.frames - frames0) as f64 / RATE;
    println!(
        "guest {guest:.3} s wall {wall:.3} s cpu {cpu_s:.3} s | x_real {:.3} x_real_per_cpu_s {:.3} | instr/guest-s {:.1}M | {}",
        guest / wall,
        guest / cpu_s,
        (cpu.steps - steps0) as f64 / guest / 1e6,
        skipped(&cpu)
    );
    Ok(())
}

fn main() -> ExitCode {
    let worker = std::thread::spawn(|| {
        #[cfg(fm1_fork)]
        if env::var("FM1_QOS").map_or(true, |v| v != "default") {
            fm1_emu::host_thread::favour_performance_cores();
        }
        run()
    });
    match worker
        .join()
        .unwrap_or_else(|_| Err("speed thread panicked".into()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("speed: {error}");
            ExitCode::FAILURE
        }
    }
}
