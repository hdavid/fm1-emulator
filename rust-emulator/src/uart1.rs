// SPDX-License-Identifier: GPL-3.0-only
// JL_UART1 (SDK WL82.h lsfr 0x2100: CON0, CON1, BAUD, BUF, OTCNT, TXADR,
// TXCNT, RXSADR, RXEADR, RXCNT, HRXCNT; each in a word slot). The FM-1 wires
// it to the MIDI DIN port (hal/fm1_uart.h). Pending bits follow the SDK's
// debug.c putchar and fm1_uart.h: CON0 bit 15 TX pending (bit 13 clears),
// bit 14 RX pending (bit 12 clears), bit 11 RX timeout pending (bit 10
// clears), bit 0 enable. MIDI IN: bytes given to `send` arrive on the RX
// line at 31250 baud (10 bits, 320 us = 7680 oscillator ticks each, back to
// back); each is written by the RX DMA at the end of its stop bit into the
// ring RXSADR..RXEADR (wrapping), and counted; a CON0 write with RDC (bit 7)
// latches the count into HRXCNT and restarts it (fm1_uart.h). RX needs UTEN
// (bit 0) and RXDMA (bit 6). A TX byte or TX DMA completes at once; the
// 31250 baud shift time is not modelled. Bytes written to BUF are kept for
// diagnostics.
pub const BASE: u32 = 0x12100;
const WORDS: usize = 11;
/// Oscillator ticks per MIDI byte: 10 bits at 31250 baud, 24 MHz.
pub const BYTE_TICKS: u64 = 24_000_000 * 10 / 31_250;

#[derive(Default)]
pub struct Uart1 {
    registers: [u32; WORDS],
    pub tx: Vec<u8>,
    pub tx_dma_bytes: u64,
    /// MIDI IN bytes not yet on the line, oldest first.
    rx_queue: std::collections::VecDeque<u8>,
    /// Ticks until the byte on the line has been received (0: line idle).
    rx_left: u64,
    /// Next ring offset the DMA writes, from RXSADR.
    rx_offset: u32,
    /// Bytes received since the last RDC latch.
    rx_count: u32,
    /// Bytes the DMA has written so far (diagnostics).
    pub rx_bytes: u64,
    /// Bytes that arrived with RX off or no ring (lost).
    pub rx_lost: u64,
}

impl Uart1 {
    pub fn contains(address: u32) -> bool {
        (BASE..BASE + 4 * WORDS as u32).contains(&address)
    }

    pub fn read(&self, address: u32, size: usize) -> u32 {
        let word = self.registers[((address - BASE) / 4) as usize];
        let shift = (address & 3) * 8;
        let mask = if size == 4 {
            u32::MAX
        } else {
            (1u32 << (size * 8)) - 1
        };
        (word >> shift) & mask
    }

    pub fn write(&mut self, address: u32, value: u32, size: usize) {
        let index = ((address - BASE) / 4) as usize;
        let shift = (address & 3) * 8;
        let mask = if size == 4 {
            u32::MAX
        } else {
            ((1u32 << (size * 8)) - 1) << shift
        };
        let value = ((value << shift) & mask) | (self.registers[index] & !mask);
        match index {
            0 => {
                let mut pending = self.registers[0] & 0xc800;
                for (clear, bit) in [(1 << 13, 1 << 15), (1 << 12, 1 << 14), (1 << 10, 1 << 11)] {
                    if value & clear != 0 {
                        pending &= !bit;
                    }
                }
                self.registers[0] = (value & !0xfc00) | pending;
                if value & 0x80 != 0 {
                    self.registers[10] = self.rx_count & 0xffff;
                    self.rx_count = 0;
                }
            }
            10 => {} // HRXCNT is read-only
            _ => {
                self.registers[index] = value;
                if index == 7 {
                    self.rx_offset = 0;
                }
                if self.registers[0] & 1 != 0 && matches!(index, 3 | 6) {
                    if index == 3 {
                        if self.tx.len() < 4096 {
                            self.tx.push(value as u8);
                        }
                    } else {
                        self.tx_dma_bytes += (value & 0xffff) as u64;
                    }
                    self.registers[0] |= 1 << 15;
                }
            }
        }
    }
}

