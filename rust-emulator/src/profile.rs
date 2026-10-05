// SPDX-License-Identifier: GPL-3.0-only
// Instruction profile of the primary core: executions per PC, and reports
// per function (ELF STT_FUNC symbols with their sizes) and per hot address
// range inside a function (a loop body: consecutive PCs that all ran).
// The emulator retires one instruction per step, so counts are instructions,
// not cycles (no flash wait states or cache misses).
use crate::firmware::Symbol;
use std::collections::HashMap;

/// Executions per PC. PCs in the XIP window use a dense table (fast enough
/// to run on every instruction); others (RAM code, ROM) a map.
pub struct Profile {
    base: u32,
    dense: Vec<u64>,
    other: HashMap<u32, u64>,
    pub total: u64,
    /// Instructions run inside an interrupt handler (the audio ISR renders
    /// the mix there, so this is the audio load).
    pub interrupt: u64,
}

/// Executed instructions attributed to one function (or to `name` =
/// "?0x..." for a PC no function covers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionCount {
    pub name: String,
    pub address: u32,
    pub count: u64,
}

/// A run of executed PCs with no gap wider than `Profile::RANGE_GAP` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotRange {
    pub start: u32,
    /// Last executed PC of the range (inclusive).
    pub end: u32,
    pub count: u64,
    /// Highest count of a single PC: about the iterations of a loop body.
    pub peak: u64,
}

impl Profile {
    /// Two executed PCs farther apart than this start a new range.
    pub const RANGE_GAP: u32 = 8;

    pub fn new() -> Self {
        Self::with_window(crate::XIP & !0xfff, crate::XIP_END)
    }

    pub fn with_window(base: u32, end: u32) -> Self {
        Self {
            base,
            dense: vec![0; ((end - base) / 2) as usize],
            other: HashMap::new(),
            total: 0,
            interrupt: 0,
        }
    }

    #[inline(always)]
    pub fn record(&mut self, pc: u32, interrupt: bool) {
        self.total += 1;
        self.interrupt += interrupt as u64;
        let index = (pc.wrapping_sub(self.base) / 2) as usize;
        match self.dense.get_mut(index) {
            Some(count) if pc & 1 == 0 => *count += 1,
            _ => *self.other.entry(pc).or_default() += 1,
        }
    }

    pub fn clear(&mut self) {
        self.dense.iter_mut().for_each(|count| *count = 0);
        self.other.clear();
        self.total = 0;
        self.interrupt = 0;
    }

    /// Every executed PC with its count, by address.
    pub fn pcs(&self) -> Vec<(u32, u64)> {
        let mut out: Vec<(u32, u64)> = self
            .dense
            .iter()
            .enumerate()
            .filter(|(_, &count)| count > 0)
            .map(|(index, &count)| (self.base + index as u32 * 2, count))
            .chain(self.other.iter().map(|(&pc, &count)| (pc, count)))
            .collect();
        out.sort_unstable();
        out
    }

    /// Counts per function, highest first. A PC is attributed to the
    /// function whose [address, address + size) contains it.
    pub fn by_function(&self, symbols: &[Symbol]) -> Vec<FunctionCount> {
        let functions = sorted_functions(symbols);
        let mut totals: HashMap<(u32, String), u64> = HashMap::new();
        for (pc, count) in self.pcs() {
            let key = match function_at(&functions, pc) {
                Some(f) => (f.address, f.name.clone()),
                None => (pc, format!("?0x{pc:08x}")),
            };
            *totals.entry(key).or_default() += count;
        }
        let mut out: Vec<FunctionCount> = totals
            .into_iter()
            .map(|((address, name), count)| FunctionCount {
                name,
                address,
                count,
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count).then(a.address.cmp(&b.address)));
        out
    }

    /// Hot ranges inside [start, end), highest count first.
    pub fn ranges(&self, start: u32, end: u32) -> Vec<HotRange> {
        let mut out: Vec<HotRange> = vec![];
        for (pc, count) in self
            .pcs()
            .into_iter()
            .filter(|&(pc, _)| pc >= start && pc < end)
        {
            match out.last_mut() {
                Some(range) if pc - range.end <= Self::RANGE_GAP => {
                    range.end = pc;
                    range.count += count;
                    range.peak = range.peak.max(count);
                }
                _ => out.push(HotRange {
                    start: pc,
                    end: pc,
                    count,
                    peak: count,
                }),
            }
        }
        out.sort_by(|a, b| b.count.cmp(&a.count).then(a.start.cmp(&b.start)));
        out
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::new()
    }
}

fn sorted_functions(symbols: &[Symbol]) -> Vec<&Symbol> {
    let mut functions: Vec<&Symbol> = symbols
        .iter()
        .filter(|s| s.is_function() && s.size > 0)
        .collect();
    functions.sort_by_key(|s| s.address);
    functions
}

fn function_at<'a>(functions: &[&'a Symbol], pc: u32) -> Option<&'a Symbol> {
    let index = functions.partition_point(|s| s.address <= pc);
    let f = *functions.get(index.checked_sub(1)?)?;
    (pc < f.address + f.size).then_some(f)
}

/// The function containing `pc`, if any.
pub fn function_of(symbols: &[Symbol], pc: u32) -> Option<&Symbol> {
    function_at(&sorted_functions(symbols), pc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(name: &str, address: u32, size: u32) -> Symbol {
        Symbol {
            name: name.into(),
            address,
            size,
            kind: 2,
        }
    }

    #[test]
    fn counts_attribute_to_functions_and_loops_form_ranges() {
        let symbols = [
            function("render", 0x0200_1000, 0x40),
            function("idle", 0x0200_2000, 8),
        ];
        let mut p = Profile::new();
        // a 3-instruction loop body run 100 times, a prologue once, idle 50 times
        p.record(0x0200_1000, false);
        for _ in 0..100 {
            for pc in [0x0200_1010, 0x0200_1012, 0x0200_1016] {
                p.record(pc, true);
            }
        }
        for _ in 0..50 {
            p.record(0x0200_2004, false);
        }
        p.record(0x01c0_0100, false); // RAM code, no symbol
        assert_eq!((p.total, p.interrupt), (352, 300));
        let f = p.by_function(&symbols);
        assert_eq!((f[0].name.as_str(), f[0].count), ("render", 301));
        assert_eq!((f[1].name.as_str(), f[1].count), ("idle", 50));
        assert_eq!((f[2].name.as_str(), f[2].count), ("?0x01c00100", 1));
        let r = p.ranges(0x0200_1000, 0x0200_1040);
        assert_eq!(
            r[0],
            HotRange {
                start: 0x0200_1010,
                end: 0x0200_1016,
                count: 300,
                peak: 100
            }
        );
        assert_eq!(r[1].count, 1);
        assert_eq!(function_of(&symbols, 0x0200_2008), None);
        p.clear();
        assert!(p.pcs().is_empty());
    }
}
