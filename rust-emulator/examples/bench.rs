// SPDX-License-Identifier: GPL-3.0-only
// Interpreter throughput and determinism check.
//   bench FIRMWARE LIMIT          lean loop: step only, report instr/s
//   bench FIRMWARE LIMIT batch    the same through Cpu::run_steps
//   bench FIRMWARE LIMIT hash     also fold PC, registers, special registers
//                                 and interrupt count into a hash every step,
//                                 then hash all SRAM and audio samples
//                                 (batch prints those final hashes too)
// FM1_CPU_MHZ=N sets the emulated clock as in diagnose.
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
    let steps = cpu.steps;
    println!(
        "{steps} instructions in {seconds:.3} s: {:.1} M instr/s",
        steps as f64 / seconds / 1e6
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
        if hashing {
            println!("state hash {hash:016x}");
        }
        println!("final state {:016x}", state_hash(&cpu, FNV_OFFSET));
        println!("ram hash {ram_hash:016x}");
        println!("audio hash {audio_hash:016x} ({} frames)", cpu.bus.audio.frames);
        println!("serial bytes pending {}", cpu.bus.usb.serial.len());
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
