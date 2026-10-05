// SPDX-License-Identifier: GPL-3.0-only
// Bounded boot diagnostics using the same CPU/bus as the graphical emulator.
use fm1_emu::{cpu::Cpu, firmware::Firmware};
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
        return Err("usage: diagnose FIRMWARE.{fwsc,elf,bin} [INSTRUCTION_LIMIT]".into());
    }
    let limit: u64 = args
        .get(1)
        .map(|s| s.parse())
        .transpose()
        .map_err(|_| "invalid instruction limit")?
        .unwrap_or(10_000_000);
    let firmware = Firmware::load(Path::new(&args[0]))?;
    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    // Optional: FM1_CPU_MHZ=N emulates an N MHz CPU (default 24, real time).
    if let Ok(mhz) = env::var("FM1_CPU_MHZ") {
        cpu.set_cpu_mhz(mhz.parse().map_err(|_| "invalid FM1_CPU_MHZ")?)?;
    }
    // Optional: FM1_WATCHDOG_OFF=1 keeps counting watchdog ticks but never
    // resets. Diagnostic experiments only; the output says so.
    let watchdog_off = env::var("FM1_WATCHDOG_OFF").is_ok_and(|v| v == "1");
    cpu.bus.system.watchdog_expiry_disabled = watchdog_off;
    let mut recent = VecDeque::new();
    // Optional: FM1_RECENT=N keeps N completed instructions with registers.
    let recent_count: usize = env::var("FM1_RECENT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12)
        .max(1);
    let mut serial_bytes = 0u64;
    let mut stdout = io::stdout().lock();
    let mut fault = None;
    // Optional: FM1_HOT=N counts primary-core PCs over the final N steps.
    let hot_window: u64 = env::var("FM1_HOT")
        .ok()
        .map(|value| value.parse().map_err(|_| "invalid FM1_HOT"))
        .transpose()?
        .unwrap_or(0);
    let mut hot = BTreeMap::<u32, u64>::new();
    let callers_pc: Option<u32> = env::var("FM1_CALLERS")
        .ok()
        .and_then(|pc| u32::from_str_radix(pc.trim_start_matches("0x"), 16).ok());
    let mut callers = BTreeMap::<u32, u64>::new();
    // Optional: FM1_WATCH=PC[,PC] prints registers whenever CPU 0 reaches PC.
    let watch: Vec<u32> = env::var("FM1_WATCH")
        .map(|list| {
            list.split(',')
                .filter_map(|pc| u32::from_str_radix(pc.trim_start_matches("0x"), 16).ok())
                .collect()
        })
        .unwrap_or_default();
    let mut watch_hits = 0;
    // Optional: FM1_MEMWATCH=ADDRESS prints each instruction that changes the
    // aligned 32-bit word at ADDRESS (first 400 changes).
    let memwatch: Option<u32> = env::var("FM1_MEMWATCH")
        .ok()
        .map(|a| {
            u32::from_str_radix(a.trim_start_matches("0x"), 16).map_err(|_| "invalid FM1_MEMWATCH")
        })
        .transpose()?
        .map(|a| a & !3);
    let mut memwatch_value = memwatch.and_then(|a| cpu.bus.read(a, 4).ok());
    let mut memwatch_hits = 0;
    // Optional: FM1_MMIO=N counts MMIO reads/writes over the final N steps.
    let mmio_window: u64 = env::var("FM1_MMIO")
        .ok()
        .map(|value| value.parse().map_err(|_| "invalid FM1_MMIO"))
        .transpose()?
        .unwrap_or(0);
    // Optional: FM1_OP=NAME counts executions per PC of one operation name.
    let op_filter = env::var("FM1_OP").ok();
    let mut op_pcs = BTreeMap::<u32, u64>::new();
    for step in 0..limit {
        let pc = cpu.pc;
        if watch.contains(&pc) && watch_hits < 400 {
            watch_hits += 1;
            let list: Vec<_> = cpu.r.iter().map(|r| format!("{r:x}")).collect();
            eprintln!(
                "watch {step}: {} sp={:x} rets={:x} [{}]",
                location(&firmware.symbols, pc),
                cpu.sr[14],
                cpu.sr[3],
                list.join(" ")
            );
        }
        if hot_window > 0 && step + hot_window >= limit {
            *hot.entry(pc).or_default() += 1;
            // Optional: FM1_CALLERS=PC counts rets at PC in the hot window.
            if callers_pc == Some(pc) {
                *callers.entry(cpu.sr[3]).or_default() += 1;
            }
        }
        if mmio_window > 0 && step + mmio_window == limit {
            *cpu.bus.mmio_stats.borrow_mut() = Some(BTreeMap::new());
        }
        match cpu.step() {
            Ok(op) => {
                if op_filter.as_deref() == Some(op) {
                    *op_pcs.entry(pc).or_default() += 1;
                }
                if recent.len() == recent_count {
                    recent.pop_front();
                }
                let fetched = cpu.bus.fetch(pc).unwrap_or(0);
                recent.push_back((pc, op, fetched, cpu.r));
            }
            Err(error) => {
                fault = Some(error);
                break;
            }
        }
        if let Some(address) = memwatch {
            let value = cpu.bus.read(address, 4).ok();
            if value != memwatch_value && memwatch_hits < 400 {
                memwatch_hits += 1;
                eprintln!(
                    "memwatch {step}: {} wrote 0x{address:08x}: {:x?} -> {:x?}",
                    location(&firmware.symbols, pc),
                    memwatch_value,
                    value
                );
            }
            memwatch_value = value;
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
    if let Some(pc) = cpu.secondary_pc() {
        eprintln!("secondary core: pc {}", location(&firmware.symbols, pc));
    }
    eprintln!(
        "LCD: {} pixels written; visible={}",
        cpu.bus.lcd.pixels_written,
        cpu.bus.screen_visible()
    );
    eprintln!(
        "interrupts: {}; USB: {} host setups, {} packets, {} CDC bytes",
        cpu.irq_entries, cpu.bus.usb.setups, cpu.bus.usb.packets, serial_bytes
    );
    eprintln!(
        "watchdog: {} feeds; timeout {:?} ticks, {} since the last feed",
        cpu.bus.system.watchdog_feeds,
        cpu.bus.system.watchdog_timeout(),
        cpu.bus.watchdog_ticks()
    );
    eprintln!(
        "audio: {} stereo frames, {} DMA halves; ADC: {} conversions",
        cpu.bus.audio.frames, cpu.bus.audio.halves, cpu.bus.devices.adc.conversions
    );
    if watchdog_off {
        eprintln!("EXPERIMENT: watchdog expiry disabled (FM1_WATCHDOG_OFF=1)");
    }
    if cpu.bus.devices.timer4.lsb_stub_used || cpu.bus.devices.timer5.lsb_stub_used {
        eprintln!("STUB timer: lsb_clk source counted at the oscillator rate");
    }
    if cpu.bus.spi2.transfers > 0 {
        eprintln!(
            "STUB SPI2 (no device): {} transfers",
            cpu.bus.spi2.transfers
        );
        // Optional: FM1_SPI2_LOG=1 lists the first SPI2 transfers.
        if env::var("FM1_SPI2_LOG").is_ok() {
            for (index, value, con) in &cpu.bus.spi2.log {
                let kind = if *index == 2 { "BUF" } else { "DMA CNT" };
                eprintln!("  SPI2 {kind} {value:#x} (CON {con:#x})");
            }
        }
    }
    let radio = cpu.bus.radio.accesses.get();
    if radio > 0 {
        eprintln!("STUB radio (JL_WL, no RF emulated): {radio} register accesses");
    }
    if let Some(&address) = firmware.symbols.get("felucca_dbg") {
        // Existing guest diagnostics from Felucca's audio.c; no guest hooks.
        if cpu.bus.read(address, 4).ok() == Some(0x44424731) {
            let read = |offset| cpu.bus.read(address + offset, 4).unwrap_or(0);
            eprintln!(
                "Felucca: {} UI frames, {} rendered audio halves, {} timer IRQs; stage={}, page={}, home={}",
                read(28), read(4), read(24), read(44), read(48), read(52)
            );
        }
    }
    if let Some(name) = &op_filter {
        eprintln!("{name}: {} distinct PCs", op_pcs.len());
        for (pc, count) in &op_pcs {
            eprintln!("  {count:>9} {}", location(&firmware.symbols, *pc));
        }
    }
    if !callers.is_empty() {
        let mut ranked: Vec<_> = callers.into_iter().collect();
        ranked.sort_by_key(|&(r, count)| (std::cmp::Reverse(count), r));
        eprintln!("callers (rets) at the FM1_CALLERS PC:");
        for (rets, count) in ranked.into_iter().take(16) {
            eprintln!("  {count:>9} 0x{rets:08x}");
        }
    }
    if !hot.is_empty() {
        let mut ranked: Vec<_> = hot.into_iter().collect();
        ranked.sort_by_key(|&(pc, count)| (std::cmp::Reverse(count), pc));
        eprintln!("hottest primary-core PCs in the final {hot_window} steps:");
        for (pc, count) in ranked.into_iter().take(24) {
            eprintln!("  {count:>9} {}", location(&firmware.symbols, pc));
        }
    }
    if let Some(stats) = cpu.bus.mmio_stats.borrow_mut().take() {
        let mmio_top: usize = env::var("FM1_MMIO_TOP")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(40);
        let mut ranked: Vec<_> = stats.into_iter().collect();
        ranked.sort_by_key(|&(key, [r, w])| (std::cmp::Reverse(r + w), key));
        eprintln!("MMIO accesses in the final {mmio_window} steps (reads, writes):");
        for ((address, pc), [r, w]) in ranked.into_iter().take(mmio_top) {
            eprintln!("  0x{address:08x}: {r:>9} {w:>9}  pc 0x{pc:08x}");
        }
    }
    // Optional: FM1_DUMP=ADDRESS:BYTES[,ADDRESS:BYTES] hex-dumps guest memory.
    if let Ok(requests) = env::var("FM1_DUMP") {
        for request in requests.split(',') {
            let (address, length) = request.split_once(':').ok_or("FM1_DUMP needs A:N")?;
            let address = u32::from_str_radix(address.trim_start_matches("0x"), 16)
                .map_err(|_| "invalid FM1_DUMP address")?;
            let length: u32 = length.parse().map_err(|_| "invalid FM1_DUMP length")?;
            for line in (0..length).step_by(16) {
                let bytes: Vec<_> = (line..(line + 16).min(length))
                    .map(|i| match cpu.bus.read(address + i, 1) {
                        Ok(byte) => format!("{byte:02x}"),
                        Err(_) => "??".into(),
                    })
                    .collect();
                eprintln!("  {:08x}: {}", address + line, bytes.join(" "));
            }
        }
    }
    // Optional: FM1_RAM=PATH saves all guest SRAM for offline inspection.
    if let Ok(path) = env::var("FM1_RAM") {
        let ram: Vec<u8> = (0..fm1_emu::RAM_SIZE as u32)
            .map(|i| cpu.bus.read(fm1_emu::RAM + i, 1).unwrap_or(0) as u8)
            .collect();
        std::fs::write(&path, ram).map_err(|e| format!("{path}: {e}"))?;
        eprintln!("RAM: saved {path}");
    }
    // Optional: FM1_PNG=PATH saves the raw panel framebuffer (ungated).
    if let Ok(path) = env::var("FM1_PNG") {
        let png = fm1_emu::png::encode_rgb(
            fm1_emu::lcd::WIDTH,
            fm1_emu::lcd::HEIGHT,
            &cpu.bus.lcd.pixels,
        )?;
        std::fs::write(&path, png).map_err(|e| format!("{path}: {e}"))?;
        eprintln!("framebuffer: saved {path}");
    }
    eprintln!("recent completed instructions:");
    for (pc, op, word, registers) in recent {
        if recent_count > 12 {
            let list: Vec<_> = registers.iter().map(|r| format!("{r:x}")).collect();
            eprintln!(
                "  {}: {word:04x} {op} [{}]",
                location(&firmware.symbols, pc),
                list.join(" ")
            );
        } else {
            eprintln!("  {}: {op}", location(&firmware.symbols, pc));
        }
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
