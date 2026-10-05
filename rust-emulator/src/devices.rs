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
#[derive(Default, Clone, PartialEq, Debug)]
pub struct TickTimer {
    control: u8,
    counter: u32,
    period: u32,
    pub pending: bool,
}
impl TickTimer {
    /// Oscillator ticks until the counter next wraps (sets pending), if
    /// running: no tick before that changes anything but the counter.
    fn ticks_to_event(&self) -> Option<u64> {
        if self.control & 1 == 0 {
            return None;
        }
        let period = self.period as u64 + 1;
        let counter = self.counter as u64;
        Some(if counter >= period {
            1
        } else {
            (period - counter).div_ceil(15)
        })
    }
    /// The counter after `elapsed` ticks that end before its next wrap.
    fn counter_after(&self, elapsed: u64) -> u32 {
        if self.control & 1 == 0 {
            self.counter
        } else {
            (self.counter as u64 + elapsed * 15) as u32
        }
    }
    fn read(&self, offset: u32, elapsed: u64) -> Option<u32> {
        match offset {
            0 => Some(self.control as u32 | if self.pending { 128 } else { 0 }),
            4 => Some(self.counter_after(elapsed)),
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
        // Runs every instruction: divide only when the counter wraps.
        self.counter = if next >= period {
            self.pending = true;
            (next % period) as u32
        } else {
            next as u32
        };
    }
}

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Timer {
    control: u32,
    counter: u32,
    period: u32,
    divider_phase: u64,
    pub pending: bool,
    pub lsb_stub_used: bool,
}

impl Timer {
    fn divider(&self) -> u64 {
        if (self.control >> 4) & 15 == 0 {
            1
        } else {
            4
        }
    }
    fn period_ticks(&self) -> u64 {
        if self.period == u32::MAX {
            1u64 << 32
        } else {
            self.period.max(1) as u64
        }
    }
    /// Oscillator ticks until the counter next wraps (sets pending), if
    /// running: no tick before that changes anything but the counter and
    /// the divider phase.
    fn ticks_to_event(&self) -> Option<u64> {
        if self.control & 3 != 1 {
            return None;
        }
        let period = self.period_ticks();
        let counter = self.counter as u64;
        if counter >= period {
            return Some(1);
        }
        // The first k with (divider_phase + k) / divider >= period - counter.
        let needed = (period - counter) * self.divider();
        Some(needed.saturating_sub(self.divider_phase).max(1))
    }
    /// The counter after `elapsed` ticks that end before its next wrap.
    fn counter_after(&self, elapsed: u64) -> u32 {
        if self.control & 3 != 1 {
            self.counter
        } else {
            (self.counter as u64 + (self.divider_phase + elapsed) / self.divider()) as u32
        }
    }
    fn read(&self, offset: u32, elapsed: u64) -> Option<u32> {
        match offset {
            0 => Some(self.control | if self.pending { 1 << 15 } else { 0 }),
            4 => Some(self.counter_after(elapsed)),
            8 => Some(self.period),
            _ => None,
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> Option<Result<(), &'static str>> {
        match offset {
            0 => {
                // Only the sources and dividers actually used: source 2 (OSC,
                // FM-1 HAL) and source 0 (lsb_clk, SDK clk_get("timer"); the
                // stock app writes CON = 1 at 0x0200573a). STUB: lsb_clk is
                // not modelled, so source 0 counts at the oscillator rate and
                // sets lsb_stub_used; its real rate is unverified.
                if value & 3 > 1
                    || (value & 3 == 1 && (!matches!(value & 12, 0 | 8) || (value >> 4) & 15 > 1))
                {
                    return Some(Err("unsupported timer clock source, mode, or divider"));
                }
                if value & (1 << 14) != 0 {
                    self.pending = false;
                }
                self.control = value & 0x3fff;
                if value & 15 == 1 {
                    self.lsb_stub_used = true;
                }
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
        // Runs every instruction: divide only when the counter wraps.
        self.counter = if next >= period {
            self.pending = true;
            (next % period) as u32
        } else {
            next as u32
        };
    }
}

#[derive(Default)]
pub struct Devices {
    pub adc: crate::adc::Adc,
    pub gpio: Gpio,
    pub timer4: Timer,
    pub timer5: Timer,
    pub tick: TickTimer,
    startup_timers: [Timer; 4],
    tick_secondary: TickTimer,
    irq_config: [[u32; 32]; 2],
    priority_mask: [u32; 2],
    software: u8,
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
        self.read_at(address, size, 0)
    }
    /// A register read `elapsed` oscillator ticks after the last advance,
    /// with no timer event in between (counters are computed, not stored).
    pub(crate) fn read_at(
        &self,
        address: u32,
        size: usize,
        elapsed: u64,
    ) -> Option<Result<u32, &'static str>> {
        let (core, a) = Self::bank(address);
        let tick = if core == 0 {
            &self.tick
        } else {
            &self.tick_secondary
        };
        let value = if (0x10400..0x10800).contains(&a) {
            self.startup_timers[((a - 0x10400) / 256) as usize].read(a & 255, elapsed)
        } else if (TIMER4..TIMER4 + 12).contains(&a) {
            self.timer4.read(a - TIMER4, elapsed)
        } else if (TIMER5..TIMER5 + 12).contains(&a) {
            self.timer5.read(a - TIMER5, elapsed)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&a) {
            tick.read(a - TICK_TIMER, elapsed)
        } else if (IRQ_CONFIG..IRQ_CONFIG + 128).contains(&a) {
            Some(self.irq_config[core][((a - IRQ_CONFIG) / 4) as usize])
        } else if matches!(a, 0x1eef1a0 | 0x1eef1a4) {
            Some(0)
        } else if a == 0x1eef1a8 {
            Some(self.priority_mask[core])
        } else if (IRQ_PENDING..IRQ_PENDING + 16).contains(&a) {
            Some(match (a - IRQ_PENDING) / 4 {
                0 => {
                    let mut bits = if tick.pending { 1 << TICK_IRQ } else { 0 };
                    for (index, timer) in self.startup_timers.iter().enumerate() {
                        if timer.pending {
                            bits |= 1 << (4 + index);
                        }
                    }
                    bits
                }
                1 => {
                    (if self.timer5.pending { 1 << 31 } else { 0 })
                        | if self.timer4.pending { 1 << 30 } else { 0 }
                }
                3 => (self.software as u32 & self.software_enabled(core)) << 24,
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
        if (0x10400..0x10800).contains(&a) {
            self.startup_timers[((a - 0x10400) / 256) as usize].write(a & 255, value)
        } else if (TIMER4..TIMER4 + 12).contains(&a) {
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
                self.software |= value as u8;
            } else {
                self.software &= !(value as u8);
            }
            Some(Ok(()))
        } else if a == 0x1eef1a8 {
            if value > 7 {
                return Some(Err("invalid CPU interrupt priority mask"));
            }
            self.priority_mask[core] = value;
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
        for timer in &mut self.startup_timers {
            timer.advance(ticks);
        }
        self.timer4.advance(ticks);
        self.timer5.advance(ticks);
        self.tick.advance(ticks);
        self.tick_secondary.advance(ticks);
    }
    /// Oscillator ticks until the next tick at which `advance(1)` does more
    /// than count (a timer wrap or an ADC completion), if any is scheduled.
    /// Until then `advance(n)` equals n calls of `advance(1)`.
    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        [
            self.adc.ticks_to_event(),
            self.timer4.ticks_to_event(),
            self.timer5.ticks_to_event(),
            self.tick.ticks_to_event(),
            self.tick_secondary.ticks_to_event(),
        ]
        .into_iter()
        .chain(self.startup_timers.iter().map(Timer::ticks_to_event))
        .flatten()
        .min()
    }
    fn software_enabled(&self, core: usize) -> u32 {
        let config = self.irq_config[core][15];
        (0..8).fold(0, |mask, bit| mask | (((config >> (bit * 4)) & 1) << bit))
    }
    /// Whether any source here is pending for either core: when not,
    /// `pending_irq_for` finds nothing.
    #[inline(always)]
    pub(crate) fn any_pending(&self) -> bool {
        self.tick.pending
            || self.tick_secondary.pending
            || self.timer4.pending
            || self.timer5.pending
            || self.software != 0
            || self.startup_timers.iter().any(|timer| timer.pending)
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
        // Called before every instruction while interrupts are enabled: skip
        // the source scan when nothing is pending or the controller is off.
        let any_pending = tick.pending
            || self.timer4.pending
            || self.timer5.pending
            || self.software != 0
            || self.startup_timers.iter().any(|timer| timer.pending);
        if !any_pending || icfg & 0x100 == 0 {
            return None;
        }
        // The highest-priority deliverable source; on equal priority the
        // later source in this order wins.
        let mut best: Option<(usize, u32)> = None;
        let mut consider = |source: usize, pending: bool| {
            if !pending {
                return;
            }
            if let Some(priority) = self.irq_priority_for(source, icfg, core) {
                if best.is_none_or(|(_, highest)| priority >= highest) {
                    best = Some((source, priority));
                }
            }
        };
        consider(TICK_IRQ, tick.pending);
        consider(62, self.timer4.pending);
        consider(TIMER5_IRQ, self.timer5.pending);
        for (i, timer) in self.startup_timers.iter().enumerate() {
            consider(4 + i, timer.pending);
        }
        for bit in 0..8 {
            consider(120 + bit, self.software & (1 << bit) != 0);
        }
        best.map(|(source, _)| source)
    }
    pub fn irq_priority(&self, source: usize, icfg: u32) -> Option<u32> {
        self.irq_priority_for(source, icfg, 0)
    }
    pub(crate) fn irq_priority_for(&self, source: usize, icfg: u32, core: usize) -> Option<u32> {
        let bits = self.irq_config[core][source >> 3] >> ((source & 7) * 4);
        (icfg & 0x100 != 0 && bits & 1 != 0).then_some((bits >> 1) & 7)
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;

    struct XorShift(u64);
    impl XorShift {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % bound
        }
    }

    /// Ticks before `ticks_to_event` only count: one batched advance equals
    /// single ticks, the computed counter matches, and the event tick is the
    /// first that sets pending (or a conservative single tick).
    #[test]
    fn timer_batches_match_single_ticks_up_to_the_event() {
        let mut rng = XorShift(0x2545_f491_4f6c_dd1d);
        for _ in 0..4000 {
            let period = match rng.below(8) {
                0 => 0,
                1 => u32::MAX,
                _ => rng.below(3000) as u32,
            };
            let mut timer = Timer {
                control: 1 | if rng.below(2) == 0 { 0 } else { 1 << 4 },
                counter: rng.below(period as u64 + 40) as u32,
                period,
                divider_phase: rng.below(6),
                ..Timer::default()
            };
            if period == u32::MAX {
                timer.counter = u32::MAX - rng.below(5000) as u32;
            }
            let events = timer.ticks_to_event().unwrap();
            let mut single = timer.clone();
            for elapsed in 1..events {
                single.advance(1);
                assert!(!single.pending, "{timer:?} wrapped before {events}");
                assert_eq!(single.counter, timer.counter_after(elapsed), "{timer:?}");
            }
            let mut batched = timer.clone();
            if events > 1 {
                batched.advance((events - 1) as u32);
            }
            assert_eq!(batched, single, "{timer:?}");
            single.advance(1);
            assert!(single.pending || events == 1, "{timer:?} no wrap at {events}");
        }
    }

    #[test]
    fn tick_timer_batches_match_single_ticks_up_to_the_event() {
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        for _ in 0..4000 {
            let period = rng.below(5000) as u32;
            let timer = TickTimer {
                control: 1,
                counter: rng.below(period as u64 + 40) as u32,
                period,
                pending: false,
            };
            let events = timer.ticks_to_event().unwrap();
            let mut single = timer.clone();
            for elapsed in 1..events {
                single.advance(1);
                assert!(!single.pending, "{timer:?} wrapped before {events}");
                assert_eq!(single.counter, timer.counter_after(elapsed), "{timer:?}");
            }
            let mut batched = timer.clone();
            if events > 1 {
                batched.advance((events - 1) as u32);
            }
            assert_eq!(batched, single, "{timer:?}");
            single.advance(1);
            assert!(single.pending || events == 1, "{timer:?} no wrap at {events}");
        }
    }
}
