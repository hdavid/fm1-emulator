// SPDX-License-Identifier: GPL-3.0-only
// Per-PC cache of fetched and classified instructions. Only code read from
// plain memory (application XIP within the image, and SRAM) is cached; those
// reads have no side effects, so caching them cannot change behaviour. Any
// change to what such a read would return invalidates the affected entries:
// SRAM writes (`invalidate_ram`) and NOR programming or XIP configuration
// changes (`flush`). Code elsewhere always takes the uncached bus path.
use crate::decode::{decode, decode_wide, primary, Op};
use crate::{RAM, RAM_SIZE};

/// Code halfwords after the instruction word, when the cache read them.
/// `None` means the interpreter reads them through the bus when needed, with
/// exactly the uncached interpreter's timing and faults.
#[derive(Clone, Copy, Default)]
pub(crate) struct Operands {
    pub x: Option<u16>,
    pub y: Option<u16>,
}

/// A cached instruction at `pc`: its word, classification and operands.
#[derive(Clone, Copy)]
pub(crate) struct Entry {
    pc: u32,
    epoch: u32,
    pub h: u16,
    x: u16,
    y: u16,
    pub op: Op,
    known: u8,
}

impl Entry {
    const X: u8 = 1;
    const Y: u8 = 2;
    const EMPTY: Self = Self {
        pc: 1, // Instruction addresses are even, so this never matches.
        epoch: 0,
        h: 0,
        x: 0,
        y: 0,
        op: Op::Unsupported,
        known: 0,
    };

    pub fn operands(&self) -> Operands {
        Operands {
            x: (self.known & Self::X != 0).then_some(self.x),
            y: (self.known & Self::Y != 0).then_some(self.y),
        }
    }
}

const BITS: u32 = 16;
const MASK: usize = (1 << BITS) - 1;
/// SRAM is tracked in 256-byte pages that may hold cached instructions.
const PAGE_SHIFT: u32 = 8;
/// An instruction's cached bytes extend at most 6 bytes from its PC.
const SPAN: u32 = 6;

pub(crate) struct CodeCache {
    entries: Box<[Entry; 1 << BITS]>,
    epoch: u32,
    ram_pages: Vec<bool>,
}

impl Default for CodeCache {
    fn default() -> Self {
        Self {
            entries: vec![Entry::EMPTY; 1 << BITS]
                .into_boxed_slice()
                .try_into()
                .unwrap_or_else(|_| unreachable!()),
            epoch: 1,
            ram_pages: vec![false; RAM_SIZE >> PAGE_SHIFT],
        }
    }
}

fn index(pc: u32) -> usize {
    (pc >> 1) as usize & MASK
}

impl CodeCache {
    #[inline]
    pub fn get(&self, pc: u32) -> Option<Entry> {
        let entry = self.entries[index(pc)];
        (entry.pc == pc && entry.epoch == self.epoch).then_some(entry)
    }

    /// Classify and remember the instruction word `h` at `pc`, read from
    /// cacheable memory, with the following halfwords `x` and `y` where
    /// those are cacheable too.
    pub fn fill(&mut self, pc: u32, h: u16, x: Option<u16>, y: Option<u16>) -> Entry {
        // A parallel bundle's primary slot executes the normalized word.
        let word = primary(h as u32);
        let mut op = decode(word);
        if op == Op::Wide {
            if let Some(x) = x {
                op = decode_wide(word, x as u32);
            }
        }
        let entry = Entry {
            pc,
            epoch: self.epoch,
            h,
            x: x.unwrap_or(0),
            y: y.unwrap_or(0),
            op,
            known: if x.is_some() { Entry::X } else { 0 } | if y.is_some() { Entry::Y } else { 0 },
        };
        self.entries[index(pc)] = entry;
        if let Some(offset) = pc
            .checked_sub(RAM)
            .filter(|&offset| (offset as usize) < RAM_SIZE)
        {
            let first = (offset >> PAGE_SHIFT) as usize;
            let last = ((offset + SPAN - 1) >> PAGE_SHIFT) as usize;
            for page in first..=last.min(self.ram_pages.len() - 1) {
                self.ram_pages[page] = true;
            }
        }
        entry
    }

    /// Forget every entry (NOR contents or XIP configuration changed).
    pub fn flush(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            // Entries from 2^32 flushes ago could match again: clear them.
            self.entries.fill(Entry::EMPTY);
            self.epoch = 1;
        }
    }

    /// SRAM bytes at `offset..offset + length` (relative to RAM) changed.
    #[inline]
    pub fn invalidate_ram(&mut self, offset: usize, length: usize) {
        let first = offset >> PAGE_SHIFT;
        let last = (offset + length.max(1) - 1) >> PAGE_SHIFT;
        for page in first..=last.min(self.ram_pages.len() - 1) {
            if self.ram_pages[page] {
                self.invalidate_page(page);
            }
        }
    }

    #[cold]
    fn invalidate_page(&mut self, page: usize) {
        self.ram_pages[page] = false;
        let start = RAM + (page << PAGE_SHIFT) as u32;
        let end = start + (1 << PAGE_SHIFT);
        // Entries starting up to SPAN bytes before the page reach into it.
        let mut pc = start.saturating_sub(SPAN) & !1;
        while pc < end {
            let slot = &mut self.entries[index(pc)];
            if slot.pc == pc {
                *slot = Entry::EMPTY;
            }
            pc += 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_writes_drop_entries_whose_bytes_they_touch() {
        let mut cache = CodeCache::default();
        let pc = RAM + 0x1fe; // Its operands reach into the next page.
        cache.fill(pc, 0, Some(0), Some(0));
        assert!(cache.get(pc).is_some());
        cache.invalidate_ram(0x300, 4); // Unrelated page.
        assert!(cache.get(pc).is_some());
        cache.invalidate_ram(0x202, 2); // The entry's second operand.
        assert!(cache.get(pc).is_none());
    }

    #[test]
    fn flush_drops_everything_and_unreadable_operands_stay_unknown() {
        let mut cache = CodeCache::default();
        let pc = crate::XIP;
        let entry = cache.fill(pc, 0xe86c, None, None);
        assert_eq!(entry.op, Op::Wide);
        assert!(entry.operands().x.is_none());
        cache.flush();
        assert!(cache.get(pc).is_none());
    }
}
