// SPDX-License-Identifier: GPL-3.0-only
// JL_UART1 (SDK WL82.h lsfr 0x2100: CON0, CON1, BAUD, BUF, OTCNT, TXADR,
// TXCNT, RXSADR, RXEADR, RXCNT, HRXCNT; each in a word slot). The FM-1 wires
// it to the MIDI DIN port (hal/fm1_uart.h). Pending bits follow the SDK's
// debug.c putchar and fm1_uart.h: CON0 bit 15 TX pending (bit 13 clears),
// bit 14 RX pending (bit 12 clears), bit 11 RX timeout pending (bit 10
// clears), bit 0 enable. No MIDI input is connected, so RX never fills
// (HRXCNT stays 0). A TX byte or TX DMA completes at once; the 31250 baud
// shift time is not modelled. Bytes written to BUF are kept for diagnostics.
pub const BASE: u32 = 0x12100;
const WORDS: usize = 11;

#[derive(Default)]
pub struct Uart1 {
    registers: [u32; WORDS],
    pub tx: Vec<u8>,
    pub tx_dma_bytes: u64,
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
            }
            10 => {} // HRXCNT is read-only
            _ => {
                self.registers[index] = value;
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
