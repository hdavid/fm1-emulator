// SPDX-License-Identifier: GPL-3.0-only
// STUB: JL_SPI2 (WL82.h lsfr 0x1e00: CON, BAUD, BUF, ADR, CNT) with no
// attached device. The stock application configures it at 0x020049ee
// (CON from its SPI platform table). A byte or DMA transfer completes at
// after its shift time: bytes * 8 * (BAUD + 1) lsb_clk ticks, with
// lsb_clk ASSUMED to be 4x the 24 MHz oscillator (not modelled; the SDK
// derives it from the PLL). Then CON bit 15 (pending) sets, bit 14
// written clears it, and the bus reads back 0xff (an idle MISO line). The
// stock driver restarts a 2-byte DMA from its interrupt, so completing at
// once starved every task. `transfers` exposes the stub in
// diagnostics; nothing here is verified against an FM-1 SPI2 peripheral.
pub const BASE: u32 = 0x11e00;
/// IRQ_SPI2_IDX (SDK hwi.h).
pub const IRQ: usize = 37;
const LSB_PER_OSC: u64 = 4;

#[derive(Default)]
pub struct Spi2 {
    registers: [u32; 5],
    pub transfers: u64,
    /// lsb_clk ticks until the transfer in progress completes.
    busy: u64,
    /// First transfers for diagnostics: (register index, value, CON).
    pub log: Vec<(usize, u32, u32)>,
}

impl Spi2 {
    /// Pending (CON bit 15) with the interrupt enabled (bit 13; the stock
    /// driver writes CON = 0x6020: clear pending, IE, bit 5). The IE bit
    /// position is inferred from that write, not documented.
    pub fn pending_irq(&self) -> bool {
        self.registers[0] & 0xa000 == 0xa000
    }

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
        if matches!(index, 2 | 4) && self.log.len() < 64 {
            self.log.push((index, value, self.registers[0]));
        }
        let bytes = match index {
            2 => {
                self.registers[2] = 0xff;
                1
            }
            4 => value as u64,
            _ => return,
        };
        self.transfers += 1;
        self.busy = (bytes * 8 * ((self.registers[1] & 0xff) as u64 + 1)).max(1);
    }

    /// Oscillator ticks until the transfer in progress completes.
    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        (self.busy != 0).then(|| self.busy.div_ceil(LSB_PER_OSC))
    }

    /// Advance by oscillator ticks; a finished transfer sets pending.
    pub fn advance(&mut self, ticks: u32) {
        if self.busy == 0 {
            return;
        }
        self.busy = self.busy.saturating_sub(ticks as u64 * LSB_PER_OSC);
        if self.busy == 0 {
            self.registers[0] |= 0x8000;
        }
    }
}
