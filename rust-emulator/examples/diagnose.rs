// SPDX-License-Identifier: GPL-3.0-only
// Bounded boot diagnostics using the same CPU/bus as the graphical emulator.
use fm1_emu::{bus::Bus, cpu::Cpu, firmware::Firmware};
use std::{
    collections::{BTreeMap, VecDeque},
    env,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

fn location(symbols: &BTreeMap<String, u32>, pc: u32) -> String {
    let closest = symbols
        .iter()
        .filter(|(_, address)| **address <= pc)
        .max_by_key(|(_, address)| **address);
    match closest {
        Some((name, address)) => format!("0x{pc:08x} ({name}+0x{:x})", pc - address),
        None => format!("0x{pc:08x}"),
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if !(1..=2).contains(&args.len()) {
        return Err("usage: diagnose APPLICATION.{elf,bin} [INSTRUCTION_LIMIT]".into());
    }
    let limit: u64 = args
        .get(1)
        .map(|s| s.parse())
        .transpose()
        .map_err(|_| "invalid instruction limit")?
        .unwrap_or(10_000_000);
    let firmware = Firmware::load(Path::new(&args[0]))?;
    let mut cpu = Cpu::new(Bus::new(firmware.image)?, firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    let mut recent = VecDeque::new();
    let mut serial_bytes = 0u64;
    let mut stdout = io::stdout().lock();
    let mut fault = None;
    for _ in 0..limit {
        let pc = cpu.pc;
        match cpu.step() {
            Ok(op) => {
                if recent.len() == 12 {
                    recent.pop_front();
                }
                recent.push_back((pc, op));
            }
            Err(error) => {
                fault = Some(error);
                break;
            }
        }
        // Terminal stdout contains only real guest CDC endpoint data.
        while let Some(byte) = cpu.bus.usb.serial.pop_front() {
            stdout.write_all(&[byte]).map_err(|e| e.to_string())?;
            serial_bytes += 1;
        }
    }
    stdout.flush().map_err(|e| e.to_string())?;
    eprintln!("application: {}", args[0]);
    eprintln!("executed: {} instructions", cpu.steps);
    eprintln!("stopped: {}", location(&firmware.symbols, cpu.pc));
    eprintln!(
        "LCD: {} pixels written; visible={}",
        cpu.bus.lcd.pixels_written,
        cpu.bus.screen_visible()
    );
    eprintln!(
        "interrupts: {}; USB: {} host setups, {} packets, {} CDC bytes",
        cpu.irq_entries, cpu.bus.usb.setups, cpu.bus.usb.packets, serial_bytes
    );
    eprintln!("watchdog: {} feeds", cpu.bus.system.watchdog_feeds);
    eprintln!("recent completed instructions:");
    for (pc, op) in recent {
        eprintln!("  {}: {op}", location(&firmware.symbols, pc));
    }
    eprintln!("registers:");
    for (i, register) in cpu.r.iter().enumerate() {
        eprintln!("  r{i}=0x{register:08x}");
    }
    eprintln!(
        "  sp=0x{:08x}; rets=0x{:08x}; interrupts_enabled={}",
        cpu.sr[14], cpu.sr[3], cpu.interrupts_enabled
    );
    match fault {
        Some(error) => Err(error.to_string()),
        None => Err(format!("instruction limit {limit} reached")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("diagnose: {error}");
            ExitCode::FAILURE
        }
    }
}
