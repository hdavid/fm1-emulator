// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{bus::Bus, cpu::Cpu, firmware::Firmware, NAMES};
use std::{env, fs::File, io::Write, path::Path, process::ExitCode};

fn number(value: &str) -> Result<u32, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        value.parse()
    }
    .map_err(|_| format!("invalid number: {value}"))
}

fn main_run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 || !matches!(args[0].as_str(), "probe" | "boot") {
        return Err("usage: fm1-emu <probe|boot> <application.elf|application.bin> [--entry ADDRESS] [--limit COUNT] [--trace PATH]".into());
    }
    let mut entry = None;
    let mut limit = 100_000;
    let mut trace_path = None;
    let mut i = 2;
    while i < args.len() {
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {}", args[i]))?;
        match args[i].as_str() {
            "--entry" => entry = Some(number(value)?),
            "--limit" => limit = value.parse().map_err(|_| "invalid instruction limit")?,
            "--trace" => trace_path = Some(value),
            option => return Err(format!("unknown option: {option}")),
        }
        i += 2;
    }
    let firmware = Firmware::load(Path::new(&args[1]))?;
    let entry = if let Some(entry) = entry {
        entry
    } else if args[0] == "probe" {
        *firmware
            .symbols
            .get("fm1_probe")
            .ok_or("probe needs ELF symbols or --entry for a raw image")?
    } else {
        firmware.entry
    };
    let image_bytes = firmware.image.len();
    let mut cpu = Cpu::new(Bus::new(firmware.image)?, entry);
    let mut trace = trace_path
        .map(File::create)
        .transpose()
        .map_err(|error| error.to_string())?;
    let trace_writer = trace.as_mut().map(|file| file as &mut dyn Write);
    if args[0] == "probe" {
        let values = cpu
            .probe(limit, trace_writer)
            .map_err(|error| error.to_string())?;
        println!("{{\"mode\":\"probe\",\"entry\":{entry},\"image_bytes\":{image_bytes},\"instructions\":{},\"values\":{{", cpu.steps);
        for (i, (name, value)) in NAMES.iter().zip(values).enumerate() {
            println!(
                "  \"{name}\":{value}{}",
                if i + 1 == NAMES.len() { "" } else { "," }
            );
        }
        println!("}}}}");
        Ok(())
    } else {
        // Application handoff, not a ROM/SPL emulator. Unknown initial CPU state
        // is zeroed; crt0 immediately supplies the application's own stacks.
        cpu.r[0] = 0x01c7_fe08;
        cpu.run(None, limit, trace_writer)
            .map_err(|error| format!("after {} instructions: {error}", cpu.steps))
    }
}

fn main() -> ExitCode {
    match main_run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fm1-emu: {error}");
            ExitCode::FAILURE
        }
    }
}
