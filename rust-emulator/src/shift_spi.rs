// SPDX-License-Identifier: GPL-3.0-only
// SPI2 drives the FM-1's 74HC595 chain on PA3/PA4. The guest supplies
// serial bytes; PA1's GPIO latch still determines when rows change.
pub(crate) const BASE: u32 = 0x11e00;
pub(crate) const IRQ: usize = 37;
#[derive(Default)]
pub(crate) struct ShiftSpi {
    registers: [u32; 5],
    data: Vec<u8>,
    remaining: u64,
    pending: bool,
}
impl ShiftSpi {
    pub(crate) fn read(&self, address: u32) -> Option<u32> {
        let offset = address.checked_sub(BASE)?;
        if offset >= 20 || offset & 3 != 0 {
            return None;
        }
        let index = (offset / 4) as usize;
        Some(
            self.registers[index]
                | if index == 0 && self.pending {
                    0x8000
                } else {
                    0
                },
        )
    }
    pub(crate) fn write(&mut self, address: u32, value: u32) -> bool {
        let index = ((address - BASE) / 4) as usize;
        let enabled = self.registers[0] & 1 != 0;
        if index == 0 {
            if value & 0x4000 != 0 {
                self.pending = false;
            }
            self.registers[0] = value & 0x3fff;
            if value & 1 == 0 {
                self.data.clear();
                self.remaining = 0;
            }
            !enabled && value & 1 != 0 && self.registers[4] != 0
        } else {
            self.registers[index] = value;
            enabled && matches!(index, 2 | 4)
        }
    }
    pub(crate) fn dma_address(&self) -> u32 {
        self.registers[3]
    }
    pub(crate) fn dma_length(&self) -> usize {
        self.registers[4] as usize
    }
    pub(crate) fn start(&mut self, bytes: &[u8], iomap: u32) -> Result<(), &'static str> {
        if self.registers[0] & !0x2000 != 0x21 || iomap & 0x20000 == 0 {
            return Err("unsupported matrix SPI configuration or pin routing");
        }
        if !self.data.is_empty() {
            return Err("matrix SPI transfer already in progress");
        }
        if bytes.is_empty() || bytes.len() > 2 {
            return Err("matrix SPI requires one or two serial bytes");
        }
        self.data.extend_from_slice(bytes);
        self.remaining = bytes.len() as u64 * 8 * (self.registers[1] as u64 + 1) * 24_000_000;
        Ok(())
    }
    pub(crate) fn advance(&mut self, ticks: u32, peripheral_hz: u32) -> Option<Vec<u8>> {
        if self.data.is_empty() {
            return None;
        }
        self.remaining = self
            .remaining
            .saturating_sub(ticks as u64 * peripheral_hz as u64);
        if self.remaining != 0 {
            return None;
        }
        self.pending = true;
        self.registers[4] = 0;
        Some(std::mem::take(&mut self.data))
    }
    /// Oscillator ticks at `peripheral_hz` until a transfer ends.
    pub(crate) fn ticks_to_event(&self, peripheral_hz: u32) -> Option<u64> {
        (!self.data.is_empty() && peripheral_hz != 0)
            .then(|| self.remaining.div_ceil(peripheral_hz as u64).max(1))
    }
    pub(crate) fn pending_irq(&self) -> bool {
        self.pending && self.registers[0] & 0x2001 == 0x2001
    }
}
