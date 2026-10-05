// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    firmware::Firmware,
    gpio::{DIE, DIR, GPIO, IN, OUT, PU},
};
use std::{path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn configured() -> Bus {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    bus.write(GPIO + DIR, 0x1e1, 4).unwrap();
    bus.write(GPIO + DIE, 0x1fb, 4).unwrap();
    bus.write(GPIO + PU, 0x1e1, 4).unwrap();
    bus.write(GPIO + 0x40 + DIR, 0x80, 4).unwrap();
    bus.write(GPIO + 0x40 + DIE, 0x80, 4).unwrap();
    bus.write(GPIO + 0x40 + PU, 0x80, 4).unwrap();
    bus
}

fn select(bus: &mut Bus, word: u16) {
    for bit in (0..16).rev() {
        let serial = if word & (1 << bit) != 0 { 16 } else { 0 };
        bus.write(GPIO + OUT, serial, 4).unwrap();
        bus.write(GPIO + OUT, serial | 8, 4).unwrap();
        // Repeated high levels must not clock another serial bit.
        bus.write(GPIO + OUT, serial | 8, 4).unwrap();
        bus.write(GPIO + OUT, serial, 4).unwrap();
    }
    bus.write(GPIO + OUT, 2, 4).unwrap();
    bus.write(GPIO + OUT, 0, 4).unwrap();
}

#[test]
fn shift_registers_select_active_low_columns_and_both_row_ports() {
    let mut bus = configured();
    bus.devices.gpio.press(0, 4, true).unwrap(); // OCT-minus, PA8
    bus.devices.gpio.press(3, 2, true).unwrap(); // independent PA6 row
    bus.devices.gpio.press(3, 5, true).unwrap(); // encoder row PB7
    select(&mut bus, 0xfffe);
    assert_eq!(bus.devices.gpio.latched, 0xfffe);
    assert_eq!(bus.read(GPIO + IN, 4).unwrap(), 0xe1);
    assert_eq!(bus.read(GPIO + 0x40 + IN, 4).unwrap(), 0x80);
    select(&mut bus, 0xfff7);
    assert_eq!(bus.read(GPIO + IN, 4).unwrap(), 0x1a1);
    assert_eq!(bus.read(GPIO + 0x40 + IN, 4).unwrap(), 0);
    select(&mut bus, u16::MAX);
    assert_eq!(bus.read(GPIO + IN, 4).unwrap(), 0x1e1);
    bus.devices.gpio.press(0, 4, false).unwrap();
    select(&mut bus, 0xfffe);
    assert_eq!(bus.read(GPIO + IN, 4).unwrap(), 0x1e1);
}

#[test]
fn disabled_output_drivers_do_not_clock_the_matrix() {
    let mut bus = configured();
    bus.write(GPIO + DIR, u32::MAX, 4).unwrap();
    select(&mut bus, 0xfffe);
    assert_eq!(bus.devices.gpio.latched, u16::MAX);
    assert!(bus.write(GPIO + IN, 0, 4).is_err());
    assert!(bus.write(GPIO + OUT, 0, 1).is_err());
    assert!(bus.read(GPIO + 0x20, 4).is_err());
    assert!(bus.devices.gpio.press(11, 0, true).is_err());
    assert!(bus.devices.gpio.press(0, 6, true).is_err());
}

#[test]
fn booted_guest_scans_all_eleven_columns_without_ghost_keys() {
    for keys in [
        vec![],
        vec![(0, 4)],
        vec![(3, 4)],
        vec![(0, 4), (3, 4), (10, 2)],
    ] {
        let firmware = Firmware::load(&root().join("build/foundation/firmware.elf")).unwrap();
        let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
        for &(column, row) in &keys {
            cpu.bus.devices.gpio.press(column, row, true).unwrap();
        }
        cpu.run(Some(firmware.symbols["foundation_done"]), 10000, None)
            .unwrap();
        for column in 0..11 {
            let mut expected = 0x1e1;
            for &(key_column, row) in &keys {
                if column == key_column {
                    expected &= !(if row == 0 { 1 } else { 1 << (row + 4) });
                }
            }
            assert_eq!(
                cpu.bus
                    .read(firmware.symbols["matrix_results"] + column as u32 * 4, 4)
                    .unwrap(),
                expected,
                "column {column}, keys {keys:?}"
            );
        }
        assert_eq!(cpu.bus.devices.gpio.latched, 0xfbff);
        assert_eq!(cpu.irq_entries, 1);
        assert_eq!(
            cpu.bus
                .read(firmware.symbols["foundation_results"] + 36, 4)
                .unwrap(),
            0x50_f00d
        );
    }
}

#[test]
fn raw_and_elf_firmware_boots_produce_the_same_observations() {
    let firmware = Firmware::load(&root().join("build/foundation/firmware.elf")).unwrap();
    let run = |path: &str, until: String, inspection: String| {
        let output = Command::new(env!("CARGO_BIN_EXE_fm1-emu"))
            .args([
                "boot",
                path,
                "--until",
                &until,
                "--inspect",
                &inspection,
                "--press",
                "0:4",
            ])
            .current_dir(root())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    assert_eq!(
        run(
            "build/foundation/firmware.elf",
            "foundation_done".into(),
            "foundation_results:10".into()
        ),
        run(
            "build/foundation/firmware.bin",
            format!("0x{:x}", firmware.symbols["foundation_done"]),
            format!("0x{:x}:10", firmware.symbols["foundation_results"])
        )
    );
}

#[test]
fn spi_dma_clocks_matrix_before_gpio_latch_and_acknowledges_its_irq() {
    use fm1_emu::{devices::IRQ_CONFIG, lcd::IOMAP, RAM, XIP};
    let mut bus = configured();
    // Source 37, priority 5. Startup installs the descriptor before enabling.
    bus.write(IRQ_CONFIG + 4 * 4, 11 << 20, 4).unwrap();
    bus.write(0x10014, 6, 4).unwrap(); // 360 MHz system, 60 MHz LSB
    bus.write(IOMAP, 0x20000, 4).unwrap();
    bus.write(RAM, 0xfeff, 2).unwrap(); // MSB-first serial bytes ff fe
    bus.write(0x11e00, 0x6020, 4).unwrap();
    bus.write(0x11e04, 29, 4).unwrap();
    bus.write(0x11e0c, RAM, 4).unwrap();
    bus.write(0x11e10, 2, 4).unwrap();
    bus.write(0x11e00, 0x6021, 4).unwrap();
    bus.devices.gpio.press(0, 4, true).unwrap();
    bus.devices.gpio.press(3, 2, true).unwrap();
    let mut c = Cpu::new(bus, XIP);
    // Replace the two-byte fixture with NOPs in SRAM to advance real CPU time.
    c.pc = RAM + 1024;
    for _ in 0..191 {
        c.step().unwrap();
    }
    assert_eq!(c.bus.pending_irq(0x100), None);
    c.step().unwrap();
    assert_eq!(c.bus.pending_irq(0x100), Some(37));
    assert_eq!(c.bus.devices.gpio.latched, 0xffff);
    c.bus.write(GPIO + OUT, 2, 4).unwrap();
    c.bus.write(GPIO + OUT, 0, 4).unwrap();
    assert_eq!(c.bus.devices.gpio.latched, 0xfffe);
    assert_eq!(c.bus.read(GPIO + IN, 4).unwrap(), 0xe1);
    c.bus.write(0x11e00, 0x6021, 4).unwrap();
    assert_eq!(c.bus.pending_irq(0x100), None);
    assert_eq!(c.bus.read(0x11e10, 4).unwrap(), 0);
}

#[test]
fn matrix_spi_rejects_invalid_dma_and_other_pin_routes() {
    use fm1_emu::{lcd::IOMAP, RAM};
    let mut bus = configured();
    bus.write(0x11e00, 0x6021, 4).unwrap();
    bus.write(0x11e0c, RAM, 4).unwrap();
    assert!(bus.write(0x11e10, 2, 4).is_err());
    bus.write(IOMAP, 0x20000, 4).unwrap();
    bus.write(0x11e0c, RAM + 512 * 1024 - 1, 4).unwrap();
    assert!(bus.write(0x11e10, 2, 4).is_err());
    assert!(bus.write(0x11e00, 0, 1).is_err());
    assert!(bus.read(0x11e00, 2).is_err());
}
