// SPDX-License-Identifier: GPL-3.0-only
// Idle UART1 receive path: WL82.h JL_UART_TypeDef and fm1_uart.h.
// There is no host DIN-MIDI input. Receive counters stay empty, DMA buffers
// stay untouched, and no receive/timeout event is generated without bytes.
// Transmit requests remain explicit faults.
#[derive(Default)]
pub(crate) struct Uart {
    registers: [u32; 11],
}
impl Uart {
    fn index(address: u32) -> Option<usize> {
        let offset = address.checked_sub(0x12100)?;
        (offset <= 40 && offset.is_multiple_of(4)).then_some((offset / 4) as usize)
    }
    fn width(index: usize, size: usize) -> bool {
        match index {
            0..=2 | 10 => matches!(size, 2 | 4),
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
                _ => Ok(self.registers[index]),
            }
        })
    }
    pub(crate) fn write(
        &mut self,
        address: u32,
        value: u32,
        size: usize,
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
                    self.registers[0] = value & 0x37f;
                    Ok(())
                }
                1 | 2 => {
                    self.registers[index] = value & 65535;
                    Ok(())
                }
                3 => Err("UART byte transmission is not implemented"),
                6 if value != 0 => Err("UART transmit DMA is not implemented"),
                10 => Err("UART receive count latch is read-only"),
                _ => {
                    self.registers[index] = value;
                    Ok(())
                }
            }
        })
    }
}
