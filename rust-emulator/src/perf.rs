// SPDX-License-Identifier: GPL-3.0-only
// corex2 performance counters (AC79 SDK csfr.h JL_TypeDef_corex2 words
// 0x80-0x8f at 0x1eee200: per core IF/RD/WR_UACNT and TL_CKCNT, each 64-bit
// as L/H words; core 1's set 0x20 above core 0's). TL_CKCNT counts CPU clock
// cycles while the core's DBG_CON enables (bits 0-2 for core 0, 8-10 for
// core 1, "if/of/ex_inv_en" in the SDK's cpu_effic_init) are set: X0X
// measured on a real FM-1 that it does not count with DBG_CON 0. Whether
// one enable bit is enough, or which, is unmeasured: any of the three runs
// it here. The three stall counters (instruction fetch, data read, data
// write) only hold what is written: the emulator has no cache or bus stalls.
pub(crate) const BASE: u32 = 0x01ee_e200;
pub(crate) const DBG_CON: u32 = 0x01ee_e344;

#[derive(Default)]
pub(crate) struct Perf {
    /// Per core: IF L/H, RD L/H, WR L/H.
    stalls: [[u32; 6]; 2],
    /// Per core: the cycle count at `mark` (oscillator ticks).
    cycles: [u64; 2],
    mark: [u64; 2],
    running: [bool; 2],
}

impl Perf {
    /// (core, word 0..7) of a counter register.
    fn index(a: u32) -> Option<(usize, usize)> {
        let offset = a.checked_sub(BASE)?;
        (offset < 0x40 && offset.is_multiple_of(4))
            .then(|| ((offset / 0x20) as usize, ((offset % 0x20) / 4) as usize))
    }

    /// Cycles of `core` at oscillator tick `now` with `per_tick` CPU cycles
    /// per tick.
    fn value(&self, core: usize, now: u64, per_tick: u32) -> u64 {
        let elapsed = if self.running[core] {
            now.saturating_sub(self.mark[core]) * per_tick as u64
        } else {
            0
        };
        self.cycles[core].wrapping_add(elapsed)
    }

    fn settle(&mut self, now: u64, per_tick: u32) {
        for core in 0..2 {
            self.cycles[core] = self.value(core, now, per_tick);
            self.mark[core] = now;
        }
    }

    pub(crate) fn read(&self, a: u32, now: u64, per_tick: u32) -> Option<u32> {
        let (core, word) = Self::index(a)?;
        Some(match word {
            6 => self.value(core, now, per_tick) as u32,
            7 => (self.value(core, now, per_tick) >> 32) as u32,
            _ => self.stalls[core][word],
        })
    }

    pub(crate) fn write(&mut self, a: u32, v: u32, now: u64, per_tick: u32) -> Option<()> {
        let (core, word) = Self::index(a)?;
        self.settle(now, per_tick);
        match word {
            6 => self.cycles[core] = (self.cycles[core] & !0xffff_ffff) | v as u64,
            7 => self.cycles[core] = (self.cycles[core] & 0xffff_ffff) | (v as u64) << 32,
            _ => self.stalls[core][word] = v,
        }
        Some(())
    }

    /// A DBG_CON write: start or stop each core's cycle count.
    pub(crate) fn control(&mut self, dbg_con: u32, now: u64, per_tick: u32) {
        self.settle(now, per_tick);
        self.running = [dbg_con & 7 != 0, (dbg_con >> 8) & 7 != 0];
    }

    /// The CPU clock changes: count the cycles so far at the old rate.
    pub(crate) fn clock_change(&mut self, now: u64, per_tick: u32) {
        self.settle(now, per_tick);
    }
}
