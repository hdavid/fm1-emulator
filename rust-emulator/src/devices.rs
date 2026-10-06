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
// enables with bit 0. Its clock follows the guest's system-clock selection.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct TickTimer {
    control: u8,
    counter: u32,
    period: u32,
    pub pending: bool,
    clock_phase: u64,
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
    fn advance(&mut self, ticks: u32, hz: u32) {
        if self.control & 1 == 0 {
            return;
        }
        let period = self.period as u64 + 1;
        self.clock_phase += ticks as u64 * hz as u64;
        let cycles = self.clock_phase / 24_000_000;
        self.clock_phase %= 24_000_000;
        let next = self.counter as u64 + cycles;
        if next >= period {
            self.pending = true;
        }
        self.counter = (next % period) as u32;
    }
    /// Oscillator ticks at `hz` until `advance` first sets `pending`: until
    /// then every advance only counts, so a span of fewer ticks advanced at
    /// once equals the same ticks advanced one by one. None: no event (off,
    /// already pending, or no clock).
    fn ticks_to_event(&self, hz: u32) -> Option<u64> {
        if self.control & 1 == 0 || self.pending || hz == 0 {
            return None;
        }
        let period = self.period as u64 + 1;
        let counter = self.counter as u64;
        if counter >= period {
            return Some(1);
        }
        let needed = (period - counter) * 24_000_000;
        Some(
            needed
                .saturating_sub(self.clock_phase)
                .div_ceil(hz as u64)
                .max(1),
        )
    }
}

// WL82.h/hwi.h: low-speed RC measurement, IRQ 44. Stock measures 32 or 64
// RC cycles against the 480 MHz PLL and converts NUM back to hertz. Model
// a nominal 32 kHz RC clock, with elapsed time in 24 MHz oscillator ticks.
#[derive(Default)]
struct RcMeasurement {
    control: u8,
    elapsed: u32,
    number: u32,
    pending: bool,
}

