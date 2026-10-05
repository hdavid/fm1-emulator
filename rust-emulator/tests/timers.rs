// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    bus::Bus,
    cpu::{Cpu, Fault},
    devices::{IRQ_CONFIG, TIMER4, TIMER5},
    firmware::Firmware,
    SYSTEM_STACK, USER_STACK,
};
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}
fn bus() -> Bus {
    Bus::new(vec![0, 0]).unwrap()
}

#[test]
fn timer4_counts_wraps_and_acknowledges_pending() {
    let mut bus = bus();
    bus.write(TIMER4 + 8, u32::MAX, 4).unwrap();
    bus.write(TIMER4 + 4, u32::MAX - 12, 4).unwrap();
    bus.write(TIMER4, 0x4009, 4).unwrap();
    bus.devices.advance(24);
    assert_eq!(bus.read(TIMER4 + 4, 4).unwrap(), 11);
    assert!(bus.devices.timer4.pending);
    bus.write(TIMER4, 0x4009, 4).unwrap();
    assert!(!bus.devices.timer4.pending);
    assert_eq!(bus.read(TIMER4 + 4, 4).unwrap(), 11);
    bus.write(TIMER4, 0x4000, 4).unwrap();
    bus.devices.advance(24);
    assert_eq!(bus.read(TIMER4 + 4, 4).unwrap(), 11);
}

#[test]
fn timer5_prescaler_period_and_interrupt_masks() {
    let mut bus = bus();
    bus.write(TIMER5 + 8, 600, 4).unwrap();
    bus.write(TIMER5, 0x4019, 4).unwrap();
    bus.devices.advance(2399);
    assert_eq!(bus.read(TIMER5 + 4, 4).unwrap(), 599);
    assert!(!bus.devices.timer5.pending);
    bus.devices.advance(1);
    assert_eq!(bus.read(TIMER5 + 4, 4).unwrap(), 0);
    assert_eq!(bus.devices.pending_irq(0x100), None);
    bus.write(IRQ_CONFIG + 7 * 4, 0x3000_0000, 4).unwrap();
    assert_eq!(bus.devices.pending_irq(0), None);
    assert_eq!(bus.devices.pending_irq(0x100), Some(63));
    bus.write(TIMER5, 0x4019, 4).unwrap();
    assert_eq!(bus.devices.pending_irq(0x100), None);
    assert!(bus.write(TIMER5, 0x4029, 4).is_ok());
    assert!(bus.write(TIMER5, 0x4005, 4).is_err());
    assert!(bus.write(TIMER5, 0, 1).is_err());
}

#[test]
fn software_irq_context_matches_all_eight_measured_priorities() {
    for priority in 0..8 {
        let mut c = Cpu::new(Bus::new(vec![0x61, 0, 0, 0]).unwrap(), fm1_emu::XIP);
        let handler = fm1_emu::RAM + 512;
        c.bus.write(handler, 0x0081, 2).unwrap();
        c.bus.write(0x01c7fe00 + 120 * 4, handler, 4).unwrap();
        c.bus
            .write(IRQ_CONFIG + 15 * 4, 1 | priority << 1, 4)
            .unwrap();
        c.bus.write(0x1eef1a0, 1, 4).unwrap();
        c.sr[11] = 0x100;
        c.sr[14] = USER_STACK;
        c.sr[13] = SYSTEM_STACK;
        c.step().unwrap();
        assert_eq!(c.sr[11], priority << 24 | 0x00780300 | 1 << priority);
        assert_eq!(c.bus.read(0x1eef1a8, 4).unwrap(), 0);
        c.bus.write(0x1eef1a4, 1, 4).unwrap();
        c.step().unwrap();
        c.step().unwrap(); // nop after return; source/priority remain latched.
        assert_eq!(c.sr[11], priority << 24 | 0x00780700);
    }
    let mut bus = Bus::new(vec![0; 4]).unwrap();
    bus.write(IRQ_CONFIG + 15 * 4, 0x55, 4).unwrap();
    bus.write(0x1eef1a0, 3, 4).unwrap();
    assert_eq!(bus.pending_irq(0x100), Some(120)); // Equal priorities: lower source first.
    bus.write(IRQ_CONFIG + 15 * 4, 0xb5, 4).unwrap();
    assert_eq!(bus.pending_irq(0x100), Some(121));
    bus.write(0x1eef1a8, 6, 4).unwrap();
    assert_eq!(bus.pending_irq(0x100), None);
}

