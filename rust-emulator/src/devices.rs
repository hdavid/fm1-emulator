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
    tick_secondary: TickTimer,
    irq_config: [[u32; 32]; 2],
    software: [u8; 2],
}

impl Devices {
    fn bank(address: u32) -> (usize, u32) {
        if (0x1eef200..0x1eef400).contains(&address) {
            (1, address - 0x200)
        } else {
            (0, address)
        }
    }
    pub fn read(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        let (core, a) = Self::bank(address);
        let tick = if core == 0 {
            &self.tick
        } else {
            &self.tick_secondary
        };
        let value = if (TIMER4..TIMER4 + 12).contains(&a) {
            self.timer4.read(a - TIMER4)
        } else if (TIMER5..TIMER5 + 12).contains(&a) {
            self.timer5.read(a - TIMER5)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&a) {
            tick.read(a - TICK_TIMER)
        } else if (IRQ_CONFIG..IRQ_CONFIG + 128).contains(&a) {
            Some(self.irq_config[core][((a - IRQ_CONFIG) / 4) as usize])
        } else if matches!(a, 0x1eef1a0 | 0x1eef1a4) {
            Some(0)
        } else if (IRQ_PENDING..IRQ_PENDING + 16).contains(&a) {
            Some(match (a - IRQ_PENDING) / 4 {
                0 => {
                    if tick.pending {
                        1 << TICK_IRQ
                    } else {
                        0
                    }
                }
                1 => {
                    if self.timer5.pending {
                        1 << 31
                    } else {
                        0
                    }
                }
                3 => (self.software[core] as u32) << 24,
                _ => 0,
            })
        } else {
            self.adc.read(address).or_else(|| self.gpio.read(address))
        }?;
        Some(if size == 4 || (a == TICK_TIMER && size == 1) {
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
        let (core, a) = Self::bank(address);
        if size != 4 && !(a == TICK_TIMER && size == 1) && self.read(address & !3, 4).is_some() {
            return Some(Err("device registers require word accesses"));
        }
        if (TIMER4..TIMER4 + 12).contains(&a) {
            self.timer4.write(a - TIMER4, value)
        } else if (TIMER5..TIMER5 + 12).contains(&a) {
            self.timer5.write(a - TIMER5, value)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&a) {
            let tick = if core == 0 {
                &mut self.tick
            } else {
                &mut self.tick_secondary
            };
            tick.write(a - TICK_TIMER, value)
        } else if (IRQ_CONFIG..IRQ_CONFIG + 128).contains(&a) {
            self.irq_config[core][((a - IRQ_CONFIG) / 4) as usize] = value;
            Some(Ok(()))
        } else if matches!(a, 0x1eef1a0 | 0x1eef1a4) {
            if value & !255 != 0 {
                return Some(Err("invalid software interrupt mask"));
            }
            if a & 4 == 0 {
                self.software[core] |= value as u8;
            } else {
                self.software[core] &= !(value as u8);
            }
            Some(Ok(()))
        } else if (IRQ_PENDING..IRQ_PENDING + 16).contains(&a) {
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
        self.tick_secondary.advance(ticks);
    }
    pub fn pending_irq(&self, icfg: u32) -> Option<usize> {
        self.pending_irq_for(icfg, 0)
    }
    pub(crate) fn pending_irq_for(&self, icfg: u32, core: usize) -> Option<usize> {
        let tick = if core == 0 {
            &self.tick
        } else {
            &self.tick_secondary
        };
        [(TICK_IRQ, tick.pending), (TIMER5_IRQ, self.timer5.pending)]
            .into_iter()
            .chain((0..8).map(|bit| (120 + bit, self.software[core] & (1 << bit) != 0)))
            .filter(|(source, pending)| {
                *pending && self.irq_priority_for(*source, icfg, core).is_some()
            })
            .max_by_key(|(source, _)| self.irq_priority_for(*source, icfg, core).unwrap())
            .map(|(source, _)| source)
    }
    pub fn irq_priority(&self, source: usize, icfg: u32) -> Option<u32> {
        self.irq_priority_for(source, icfg, 0)
    }
    pub(crate) fn irq_priority_for(&self, source: usize, icfg: u32, core: usize) -> Option<u32> {
        let bits = self.irq_config[core][source >> 3] >> ((source & 7) * 4);
        (icfg & 0x100 != 0 && bits & 1 != 0).then_some((bits >> 1) & 7)
    }
}
