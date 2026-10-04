// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{bus::Bus, cpu::Cpu, firmware::Firmware};
use std::path::Path;

fn firmware(extension: &str) -> Firmware {
    Firmware::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../build/display/firmware.{extension}")),
    )
    .unwrap()
}
fn frame(cpu: &mut Cpu, stop: u32) {
    cpu.step().unwrap();
    cpu.run(Some(stop), cpu.steps + 200_000, None).unwrap();
}
#[test]
fn guest_draws_live_timer_and_matrix_pixels_via_spi_dma() {
    let firmware = firmware("elf");
    let stop = firmware.symbols["display_frame_done"];
    let mut cpu = Cpu::new(Bus::new(firmware.image).unwrap(), firmware.entry);
    frame(&mut cpu, stop);
    assert!(cpu.bus.screen_visible());
    assert_eq!(cpu.irq_entries, 1); // Five-foundation startup still ran.
    assert_eq!(
        cpu.bus
            .read(firmware.symbols["foundation_results"] + 36, 4)
            .unwrap(),
        0x50f00d
    );
    assert!(cpu.bus.lcd.pixels_written > 240 * 240);
    assert_eq!(cpu.bus.lcd.pixels[0], 0x101010);
    assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0x313031);
    let first = cpu.bus.lcd.pixels.clone();
    let first_ticks = cpu.bus.read(firmware.symbols["display_ticks"], 4).unwrap();
    cpu.bus.devices.gpio.press(0, 4, true).unwrap(); // OCT-minus
    frame(&mut cpu, stop);
    assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0xf7cb00);
    assert_eq!(
        cpu.bus.read(firmware.symbols["matrix_results"], 4).unwrap(),
        0xe1
    );
    assert_ne!(
        cpu.bus.read(firmware.symbols["display_ticks"], 4).unwrap(),
        first_ticks
    );
    assert_ne!(
        &first[80 * 240..110 * 240],
        &cpu.bus.lcd.pixels[80 * 240..110 * 240]
    );
    cpu.bus.devices.gpio.press(0, 4, false).unwrap();
    frame(&mut cpu, stop);
    assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0x313031);
    assert_eq!(
        cpu.bus.read(firmware.symbols["display_frames"], 4).unwrap(),
        3
    );
}
#[test]
fn raw_and_elf_images_produce_identical_lcd_output() {
    let elf = firmware("elf");
    let stop = elf.symbols["display_frame_done"];
    let run = |f: Firmware| {
        let mut cpu = Cpu::new(Bus::new(f.image).unwrap(), f.entry);
        frame(&mut cpu, stop);
        cpu.bus.lcd.pixels
    };
    assert_eq!(run(elf), run(firmware("bin")));
}

#[test]
fn short_calls_use_signed_offsets_and_save_the_following_address() {
    // Encodings/offsets emitted by the vendor assembler in display/firmware.dis.
    for (instruction, displacement) in [(0x8201u16, 4i32), (0x8071, -64), (0x9731, 238)] {
        let mut image = vec![0; 512];
        image[256..258].copy_from_slice(&instruction.to_le_bytes());
        let target = (258 + displacement) as usize;
        image[target..target + 2].copy_from_slice(&0x0080u16.to_le_bytes());
        let mut cpu = Cpu::new(Bus::new(image).unwrap(), fm1_emu::XIP + 256);
        cpu.step().unwrap();
        assert_eq!(cpu.pc, fm1_emu::XIP + target as u32);
        assert_eq!(cpu.sr[3], fm1_emu::XIP + 258);
        cpu.step().unwrap();
        assert_eq!(cpu.pc, fm1_emu::XIP + 258);
    }
}