// WL82 RAND R64L/R64H. Deterministic emulator noise that changes with
// device time: a hash of the oscillator ticks elapsed, so reads see the same
// words whether device time advanced tick by tick or in one span. Paired
// reads within one oscillator tick share a single 64-bit value.
#[derive(Default)]
struct Random {
    ticks: u64,
}
impl Random {
    fn advance(&mut self, ticks: u32) {
        self.ticks += ticks as u64;
    }
    /// The 64-bit value `elapsed` ticks after the last advance (SplitMix64).
    fn value(&self, elapsed: u64) -> u64 {
        let mut z = (self.ticks + elapsed)
            .wrapping_add(1)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}
impl RcMeasurement {
    fn read(&self, offset: u32) -> Option<u32> {
        match offset {
            0 => Some(self.control as u32 | if self.pending { 128 } else { 0 }),
            4 => Some(self.number),
            _ => None,
        }
    }
    fn write(&mut self, offset: u32, value: u32) -> Option<Result<(), &'static str>> {
        if offset != 0 {
            return (offset == 4).then_some(Err("RC measurement result is read-only"));
        }
        if value & !0xc3 != 0 {
            return Some(Err("unsupported RC measurement configuration"));
        }
        if value & 64 != 0 {
            self.pending = false;
        }
        if value & 1 == 0 || self.control & 1 == 0 {
            self.elapsed = 0;
        }
        self.control = (value & 3) as u8;
        Some(Ok(()))
    }
    fn ticks_to_event(&self) -> Option<u64> {
        if self.control & 1 == 0 || self.pending {
            return None;
        }
        let period = (32u64 << ((self.control >> 1) & 1)) * (24_000_000 / 32_000);
        Some(period.saturating_sub(self.elapsed as u64).max(1))
    }
    fn advance(&mut self, ticks: u32) {
        if self.control & 1 == 0 || self.pending {
            return;
        }
        let cycles = 32 << ((self.control >> 1) & 1);
        let period = cycles * (24_000_000 / 32_000);
        self.elapsed = self.elapsed.saturating_add(ticks);
        if self.elapsed >= period {
            self.number = cycles * (480_000_000 / 32_000);
            self.elapsed = 0;
            self.pending = true;
        }
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Timer {
    control: u32,
    counter: u32,
    period: u32,
    divider_phase: u64,
    pub pending: bool,
}

impl Timer {
    pub(crate) fn read(&self, offset: u32) -> Option<u32> {
        match offset {
            0 => Some(self.control | if self.pending { 1 << 15 } else { 0 }),
            4 => Some(self.counter),
            8 => Some(self.period),
            _ => None,
        }
    }

    pub(crate) fn write(&mut self, offset: u32, value: u32) -> Option<Result<(), &'static str>> {
        match offset {
            0 => {
                // Count mode, with LSB (0) or OSC (2) source. Edge capture
                // and external clock inputs remain unsupported.
                if value & 3 > 1 || (value & 3 == 1 && !matches!(value & 12, 0 | 8)) {
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
        self.advance_with_clock(oscillator_ticks, 60_000_000);
    }

    /// Oscillator ticks until `advance_with_clock` first sets `pending`
    /// (see `TickTimer::ticks_to_event`).
    fn ticks_to_event(&self, peripheral_hz: u32) -> Option<u64> {
        if self.control & 3 != 1 || self.pending {
            return None;
        }
        let prescaler = [
            1u64, 4, 16, 64, 2, 8, 32, 128, 256, 1024, 4096, 16384, 512, 2048, 8192, 32768,
        ][((self.control >> 4) & 15) as usize];
        let hz = if self.control & 12 == 8 {
            24_000_000
        } else {
            peripheral_hz
        };
        if hz == 0 {
            return None;
        }
        let period = if self.period == u32::MAX {
            1u64 << 32
        } else {
            self.period.max(1) as u64
        };
        let counter = self.counter as u64;
        if counter >= period {
            return Some(1);
        }
        let needed = (period - counter) * 24_000_000 * prescaler;
        Some(
            needed
                .saturating_sub(self.divider_phase)
                .div_ceil(hz as u64)
                .max(1),
        )
    }

    pub(crate) fn advance_with_clock(&mut self, oscillator_ticks: u32, peripheral_hz: u32) {
        if self.control & 3 != 1 {
            return;
        }
        // Vendor timer.c's non-monotonic 16-entry prescaler encoding.
        let prescaler = [
            1, 4, 16, 64, 2, 8, 32, 128, 256, 1024, 4096, 16384, 512, 2048, 8192, 32768,
        ][((self.control >> 4) & 15) as usize];
        let hz = if self.control & 12 == 8 {
            24_000_000
        } else {
            peripheral_hz
        };
        let divider = 24_000_000 * prescaler as u64;
        self.divider_phase += oscillator_ticks as u64 * hz as u64;
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
    startup_timers: [Timer; 4],
    tick_secondary: TickTimer,
    rc_measurement: RcMeasurement,
    random: Random,
    // WL82.h JL_IOMAP CON2/CON3, used by fm1_uart.h for UART1 RX routing.
    uart_iomap: [u32; 2],
    uart: crate::uart::Uart,
    irq_config: [[u32; 32]; 2],
    priority_mask: [u32; 2],
    software: u8,
}

impl Devices {
    /// TIMER4 or TIMER5, by the address of its CON register.
    pub(crate) fn timer(&self, base: u32) -> Option<&Timer> {
        match base {
            TIMER4 => Some(&self.timer4),
            TIMER5 => Some(&self.timer5),
            _ => None,
        }
    }
    /// TIMER4 or TIMER5, by the address of its CON register.
    pub(crate) fn timer_mut(&mut self, base: u32) -> Option<&mut Timer> {
        match base {
            TIMER4 => Some(&mut self.timer4),
            TIMER5 => Some(&mut self.timer5),
            _ => None,
        }
    }
    fn bank(address: u32) -> (usize, u32) {
        if (0x1eef200..0x1eef400).contains(&address) {
            (1, address - 0x200)
        } else {
            (0, address)
        }
    }
    pub fn read(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        if let Some(result) = self.uart.read(address, size) {
            return Some(result);
        }
        let (core, a) = Self::bank(address);
        let tick = if core == 0 {
            &self.tick
        } else {
            &self.tick_secondary
        };
        let value = if matches!(a, 0x51024 | 0x51028) {
            Some(self.uart_iomap[((a - 0x51024) / 4) as usize])
        } else if (0x10400..0x10800).contains(&a) {
            self.startup_timers[((a - 0x10400) / 256) as usize].read(a & 255)
        } else if (TIMER4..TIMER4 + 12).contains(&a) {
            self.timer4.read(a - TIMER4)
        } else if (TIMER5..TIMER5 + 12).contains(&a) {
            self.timer5.read(a - TIMER5)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&a) {
            tick.read(a - TICK_TIMER)
        } else if (0x13600..0x13608).contains(&a) {
            self.rc_measurement.read(a - 0x13600)
        } else if matches!(a, 0x13b00 | 0x13b04) {
            Some((self.random.value(0) >> if a & 4 == 0 { 0 } else { 32 }) as u32)
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
                    if self.adc.pending_irq() {
                        bits |= 1 << 24;
                    }
                    if self.uart.pending_irq() {
                        bits |= 1 << 20;
                    }
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
                        | if self.rc_measurement.pending {
                            1 << 12
                        } else {
                            0
                        }
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
        if let Some(result) = self.uart.write(address, value, size, None) {
            return Some(result);
        }
        let (core, a) = Self::bank(address);
        if size != 4 && !(a == TICK_TIMER && size == 1) && self.read(address & !3, 4).is_some() {
            return Some(Err("device registers require word accesses"));
        }
        if matches!(a, 0x51024 | 0x51028) {
            self.uart_iomap[((a - 0x51024) / 4) as usize] = value;
            Some(Ok(()))
        } else if (0x10400..0x10800).contains(&a) {
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
        } else if (0x13600..0x13608).contains(&a) {
            self.rc_measurement.write(a - 0x13600, value)
        } else if matches!(a, 0x13b00 | 0x13b04) {
            Some(Err("random generator registers are read-only"))
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
        self.advance_with_timer_clock(ticks, 60_000_000);
    }
    /// A register read `elapsed` oscillator ticks after the last advance,
    /// with no device event in between: counters are what that many more
    /// ticks at these clocks give.
    pub(crate) fn read_at(
        &self,
        address: u32,
        size: usize,
        elapsed: u64,
        peripheral_hz: u32,
        core_hz: u32,
    ) -> Option<Result<u32, &'static str>> {
        if elapsed == 0 {
            return self.read(address, size);
        }
        let ticks = u32::try_from(elapsed).expect("device spans are bounded");
        let (core, a) = Self::bank(address);
        let value = if (0x10400..0x10800).contains(&a) {
            let mut timer = self.startup_timers[((a - 0x10400) / 256) as usize].clone();
            timer.advance_with_clock(ticks, peripheral_hz);
            timer.read(a & 255)
        } else if (TIMER4..TIMER4 + 12).contains(&a) {
            let mut timer = self.timer4.clone();
            timer.advance_with_clock(ticks, peripheral_hz);
            timer.read(a - TIMER4)
        } else if (TIMER5..TIMER5 + 12).contains(&a) {
            let mut timer = self.timer5.clone();
            timer.advance_with_clock(ticks, peripheral_hz);
            timer.read(a - TIMER5)
        } else if (TICK_TIMER..TICK_TIMER + 12).contains(&a) {
            let mut tick = if core == 0 {
                self.tick.clone()
            } else {
                self.tick_secondary.clone()
            };
            tick.advance(ticks, core_hz);
            tick.read(a - TICK_TIMER)
        } else if matches!(a, 0x13b00 | 0x13b04) {
            Some((self.random.value(elapsed) >> if a & 4 == 0 { 0 } else { 32 }) as u32)
        } else {
            return self.read(address, size);
        }?;
        Some(if size == 4 || (a == TICK_TIMER && size == 1) {
            Ok(value)
        } else {
            Err("device registers require word accesses")
        })
    }
    /// Oscillator ticks until the next tick at which advancing does more
    /// than count (an interrupt source sets, a conversion or transfer ends),
    /// at these clocks; None when nothing is scheduled.
    pub(crate) fn ticks_to_event(
        &self,
        peripheral_hz: u32,
        core_hz: u32,
        uart_hz: u32,
    ) -> Option<u64> {
        [
            self.adc.ticks_to_event(),
            self.timer4.ticks_to_event(peripheral_hz),
            self.timer5.ticks_to_event(peripheral_hz),
            self.tick.ticks_to_event(core_hz),
            self.tick_secondary.ticks_to_event(core_hz),
            self.rc_measurement.ticks_to_event(),
            self.uart.ticks_to_event(uart_hz),
        ]
        .into_iter()
        .chain(
            self.startup_timers
                .iter()
                .map(|timer| timer.ticks_to_event(peripheral_hz)),
        )
        .flatten()
        .min()
    }
    pub(crate) fn write_uart(
        &mut self,
        address: u32,
        value: u32,
        size: usize,
        ram: &[u8],
    ) -> Option<Result<(), &'static str>> {
        self.uart.write(address, value, size, Some(ram))
    }
    pub(crate) fn advance_uart(&mut self, ticks: u32, hz: u32, ram: &mut [u8]) {
        self.uart.advance(ticks, hz);
        self.uart.advance_rx(ticks, ram);
    }
    pub(crate) fn uart(&self) -> &crate::uart::Uart {
        &self.uart
    }
    pub(crate) fn uart_mut(&mut self) -> &mut crate::uart::Uart {
        &mut self.uart
    }
    pub(crate) fn advance_with_timer_clock(&mut self, ticks: u32, peripheral_hz: u32) {
        self.advance_with_clocks(ticks, peripheral_hz, 360_000_000);
    }
    pub(crate) fn advance_with_clocks(&mut self, ticks: u32, peripheral_hz: u32, core_hz: u32) {
        self.adc.advance(ticks);
        for timer in &mut self.startup_timers {
            timer.advance_with_clock(ticks, peripheral_hz);
        }
        self.timer4.advance_with_clock(ticks, peripheral_hz);
        self.timer5.advance_with_clock(ticks, peripheral_hz);
        self.tick.advance(ticks, core_hz);
        self.tick_secondary.advance(ticks, core_hz);
        self.rc_measurement.advance(ticks);
        self.random.advance(ticks);
    }
    fn software_enabled(&self, core: usize) -> u32 {
        let config = self.irq_config[core][15];
        (0..8).fold(0, |mask, bit| mask | (((config >> (bit * 4)) & 1) << bit))
    }
    pub fn pending_irq(&self, icfg: u32) -> Option<usize> {
        self.pending_irq_for(icfg, 0)
    }
    pub(crate) fn pending_irq_for(&self, icfg: u32, core: usize) -> Option<usize> {
        self.select_irq(self.pending_sources(core), icfg, core)
    }
    pub(crate) fn pending_sources(&self, core: usize) -> u128 {
        let tick = if core == 0 {
            &self.tick
        } else {
            &self.tick_secondary
        };
        let mut sources = ((tick.pending as u128) << TICK_IRQ)
            | ((self.adc.pending_irq() as u128) << 24)
            | ((self.uart.pending_irq() as u128) << 20)
            | ((self.rc_measurement.pending as u128) << 44)
            | ((self.timer4.pending as u128) << 62)
            | ((self.timer5.pending as u128) << TIMER5_IRQ)
            | ((self.software as u128) << 120);
        for (index, timer) in self.startup_timers.iter().enumerate() {
            sources |= (timer.pending as u128) << (4 + index);
        }
        sources
    }
    pub(crate) fn select_irq(&self, mut sources: u128, icfg: u32, core: usize) -> Option<usize> {
        if icfg & 0x100 == 0 {
            return None;
        }
        let mut candidate = None;
        let mut highest = 0;
        while sources != 0 {
            let source = sources.trailing_zeros() as usize;
            sources &= sources - 1;
            if let Some(priority) = self.irq_priority_for(source, icfg, core) {
                // Visit sources in ascending order to retain the lowest ID
                // when equally ranked interrupts are pending together.
                if candidate.is_none() || priority > highest {
                    candidate = Some(source);
                    highest = priority;
                }
            }
        }
        candidate
    }
    pub fn irq_priority(&self, source: usize, icfg: u32) -> Option<u32> {
        self.irq_priority_for(source, icfg, 0)
    }
    pub(crate) fn irq_priority_for(&self, source: usize, icfg: u32, core: usize) -> Option<u32> {
        let bits = self.irq_config[core][source >> 3] >> ((source & 7) * 4);
        let priority = (bits >> 1) & 7;
        (icfg & 0x100 != 0 && bits & 1 != 0 && priority >= self.priority_mask[core])
            .then_some(priority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simultaneous_interrupts_respect_ties_masks_and_independent_cores() {
        let mut d = Devices::default();
        d.tick.pending = true;
        d.tick_secondary.pending = true;
        d.timer5.pending = true;
        d.write(IRQ_CONFIG, 7 << 12, 4).unwrap().unwrap(); // Tick: priority 3.
        d.write(IRQ_CONFIG + 7 * 4, 7 << 28, 4).unwrap().unwrap(); // Timer5: priority 3.
        d.write(IRQ_CONFIG + 0x200, 11 << 12, 4).unwrap().unwrap(); // Secondary tick: priority 5.
        d.write(IRQ_CONFIG + 0x200 + 7 * 4, 13 << 28, 4)
            .unwrap()
            .unwrap(); // Secondary timer5: priority 6.
        assert_eq!(d.pending_irq_for(0x100, 0), Some(TICK_IRQ));
        assert_eq!(d.pending_irq_for(0x100, 1), Some(TIMER5_IRQ));
        assert_eq!(d.pending_irq_for(0, 0), None);
        d.write(0x1eef1a8, 4, 4).unwrap().unwrap();
        assert_eq!(d.pending_irq_for(0x100, 0), None);
        d.tick_secondary.pending = false;
        d.write(IRQ_CONFIG + 0x200 + 7 * 4, 12 << 28, 4)
            .unwrap()
            .unwrap(); // Pending timer5 is now disabled.
        assert_eq!(d.pending_irq_for(0x100, 1), None);
        d.write(IRQ_CONFIG + 15 * 4, 15 << 28, 4).unwrap().unwrap(); // Software7 at priority 7.
        d.write(0x1eef1a0, 128, 4).unwrap().unwrap();
        assert_eq!(d.pending_irq_for(0x100, 0), Some(127));
        d.write(0x1eef1a4, 128, 4).unwrap().unwrap();
        assert_eq!(d.pending_irq_for(0x100, 0), None);
    }
    #[test]
    fn peripheral_timer_accumulates_fractional_clock_ticks_across_rate_changes() {
        let mut t = Timer::default();
        t.write(8, 3).unwrap().unwrap();
        t.write(0, 1).unwrap().unwrap();
        t.advance_with_clock(7, 10_000_000);
        assert_eq!(t.read(4), Some(2));
        assert!(!t.pending);
        t.advance_with_clock(1, 2_000_000);
        assert_eq!(t.read(4), Some(0));
        assert!(t.pending);
        t.write(0, 0x4001).unwrap().unwrap();
        assert!(!t.pending);
    }
    #[test]
    fn all_timer_prescalers_count_without_losing_partial_periods() {
        let mut t = Timer::default();
        for (index, div) in [
            1, 4, 16, 64, 2, 8, 32, 128, 256, 1024, 4096, 16384, 512, 2048, 8192, 32768,
        ]
        .into_iter()
        .enumerate()
        {
            t.write(0, 0).unwrap().unwrap();
            t.write(4, 0).unwrap().unwrap();
            t.write(8, 2).unwrap().unwrap();
            t.write(0, 0x4009 | (index as u32) << 4).unwrap().unwrap();
            t.advance_with_clock(2 * div - 1, 60_000_000);
            assert_eq!(t.read(4), Some(1));
            assert!(!t.pending);
            t.advance_with_clock(1, 60_000_000);
            assert_eq!(t.read(4), Some(0));
            assert!(t.pending);
        }
    }

    struct XorShift(u64);
    impl XorShift {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % bound
        }
    }

    /// Ticks before `ticks_to_event` only count: one advance over them
    /// equals single ticks, and the event tick is the first that sets
    /// pending (or a conservative single tick).
    #[test]
    fn timer_spans_match_single_ticks_up_to_the_event() {
        let mut rng = XorShift(0x2545_f491_4f6c_dd1d);
        for _ in 0..4000 {
            let period = match rng.below(8) {
                0 => 0,
                1 => u32::MAX,
                _ => rng.below(3000) as u32,
            };
            let hz = [24_000_000, 60_000_000, 45_000_000, 7_000_000][rng.below(4) as usize];
            let source = if rng.below(2) == 0 { 0 } else { 8 };
            let mut timer = Timer {
                control: 1 | source | (rng.below(3) as u32) << 4,
                counter: rng.below(period as u64 + 40) as u32,
                period,
                ..Timer::default()
            };
            if period == u32::MAX {
                timer.counter = u32::MAX - rng.below(5000) as u32;
            }
            timer.advance_with_clock(rng.below(7) as u32, hz);
            timer.pending = false;
            let Some(events) = timer.ticks_to_event(hz) else {
                panic!("{timer:?} has no event");
            };
            let events = events.min(20_000);
            let mut single = timer.clone();
            for _ in 1..events {
                single.advance_with_clock(1, hz);
                assert!(!single.pending, "{timer:?} wrapped before {events}");
            }
            let mut spanned = timer.clone();
            if events > 1 {
                spanned.advance_with_clock((events - 1) as u32, hz);
            }
            assert_eq!(spanned, single, "{timer:?}");
        }
    }

    #[test]
    fn tick_timer_spans_match_single_ticks_up_to_the_event() {
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        for _ in 0..4000 {
            let period = rng.below(5000) as u32;
            let hz = [360_000_000, 192_000_000, 24_000_000, 5_000_000][rng.below(4) as usize];
            let mut timer = TickTimer {
                control: 1,
                counter: rng.below(period as u64 + 40) as u32,
                period,
                ..TickTimer::default()
            };
            timer.advance(rng.below(3) as u32, hz);
            timer.pending = false;
            let events = timer.ticks_to_event(hz).unwrap();
            let mut single = timer.clone();
            for _ in 1..events {
                single.advance(1, hz);
                assert!(!single.pending, "{timer:?} wrapped before {events}");
            }
            let mut spanned = timer.clone();
            if events > 1 {
                spanned.advance((events - 1) as u32, hz);
            }
            assert_eq!(spanned, single, "{timer:?}");
            single.advance(1, hz);
            assert!(
                single.pending || events == 1,
                "{timer:?} no wrap at {events}"
            );
        }
    }

    #[test]
    fn reads_between_events_see_the_counters_of_single_ticks() {
        let mut lazy = Devices::default();
        lazy.write(TIMER5 + 8, 1000, 4).unwrap().unwrap();
        lazy.write(TIMER5, 1, 4).unwrap().unwrap();
        lazy.write(TICK_TIMER + 8, 100_000, 4).unwrap().unwrap();
        lazy.write(TICK_TIMER, 1, 4).unwrap().unwrap();
        let mut eager = Devices::default();
        eager.write(TIMER5 + 8, 1000, 4).unwrap().unwrap();
        eager.write(TIMER5, 1, 4).unwrap().unwrap();
        eager.write(TICK_TIMER + 8, 100_000, 4).unwrap().unwrap();
        eager.write(TICK_TIMER, 1, 4).unwrap().unwrap();
        for elapsed in 1..300u64 {
            eager.advance_with_clocks(1, 60_000_000, 320_000_000);
            for address in [TIMER5 + 4, TICK_TIMER + 4, 0x13b00, 0x13b04] {
                assert_eq!(
                    lazy.read_at(address, 4, elapsed, 60_000_000, 320_000_000),
                    eager.read(address, 4),
                    "{address:x} after {elapsed}"
                );
            }
        }
    }
}
