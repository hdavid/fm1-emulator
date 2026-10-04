// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    bus::Bus,
    cpu::{Cpu, Fault},
    firmware::Firmware,
    RAM, RAM_SIZE, XIP,
};
use std::{fs, path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn cpu(bytes: &[u8]) -> Cpu {
    Cpu::new(Bus::new(bytes.to_vec()).unwrap(), XIP)
}

#[test]
fn elf_reconstructs_the_exact_flash_application() {
    let firmware = Firmware::load(&root().join("build/fm1-diag.elf")).unwrap();
    assert_eq!(
        firmware.image,
        fs::read(root().join("build/fm1-diag.bin")).unwrap()
    );
    assert_eq!(firmware.entry, XIP);
    assert_eq!(
        firmware.symbols["fm1_probe_end"] - firmware.symbols["fm1_probe"],
        114
    );
}

#[test]
fn unchanged_firmware_probe_matches_the_physical_fm1() {
    let firmware = Firmware::load(&root().join("build/fm1-diag.elf")).unwrap();
    let mut cpu = Cpu::new(
        Bus::new(firmware.image).unwrap(),
        firmware.symbols["fm1_probe"],
    );
    let capture = fs::read_to_string(root().join("build/hardware-initial.txt")).unwrap();
    let mut hardware = [None; 12];
    for line in capture.lines().filter(|line| line.starts_with("W ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let i: usize = fields[1].parse().unwrap();
        assert!(hardware[i].is_none());
        hardware[i] = Some(u32::from_str_radix(fields[2], 16).unwrap());
    }
    assert_eq!(
        cpu.probe(1000, None).unwrap(),
        hardware.map(|value| value.unwrap())
    );
    assert_eq!(cpu.steps, 70);
}

#[test]
fn full_probe_image_preserves_registers_and_stack() {
    let firmware = Firmware::load(&root().join("build/probe.elf")).unwrap();
    assert_eq!(
        firmware.image,
        fs::read(root().join("build/probe.bin")).unwrap()
    );
    let stop = firmware.symbols["probe_done"];
    let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
    cpu.r = std::array::from_fn(|n| 0x1020_3040 + n as u32 * 0x0101_0101);
    let before = cpu.r;
    cpu.run(Some(stop), 1000, None).unwrap();
    assert_eq!(cpu.steps, 75);
    assert_eq!(cpu.r[1..], before[1..]);
    assert_eq!(cpu.sr[14], 0x01c7_a000);
}

#[test]
fn raw_firmware_cli_accepts_an_explicit_probe_entry() {
    let output = Command::new(env!("CARGO_BIN_EXE_fm1-emu"))
        .args(["probe", "build/fm1-diag.bin", "--entry", "0x02002bc2"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"constant\":305419896"));
}

#[test]
fn bus_checks_alignment_boundaries_and_flash_writes() {
    let mut bus = Bus::new(vec![0x80, 0x00]).unwrap();
    bus.write(RAM, 0x1234_5678, 4).unwrap();
    assert_eq!(bus.read(RAM, 1).unwrap(), 0x78);
    assert_eq!(bus.read(RAM + 2, 2).unwrap(), 0x1234);
    assert!(bus.write(RAM + 1, 0, 4).is_err());
    assert!(bus.write(XIP, 0, 4).is_err());
    assert!(bus.read(RAM + RAM_SIZE as u32, 4).is_err());
    assert!(bus.read(0xffff_fffc, 4).is_err());
    assert!(bus.read(0x10800, 4).is_err());
    assert!(bus.read(RAM, 3).is_err());
    bus.write(RAM, 0x80, 2).unwrap();
    assert_eq!(bus.fetch(RAM).unwrap(), 0x80);
}

#[test]
fn arithmetic_wraps_and_logic_uses_vendor_encodings() {
    type ArithmeticCase = (u16, fn(u32, u32) -> u32);
    let cases: [ArithmeticCase; 8] = [
        (0x1c91, u32::wrapping_add),
        (0x1e91, u32::wrapping_sub),
        (0x1929, |a, b| a ^ b),
        (0x19a1, |a, b| a & b),
        (0x1921, |a, b| a | b),
        (0x19a9, |_, b| !b),
        (0xa421, |_, b| b << 4),
        (0xa4a1, |_, b| b >> 4),
    ];
    for (word, expected) in cases {
        for (a, b) in [
            (0, 0),
            (u32::MAX, 1),
            (0x8000_0000, u32::MAX),
            (0xa5a5_5a5a, 0x1234_5678),
        ] {
            let mut cpu = cpu(&word.to_le_bytes());
            cpu.r[1] = a;
            cpu.r[2] = b;
            cpu.step().unwrap();
            assert_eq!(cpu.r[1], expected(a, b), "opcode {word:04x}");
        }
    }
}

#[test]
fn branch_and_instruction_limits_do_not_succeed_silently() {
    for (value, target) in [(0, XIP + 2), (1, XIP - 4), (u32::MAX, XIP - 4)] {
        let mut cpu = cpu(&[0xf3, 0x5d]);
        cpu.r[3] = value;
        cpu.step().unwrap();
        assert_eq!(cpu.pc, target);
    }
    let mut looping = cpu(&[0xf7, 0x9f]);
    assert_eq!(
        looping.run(None, 3, None),
        Err(Fault::Limit { pc: XIP, limit: 3 })
    );
    assert!(matches!(
        cpu(&[0xff, 0xff]).step(),
        Err(Fault::Unsupported { .. })
    ));
    assert!(matches!(
        cpu(&[0xc0, 0xff]).step(),
        Err(Fault::Access { .. })
    ));
}

#[test]
fn malformed_elf_is_rejected_without_panicking() {
    let image = fs::read(root().join("build/fm1-diag.elf")).unwrap();
    for length in [0, 4, 16, 51, 100] {
        assert!(Firmware::from_elf(&image[..length]).is_err());
    }
    for (offset, patch) in [
        (18, vec![0, 0]),
        (28, u32::MAX.to_le_bytes().to_vec()),
        (24, 0x10000u32.to_le_bytes().to_vec()),
        (42, vec![0, 0]),
        (32, u32::MAX.to_le_bytes().to_vec()),
    ] {
        let mut bad = image.clone();
        bad[offset..offset + patch.len()].copy_from_slice(&patch);
        assert!(Firmware::from_elf(&bad).is_err(), "offset {offset}");
    }
}

#[test]
fn startup_stops_at_the_first_unmodeled_timer_register() {
    let firmware = Firmware::load(&root().join("build/fm1-diag.elf")).unwrap();
    let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
    cpu.r[0] = 0x01c7_fe08;
    match cpu.run(None, 100, None).unwrap_err() {
        Fault::Access { pc, fault } => {
            assert_eq!(pc, 0x0200_1db4);
            assert_eq!(fault.address, 0x10800);
        }
        fault => panic!("unexpected fault: {fault}"),
    }
    assert_eq!(cpu.steps, 17);
    assert_eq!(cpu.bus.read(0x01c0_7f28, 4).unwrap(), 0x01c7_fe08);
}