impl Uart1 {
    /// Put MIDI bytes on the RX line: the first starts now (if the line is
    /// idle), the others follow back to back.
    pub fn send(&mut self, bytes: &[u8]) {
        self.rx_queue.extend(bytes.iter().copied());
        if self.rx_left == 0 && !self.rx_queue.is_empty() {
            self.rx_left = BYTE_TICKS;
        }
    }
    /// Bytes still waiting or on the line.
    pub fn rx_pending(&self) -> usize {
        self.rx_queue.len()
    }
    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        (self.rx_left > 0).then_some(self.rx_left)
    }
    /// Advance the RX line by `ticks`; a byte completed in that span is
    /// written into `ram` (the bus's SRAM at `ram_base`). Returns the SRAM
    /// offsets written, for the code cache.
    pub(crate) fn advance(&mut self, ticks: u32, ram: &mut [u8], ram_base: u32) -> Option<usize> {
        if self.rx_left == 0 {
            return None;
        }
        let ticks = ticks as u64;
        if ticks < self.rx_left {
            self.rx_left -= ticks;
            return None;
        }
        // Spans end at events (ticks_to_event), so at most one byte lands.
        self.rx_left = 0;
        let byte = self.rx_queue.pop_front()?;
        if !self.rx_queue.is_empty() {
            self.rx_left = BYTE_TICKS;
        }
        let (start, end) = (self.registers[7], self.registers[8]);
        let on = self.registers[0] & 0x41 == 0x41;
        let offset = start.wrapping_add(self.rx_offset).wrapping_sub(ram_base) as usize;
        if !on || end <= start || offset >= ram.len() {
            self.rx_lost += 1;
            return None;
        }
        ram[offset] = byte;
        self.rx_offset += 1;
        if start + self.rx_offset >= end {
            self.rx_offset = 0;
        }
        self.rx_count += 1;
        self.rx_bytes += 1;
        self.registers[0] |= 1 << 14;
        Some(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_bytes_land_in_the_ring_at_31250_baud_and_wrap() {
        let base = 0x01c0_0000;
        let mut ram = vec![0u8; 16];
        let mut u = Uart1::default();
        u.write(BASE + 0x1c, base + 4, 4); // RXSADR
        u.write(BASE + 0x20, base + 6, 4); // RXEADR: a 2-byte ring
        u.write(BASE, 0x41, 4); // UTEN + RXDMA
        u.send(&[0xF8, 0xFA, 0x90]);
        assert_eq!(u.ticks_to_event(), Some(BYTE_TICKS));
        assert_eq!(u.advance((BYTE_TICKS - 1) as u32, &mut ram, base), None);
        assert_eq!(u.advance(1, &mut ram, base), Some(4));
        assert_eq!(ram[4], 0xF8);
        assert_eq!(u.advance(BYTE_TICKS as u32, &mut ram, base), Some(5));
        assert_eq!(u.advance(BYTE_TICKS as u32, &mut ram, base), Some(4));
        assert_eq!((ram[4], ram[5]), (0x90, 0xFA));
        assert_eq!(u.ticks_to_event(), None);
        u.write(BASE, 0x41 | 0x80, 4); // RDC latches the count
        assert_eq!(u.read(BASE + 0x28, 4), 3);
        assert_eq!(u.rx_bytes, 3);
    }

    #[test]
    fn bytes_are_lost_while_rx_is_off() {
        let mut ram = vec![0u8; 16];
        let mut u = Uart1::default();
        u.send(&[0xF8]);
        assert_eq!(u.advance(BYTE_TICKS as u32, &mut ram, 0x01c0_0000), None);
        assert_eq!(u.rx_lost, 1);
    }
}
