// SPDX-License-Identifier: GPL-3.0-only
// Interpreter throughput and determinism check.
//   bench FIRMWARE LIMIT          step only, report instructions per second
//   bench FIRMWARE LIMIT batch    the same through Cpu::run_steps
//   bench FIRMWARE LIMIT hash     also fold PC, registers, special registers
//                                 and the interrupt count into a hash after
//                                 every step
// batch and hash also print hashes of the final state, SRAM, the audio
// samples and the LCD pixels: two builds that execute alike print the same.
// FM1_CPU_MHZ=N sets the instruction clock as in diagnose; FM1_IDLE_SKIP=0
// steps halted idle slots one by one in batch mode; FM1_NESTED_IRQ=1 lets
// interrupts nest; FM1_BLOCK_CACHE=1 executes through the block cache and JIT.
use fm1_emu::{cpu::Cpu, firmware::Firmware};
use std::{env, path::Path, process::ExitCode, time::Instant};

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

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: bench FIRMWARE LIMIT [hash|batch]".into());
    }
    let limit: u64 = args[1].parse().map_err(|_| "invalid instruction limit")?;
    let (hashing, batched) = match args.get(2).map(String::as_str) {
        None => (false, false),
        Some("hash") => (true, false),
        Some("batch") => (false, true),
        Some(other) => return Err(format!("unknown mode {other}")),
    };
    let firmware = Firmware::load(Path::new(&args[0]))?;
    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    if let Ok(mhz) = env::var("FM1_CPU_MHZ") {
        cpu.set_cpu_mhz(mhz.parse().map_err(|_| "invalid FM1_CPU_MHZ")?)?;
    }
    cpu.idle_skip = env::var("FM1_IDLE_SKIP").map_or(true, |v| v != "0");
    cpu.nested_irqs = env::var("FM1_NESTED_IRQ").is_ok_and(|v| v == "1");
    cpu.set_block_cache(env::var("FM1_BLOCK_CACHE").is_ok_and(|v| v == "1"));
    let mut hash = FNV_OFFSET;
    let mut fault = None;
    let start = Instant::now();
    if hashing {
        for _ in 0..limit {
            if let Err(error) = cpu.step() {
                fault = Some(error);
                break;
            }
            hash = state_hash(&cpu, hash);
        }
    } else if batched {
        if let Err(error) = cpu.run_steps(limit) {
            fault = Some(error);
        }
    } else {
        for _ in 0..limit {
            if let Err(error) = cpu.step() {
                fault = Some(error);
                break;
            }
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let calls = cpu.core_steps[0].max(cpu.core_steps[1]);
    println!(
        "{} instructions ({calls} calls) in {seconds:.3} s: {:.1} M instr/s",
        cpu.steps,
        cpu.steps as f64 / seconds / 1e6
    );
    let guest = cpu.bus.oscillator_ticks() as f64 / 24e6;
    println!(
        "guest time {guest:.3} s: {:.3}x real time; {} slots halted in idle ({} jumped over)",
        guest / seconds,
        cpu.idle_slots,
        cpu.idle_skipped
    );
    if hashing || batched {
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
        if hashing {
            println!("state hash {hash:016x}");
        }
        println!("final state {:016x}", state_hash(&cpu, FNV_OFFSET));
        println!(
            "steps {} core steps {:?} oscillator ticks {} interrupts {}",
            cpu.steps,
            cpu.core_steps,
            cpu.bus.oscillator_ticks(),
            cpu.irq_entries
        );
        println!("ram hash {ram_hash:016x}");
        println!(
            "audio hash {audio_hash:016x} ({} frames)",
            cpu.bus.audio.frames
        );
        println!(
            "lcd hash {lcd_hash:016x} ({} pixels written)",
            cpu.bus.lcd.pixels_written
        );
        println!("fault {:?}", fault.as_ref().map(ToString::to_string));
    } else if let Some(error) = fault {
        println!("fault: {error}");
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench: {error}");
            ExitCode::FAILURE
        }
    }
}
