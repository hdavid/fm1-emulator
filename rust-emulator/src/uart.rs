// SPDX-License-Identifier: GPL-3.0-only
// UART1: WL82.h JL_UART_TypeDef and the vendor uart.c/spec_uart.c.
// There is no host DIN-MIDI input. Receive counters stay empty, DMA buffers
// stay untouched, and no receive/timeout event is generated without bytes.
// TX supports 8N1 byte/DMA transfers. No host DIN-MIDI routing is provided.
#[derive(Default)]
pub(crate) struct Uart {
    registers: [u32; 11],
    transmit: Vec<u8>,
    remaining: u64,
    pending: bool,
    // Bounded observation of bytes that have actually finished transmitting.
    transmitted: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_midi_dma_transmits_three_bytes_at_31250_baud() {
        let mut uart = Uart::default();
        let ram = [0x90, 60, 127, 0xa5];
        uart.write(0x12100, 0x6d, 2, None).unwrap().unwrap();
        uart.write(0x12108, 383, 2, None).unwrap().unwrap();
        uart.write(0x12114, crate::RAM, 4, None).unwrap().unwrap();
        uart.write(0x12118, 3, 2, Some(&ram)).unwrap().unwrap();
        // PLL48M / (4 * (383 + 1)) = 31250 baud; three 8N1
        // frames require 960us, or 23040 oscillator ticks.
        uart.advance(23039, 48_000_000);
        assert!(!uart.pending_irq());
        assert!(uart.transmitted.is_empty());
        uart.advance(1, 48_000_000);
        assert!(uart.pending_irq());
        assert_eq!(uart.transmitted, [0x90, 60, 127]);
        assert_eq!(uart.read(0x12100, 2), Some(Ok(0x806d)));
        assert_eq!(uart.read(0x12128, 2), Some(Ok(0)));
        assert_eq!(ram, [0x90, 60, 127, 0xa5]);
        uart.write(0x12100, 0x206d, 2, None).unwrap().unwrap();
        assert!(!uart.pending_irq());
        assert_eq!(uart.read(0x12100, 2), Some(Ok(0x6d)));
    }

    #[test]
    fn byte_transmit_divider_and_interrupt_enable_are_independent() {
        let mut uart = Uart::default();
        uart.write(0x12100, 0x11, 2, None).unwrap().unwrap();
        uart.write(0x1210c, 0xf8, 1, None).unwrap().unwrap();
        // OSC source, divider 3, BAUD reset to zero: 30 ticks.
        uart.advance(29, 24_000_000);
        assert_eq!(uart.read(0x12100, 2), Some(Ok(0x11)));
        uart.advance(1, 24_000_000);
        assert_eq!(uart.transmitted, [0xf8]);
        assert!(!uart.pending_irq());
        uart.write(0x12100, 0x15, 2, None).unwrap().unwrap();
        assert!(uart.pending_irq());
        uart.write(0x12100, 0x2015, 2, None).unwrap().unwrap();
        assert!(!uart.pending_irq());
        // Read-only pending bits cannot manufacture receive/TX events.
        uart.write(0x12100, 0xc815, 2, None).unwrap().unwrap();
        assert_eq!(uart.read(0x12100, 2), Some(Ok(0x15)));
    }

