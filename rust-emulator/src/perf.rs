// SPDX-License-Identifier: GPL-3.0-only
// corex2 performance counters, vendor csfr.h JL_TypeDef_corex2 words
// 0x80-0x8f at 0x1eee200: per core IF/RD/WR_UACNT and TL_CKCNT, each 64-bit
// as L/H words, core 1's set 0x20 above core 0's. TL_CKCNT counts CPU clock
// cycles while the core's DBG_CON enables are set (bits 0-2 for core 0,
// 8-10 for core 1; the SDK's cpu_effic_init sets them before reading). X0X
// measured on a physical FM-1 that it does not count with DBG_CON 0. Which
// of the three enables is enough is unmeasured: any of them runs it here.
// The stall counters only hold what is written: issue is not cycle
// accurate and no cache or bus stalls are modeled.
pub(crate) const DBG_CON: u32 = 0x01ee_e344;
const BASE: u32 = 0x01ee_e200;

#[derive(Default)]
pub(crate) struct Perf {
    stalls: [[u32; 6]; 2],
    /// Each core's count at `mark` (in elapsed CPU cycles).
    count: [u64; 2],
    mark: [u64; 2],
    running: [bool; 2],
}

impl Perf {
    fn index(a: u32) -> Option<(usize, usize)> {
        let offset = a.checked_sub(BASE)?;
        (offset < 0x40 && offset.is_multiple_of(4))
            .then_some(((offset / 0x20) as usize, ((offset % 0x20) / 4) as usize))
    }
    fn value(&self, core: usize, cycles: u64) -> u64 {
        let elapsed = if self.running[core] {
            cycles - self.mark[core]
        } else {
            0
        };
        self.count[core].wrapping_add(elapsed)
    }
    fn settle(&mut self, cycles: u64) {
        for core in 0..2 {
            self.count[core] = self.value(core, cycles);
            self.mark[core] = cycles;
        }
    }
    /// A counter register at `cycles` elapsed CPU cycles.
    pub(crate) fn read(&self, a: u32, cycles: u64) -> Option<u32> {
        let (core, word) = Self::index(a)?;
        Some(match word {
            6 => self.value(core, cycles) as u32,
            7 => (self.value(core, cycles) >> 32) as u32,
            _ => self.stalls[core][word],
        })
    }
    pub(crate) fn write(&mut self, a: u32, v: u32, cycles: u64) -> Option<()> {
        let (core, word) = Self::index(a)?;
        self.settle(cycles);
        match word {
            6 => self.count[core] = (self.count[core] & !0xffff_ffff) | v as u64,
            7 => self.count[core] = (self.count[core] & 0xffff_ffff) | (v as u64) << 32,
            _ => self.stalls[core][word] = v,
        }
        Some(())
    }
    /// A DBG_CON write starts or stops each core's cycle count.
    pub(crate) fn control(&mut self, dbg_con: u32, cycles: u64) {
        self.settle(cycles);
        self.running = [dbg_con & 7 != 0, (dbg_con >> 8) & 7 != 0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycle_count_runs_only_while_enabled_and_is_writable() {
        let mut p = Perf::default();
        assert_eq!(p.read(0x1eee218, 100), Some(0));
        p.control(7, 100);
        assert_eq!(p.read(0x1eee218, 1100), Some(1000));
        p.control(0, 1100);
        assert_eq!(p.read(0x1eee218, 5000), Some(1000));
        p.write(0x1eee218, 0xffff_fff0, 5000).unwrap();
        p.write(0x1eee21c, 2, 5000).unwrap();
        p.control(1 << 8 | 1, 5000);
        assert_eq!(p.read(0x1eee218, 5032), Some(0x10));
        assert_eq!(p.read(0x1eee21c, 5032), Some(3));
        assert_eq!(p.read(0x1eee238, 5032), Some(32), "core 1's own count");
        p.write(0x1eee200, 0x123, 5032).unwrap();
        assert_eq!(p.read(0x1eee200, 9000), Some(0x123));
        assert_eq!(p.read(0x1eee240, 9000), None);
    }
}
