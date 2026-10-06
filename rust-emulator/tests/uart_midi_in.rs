// SPDX-License-Identifier: GPL-3.0-only
// MIDI bytes on the UART1 RX line (DIN/TRS MIDI IN) reach the guest's RX
// DMA ring at 31250 baud, as hal/fm1_uart.h configures it.
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

#[test]
fn midi_in_bytes_reach_the_rx_ring_one_byte_time_apart() {
    // nop; goto back to the nop
    let mut cpu = Cpu::new(Bus::new(vec![0, 0, 0xf7, 0x9e]).unwrap(), XIP);
    cpu.bus.set_instruction_clock(Some(24_000_000)); // one tick per step
    cpu.bus.write(0x1211c, RAM + 0x100, 4).unwrap(); // RXSADR
    cpu.bus.write(0x12120, RAM + 0x140, 4).unwrap(); // RXEADR
    cpu.bus.write(0x12100, 0x41, 2).unwrap(); // UTEN + RXDMA
    cpu.bus.uart_midi_send(&[0x90, 60, 100]);
    let byte = 24_000_000 * 10 / 31_250; // oscillator ticks per byte
    for _ in 0..byte - 2 {
        cpu.step().unwrap();
    }
    assert_eq!(
        cpu.bus.read(RAM + 0x100, 1).unwrap(),
        0,
        "still on the line"
    );
    for _ in 0..byte + 4 {
        cpu.step().unwrap();
    }
    assert_eq!(cpu.bus.read(RAM + 0x100, 4).unwrap(), 0x0064_3c90 & 0xffff);
    for _ in 0..byte {
        cpu.step().unwrap();
    }
    assert_eq!(cpu.bus.read(RAM + 0x100, 4).unwrap(), 0x0064_3c90);
    assert_eq!(cpu.bus.uart_rx_counts(), (0, 3, 0));
    cpu.bus.write(0x12100, 0x10c1, 2).unwrap(); // RDC latch, clear RX pending
    assert_eq!(cpu.bus.read(0x12128, 4).unwrap(), 3);
}
