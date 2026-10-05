// SPDX-License-Identifier: GPL-3.0-only
// STUB: JL_SPI2 (WL82.h lsfr 0x1e00: CON, BAUD, BUF, ADR, CNT) with no
// attached device. The stock application configures it at 0x020049ee
// (CON from its SPI platform table). A byte or DMA transfer completes at
// once: CON bit 15 (pending) sets, bit 14 written clears it, and the bus
// reads back 0xff (an idle MISO line). `transfers` exposes the stub in
// diagnostics; nothing here is verified against an FM-1 SPI2 peripheral.
pub const BASE: u32 = 0x11e00;

#[derive(Default)]
pub struct Spi2 {
    registers: [u32; 5],
    pub transfers: u64,
}

impl Spi2 {
    pub fn contains(address: u32) -> bool {
        (BASE..BASE + 20).contains(&address)
    }

    pub fn read(&self, address: u32) -> u32 {
        self.registers[((address - BASE) / 4) as usize]
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let index = ((address - BASE) / 4) as usize;
        if index == 0 {
            let pending = if value & 0x4000 != 0 {
                0
            } else {
                self.registers[0] & 0x8000
            };
            self.registers[0] = (value & 0x3fff) | pending;
            return;
        }
        self.registers[index] = value;
        match index {
            2 => {
                self.transfers += 1;
                self.registers[2] = 0xff;
                self.registers[0] |= 0x8000;
            }
            4 => {
                self.transfers += 1;
                self.registers[0] |= 0x8000;
            }
            _ => {}
        }
    }
}
