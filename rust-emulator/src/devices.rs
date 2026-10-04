// SPDX-License-Identifier: GPL-3.0-only
// Functional (not cycle-accurate) register models from fm1_time.h/fm1_timer.h.
use crate::gpio::Gpio;
pub const TIMER4: u32 = 0x10800;
pub const TIMER5: u32 = 0x10900;
pub const IRQ_CONFIG: u32 = 0x01ee_f100;
pub const IRQ_PENDING: u32 = 0x01ee_f180;
pub const TIMER5_IRQ: usize = 63;
pub const TICK_TIMER: u32 = 0x01eef0ec;
pub const TICK_IRQ: usize = 3;
pub const OSC_TICKS_PER_INSTRUCTION: u32 = 1;

// Core TTMR: WL82 csfr.h/hwi.h; stock code acknowledges with bit 6 and
// enables with bit 0. Functional 360 MHz core / 24 MHz oscillator handoff.
#[derive(Default)]
pub struct TickTimer {
    control: u8,
    counter: u32,
    period: u32,
    pub pending: bool,
}
impl TickTimer {
    fn read(&self, offset: u32) -> Option<u32> {
        match offset {
            0 => Some(self.control as u32 | if self.pending { 128 } else { 0 }),
            4 => Some(self.counter),
            8 => Some(self.period),
            _ => None,
        }
    }
    fn write(&mut self, offset: u32, value: u32) -> Option<Result<(), &'static str>> {
        match offset {
            0 => {
                if value & 64 != 0 {
                    self.pending = false;
                }
                self.control = (value & 63) as u8;
            }
            4 => self.counter = value,
            8 => self.period = value,
            _ => return None,
        }
        Some(Ok(()))
    }
    fn advance(&mut self, ticks: u32) {
        if self.control & 1 == 0 {
            return;
        }
        let period = self.period as u64 + 1;
        let next = self.counter as u64 + ticks as u64 * 15;
        if next >= period {
            self.pending = true;
        }
        self.counter = (next % period) as u32;
    }
}

#[derive(Default)]
pub struct Timer {
    control: u32,
    counter: u32,
    period: u32,
    divider_phase: u64,
    pub pending: bool,
}

impl Timer {
    fn read(&self, offset: u32) -> Option<u32> {
        match offset {
            0 => Some(self.control | if self.pending { 1 << 15 } else { 0 }),
            4 => Some(self.counter),
            8 => Some(self.period),
            _ => None,
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> Option<Result<(), &'static str>> {
        match offset {
            0 => {
                // Only the source and dividers actually used by the FM-1 HAL.
                if value & 3 > 1 || (value & 3 == 1 && (value & 12 != 8 || (value >> 4) & 15 > 1)) {
                    return Some(Err("unsupported timer clock source, mode, or divider"));
                }
                if value & (1 << 14) != 0 {
                    self.pending = false;
                }
                self.control = value & 0x3fff;
                if value & 3 == 0 {
                    self.divider_phase = 0;
                }
            }
            4 => {
                self.counter = value;
                self.divider_phase = 0;
            }
            8 => self.period = value,
            _ => return None,
        }
        Some(Ok(()))
    }

    pub fn advance(&mut self, oscillator_ticks: u32) {
        if self.control & 3 != 1 {
            return;
        }
        let divider = if (self.control >> 4) & 15 == 0 { 1 } else { 4 };
        self.divider_phase += oscillator_ticks as u64;
        let ticks = self.divider_phase / divider;
        self.divider_phase %= divider;
        let period = if self.period == u32::MAX {
            1u64 << 32
        } else {
            self.period.max(1) as u64
        };
        let next = self.counter as u64 + ticks;
        if next >= period {
            self.pending = true;
        }
        self.counter = (next % period) as u32;
    }
}

#[derive(Default)]
pub struct Devices {
    pub adc: crate::adc::Adc,
    pub gpio: Gpio,
    pub timer4: Timer,
    pub timer5: Timer,
    pub tick: TickTimer,
    irq_config: [u32; 32],
}

impl Devices {
    pub fn read(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        let value = if (TIMER4..TIMER4 + 12).contains(&address) {
            self.timer4.read(address - TIMER4)
        } else if (TIMER5..TIMER5 + 12).contains(&address) {
            self.timer5.read(address - TIMER5)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&address) {
            self.tick.read(address - TICK_TIMER)
        } else if (IRQ_CONFIG..IRQ_CONFIG + 128).contains(&address) {
            Some(self.irq_config[((address - IRQ_CONFIG) / 4) as usize])
        } else if (IRQ_PENDING..IRQ_PENDING + 16).contains(&address) {
            Some(if address == IRQ_PENDING && self.tick.pending {
                1 << TICK_IRQ
            } else if address == IRQ_PENDING + 4 && self.timer5.pending {
                1 << 31
            } else {
                0
            })
        } else {
            self.adc.read(address).or_else(|| self.gpio.read(address))
        }?;
        Some(if size == 4 || (address == TICK_TIMER && size == 1) {
            Ok(value)
        } else {
            Err("device registers require word accesses")
        })
    }

    pub fn write(
        &mut self,
        address: u32,
        value: u32,
        size: usize,
    ) -> Option<Result<(), &'static str>> {
        if size != 4
            && !(address == TICK_TIMER && size == 1)
            && self.read(address & !3, 4).is_some()
        {
            return Some(Err("device registers require word accesses"));
        }
        if (TIMER4..TIMER4 + 12).contains(&address) {
            self.timer4.write(address - TIMER4, value)
        } else if (TIMER5..TIMER5 + 12).contains(&address) {
            self.timer5.write(address - TIMER5, value)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&address) {
            self.tick.write(address - TICK_TIMER, value)
        } else if (IRQ_CONFIG..IRQ_CONFIG + 128).contains(&address) {
            self.irq_config[((address - IRQ_CONFIG) / 4) as usize] = value;
            Some(Ok(()))
        } else if (IRQ_PENDING..IRQ_PENDING + 16).contains(&address) {
            Some(Err("interrupt pending registers are read-only"))
        } else {
            self.adc
                .write(address, value)
                .or_else(|| self.gpio.write(address, value))
        }
    }

    pub fn advance(&mut self, ticks: u32) {
        self.adc.advance(ticks);
        self.timer4.advance(ticks);
        self.timer5.advance(ticks);
        self.tick.advance(ticks);
    }

    pub fn pending_irq(&self, icfg: u32) -> Option<usize> {
        [
            (TICK_IRQ, self.tick.pending),
            (TIMER5_IRQ, self.timer5.pending),
        ]
        .into_iter()
        .filter(|(source, pending)| *pending && self.irq_priority(*source, icfg).is_some())
        .max_by_key(|(source, _)| self.irq_priority(*source, icfg).unwrap())
        .map(|(source, _)| source)
    }

    pub fn irq_priority(&self, source: usize, icfg: u32) -> Option<u32> {
        let bits = self.irq_config[source >> 3] >> ((source & 7) * 4);
        (icfg & 0x100 != 0 && bits & 1 != 0).then_some((bits >> 1) & 7)
    }
}