    #[test]
    fn dma_checks_sram_bounds_and_disabled_transmit_does_not_complete() {
        let mut uart = Uart::default();
        let ram = [0x90, 60];
        uart.write(0x12114, crate::RAM + 1, 4, None)
            .unwrap()
            .unwrap();
        assert!(uart.write(0x12118, 2, 2, Some(&ram)).unwrap().is_err());
        uart.write(0x12114, crate::RAM, 4, None).unwrap().unwrap();
        assert!(uart.write(0x12118, 2, 2, Some(&ram)).unwrap().is_err());
        uart.write(0x12100, 5, 2, None).unwrap().unwrap();
        uart.write(0x12118, 2, 2, Some(&ram)).unwrap().unwrap();
        uart.write(0x12100, 0, 2, None).unwrap().unwrap();
        uart.advance(1_000_000, 24_000_000);
        assert!(uart.transmitted.is_empty());
        assert!(!uart.pending_irq());
        uart.write(0x12100, 7, 2, None).unwrap().unwrap();
        assert!(uart.write(0x1210c, 0x90, 1, None).unwrap().is_err());
        uart.write(0x12100, 5, 2, None).unwrap().unwrap();
        uart.write(0x12104, 1, 2, None).unwrap().unwrap();
        assert!(uart.write(0x1210c, 0x90, 1, None).unwrap().is_err());
    }
}
impl Uart {
    fn index(address: u32) -> Option<usize> {
        let offset = address.checked_sub(0x12100)?;
        (offset <= 40 && offset.is_multiple_of(4)).then_some((offset / 4) as usize)
    }
    fn width(index: usize, size: usize) -> bool {
        match index {
            0..=2 | 6 | 10 => matches!(size, 2 | 4),
            3 => matches!(size, 1 | 4),
            _ => size == 4,
        }
    }
    pub(crate) fn read(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        let index = Self::index(address)?;
        Some(if !Self::width(index, size) {
            Err("unsupported UART register access width")
        } else {
            match index {
                2 | 6 => Err("UART register is write-only"),
                3 => Err("UART byte input is not implemented"),
                10 => Ok(0), // RDC latches zero bytes received on an idle wire.
                0 => Ok(self.registers[0] | if self.pending { 0x8000 } else { 0 }),
                _ => Ok(self.registers[index]),
            }
        })
    }
    pub(crate) fn write(
        &mut self,
        address: u32,
        value: u32,
        size: usize,
        ram: Option<&[u8]>,
    ) -> Option<Result<(), &'static str>> {
        let index = Self::index(address)?;
        Some(if !Self::width(index, size) {
            Err("unsupported UART register access width")
        } else {
            match index {
                0 => {
                    // CON0 clear strobes: 0x3400 clears TX/RX/timeout;
                    // 0x80 latches HRXCNT. Pending bits 15/14/11 are RO.
                    // Preserve the receive configuration, never latch strobes
                    // or manufacture pending events on a quiet input.
                    if value & 0x2000 != 0 {
                        self.pending = false;
                    }
                    self.registers[0] = value & 0x37f;
                    if value & 1 == 0 {
                        self.transmit.clear();
                        self.remaining = 0;
                    }
                    Ok(())
                }
                1 | 2 => {
                    self.registers[index] = value & 65535;
                    Ok(())
                }
                3 => self.start(&[value as u8]),
                6 => {
                    let count = (value & 65535) as usize;
                    if count == 0 {
                        Ok(())
                    } else {
                        let bytes = ram.and_then(|ram| {
                            let offset = self.registers[5].checked_sub(crate::RAM)? as usize;
                            ram.get(offset..offset.checked_add(count)?)
                        });
                        match bytes {
                            Some(bytes) => self.start(bytes),
                            None => Err("UART transmit DMA source is outside SRAM"),
                        }
                    }
                }
                10 => Err("UART receive count latch is read-only"),
                _ => {
                    self.registers[index] = value;
                    Ok(())
                }
            }
        })
    }

    fn start(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.registers[0] & 3 != 1 || self.registers[1] != 0 {
            return Err("UART transmit requires enabled 8N1 configuration");
        }
        if !self.transmit.is_empty() {
            return Err("UART transmit already in progress");
        }
        let divider = if self.registers[0] & 16 == 0 { 4 } else { 3 };
        self.remaining =
            bytes.len() as u64 * 10 * divider * (self.registers[2] as u64 + 1) * 24_000_000;
        self.transmit.extend_from_slice(bytes);
        self.pending = false;
        Ok(())
    }

    /// Oscillator ticks at `hz` until a transmission in progress ends.
    pub(crate) fn ticks_to_event(&self, hz: u32) -> Option<u64> {
        (!self.transmit.is_empty() && self.registers[0] & 1 != 0 && hz != 0)
            .then(|| self.remaining.div_ceil(hz as u64).max(1))
    }

    pub(crate) fn advance(&mut self, ticks: u32, hz: u32) {
        if self.transmit.is_empty() || self.registers[0] & 1 == 0 {
            return;
        }
        self.remaining = self.remaining.saturating_sub(ticks as u64 * hz as u64);
        if self.remaining == 0 {
            self.pending = true;
            let keep = self.transmit.len().min(4096 - self.transmitted.len());
            self.transmitted.extend_from_slice(&self.transmit[..keep]);
            self.transmit.clear();
        }
    }

    pub(crate) fn pending_irq(&self) -> bool {
        self.pending && self.registers[0] & 5 == 5
    }
}