#[test]
fn irq_context_matches_hardware_without_overwriting_the_guest_mask() {
    let mut c = Cpu::new(Bus::new(vec![0x61, 0, 0, 0]).unwrap(), fm1_emu::XIP);
    let handler = fm1_emu::RAM + 512;
    c.bus.write(handler, 0x0081, 2).unwrap();
    c.bus.write(0x01c7fe00 + 127 * 4, handler, 4).unwrap();
    c.bus.write(IRQ_CONFIG + 15 * 4, 0xb0000000, 4).unwrap();
    c.bus.write(0x1eef1a0, 128, 4).unwrap();
    c.sr[11] = 0x100;
    c.sr[14] = USER_STACK;
    c.sr[13] = SYSTEM_STACK;
    c.step().unwrap();
    assert_eq!(c.pc, handler);
    assert_eq!(c.bus.read(0x1eef1a8, 4).unwrap(), 0);
    assert_eq!(c.sr[11], 0x057f0320);
    c.bus.write(0x1eef1a4, 128, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(0x1eef1a8, 4).unwrap(), 0);
    assert_eq!(c.sr[11], 0x057f0700);
}

#[test]
fn firmware_boot_initializes_ram_and_services_a_real_guest_isr() {
    let firmware = Firmware::load(&root().join("build/foundation/firmware.elf")).unwrap();
    assert_eq!(
        firmware.image,
        fs::read(root().join("build/foundation/firmware.bin")).unwrap()
    );
    let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
    for symbol in ["foundation_bss", "foundation_irqs", "foundation_irq_sp"] {
        cpu.bus
            .write(firmware.symbols[symbol], 0xdead_beef, 4)
            .unwrap();
    }
    assert_eq!(
        cpu.bus
            .read(firmware.symbols["foundation_data"], 4)
            .unwrap(),
        0
    );
    assert_eq!(cpu.bus.read(firmware.symbols["ram_magic"], 4).unwrap(), 0);
    cpu.run(Some(firmware.symbols["foundation_done"]), 150000, None)
        .unwrap();
    let address = firmware.symbols["foundation_results"];
    let values: Vec<_> = (0..10)
        .map(|i| cpu.bus.read(address + i * 4, 4).unwrap())
        .collect();
    assert_eq!(values[0], 0xc001_cafe);
    assert_eq!(values[1], 0);
    assert_eq!(values[2], 0x5a17);
    assert!(values[4] > values[3]);
    assert_eq!(values[6], 1);
    assert_eq!(values[7], SYSTEM_STACK - 28);
    assert_eq!(values[8], USER_STACK);
    assert_eq!(values[9], 0x50_f00d);
    assert_eq!(cpu.irq_entries, 1);
    assert!(!cpu.interrupts_enabled);
    assert!(!cpu.bus.devices.timer5.pending);
    let capture = fs::read_to_string(root().join("build/hardware-initial.txt")).unwrap();
    for line in capture.lines().filter(|line| line.starts_with("W ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let i: u32 = fields[1].parse().unwrap();
        let expected = u32::from_str_radix(fields[2], 16).unwrap();
        assert_eq!(
            cpu.bus
                .read(firmware.symbols["cpu_probe_results"] + i * 4, 4)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn an_interrupt_with_a_missing_vector_fails_visibly() {
    let mut cpu = Cpu::new(bus(), 0x0200_0120);
    cpu.sr[11] = 0x100;
    cpu.sr[13] = SYSTEM_STACK;
    cpu.interrupts_enabled = true;
    cpu.bus.write(IRQ_CONFIG + 7 * 4, 0x1000_0000, 4).unwrap();
    cpu.bus.write(TIMER5 + 8, 1, 4).unwrap();
    cpu.bus.write(TIMER5, 0x4009, 4).unwrap();
    cpu.bus.devices.advance(1);
    assert!(
        matches!(cpu.step(),Err(Fault::Access { fault, .. }) if fault.operation=="fetch" && fault.address==0)
    );
}
