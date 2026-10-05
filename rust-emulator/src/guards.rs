// SPDX-License-Identifier: GPL-3.0-only
use std::collections::BTreeMap;
#[derive(Default)]
pub struct Guards {
    registers: BTreeMap<u32, u32>,
    /// Copies of the protection enable register (0x1eee348) and the three
    /// (end, start) limit pairs, which every guest write consults.
    enabled: u32,
    limits: [(u32, u32); 3],
}
impl Guards {
    fn known(a: u32) -> bool {
        matches!(
            a,
            0x1eee240
                | 0x1eee244
                | 0x1eee248
                | 0x1eee340
                | 0x1eee348
                | 0x1eef0d0
                | 0x1eef0d4
                | 0x1eef0d8
                | 0x1eef0dc
                | 0x1eef0e0
                | 0x1eef0e4
                | 0x1eef1c0
                | 0x1eef3c0
                | 0x1eef1c4
                | 0x1eef1c8
                | 0x1eef1cc
                | 0x1eef1d0
        ) || (0x1eee280..0x1eee28c).contains(&a)
            || (0x1eee2c0..0x1eee2cc).contains(&a)
            || (0x1eee380..0x1eee390).contains(&a)
            || (0x1eee340..=0x1eee358).contains(&a)
            || (0x1eef2d0..=0x1eef2e4).contains(&a)
            || (0x41c00..=0x41c18).contains(&a)
    }
    fn value(&self, a: u32) -> u32 {
        *self.registers.get(&a).unwrap_or(&0)
    }
    pub fn read(&self, a: u32) -> Option<u32> {
        Self::known(a).then(|| self.value(a))
    }
    pub fn write(&mut self, a: u32, v: u32) -> Option<Result<(), &'static str>> {
        if !Self::known(a) {
            return None;
        }
        match a {
            0x1eee240 => {
                if v == 0xe7 {
                    self.registers.insert(a, self.value(a) ^ 1);
                }
            }
            0x1eee248 => {
                self.registers.insert(0x1eee244, self.value(0x1eee244) & !v);
            }
            0x1eef0d4 | 0x1eef2d4 => {
                self.registers.insert(a, self.value(a) & !v);
            }
            _ => {
                self.registers.insert(a, v);
            }
        }
        self.enabled = self.value(0x1eee348);
        self.limits = std::array::from_fn(|n| {
            let n = n as u32;
            (self.value(0x1eee280 + n * 4), self.value(0x1eee2c0 + n * 4))
        });
        Some(Ok(()))
    }
    /// EMU_CON of a core's q32DSP bank (AC79 SDK csfr.h: 0x1eef0d0, core 1
    /// 0x200 higher).
    pub fn emu_con(&self, core: usize) -> u32 {
        self.value(0x1eef0d0 + 0x200 * core as u32)
    }
    /// Latch exception causes in a core's EMU_MSG (write one to clear).
    pub fn raise_emu_msg(&mut self, core: usize, bits: u32) {
        let a = 0x1eef0d4 + 0x200 * core as u32;
        self.registers.insert(a, self.value(a) | bits);
    }
    pub fn check_write(&self, a: u32, size: usize) -> Result<(), &'static str> {
        // Runs on every guest write: consult the cached register copies.
        let enabled = self.enabled;
        if enabled & 7 == 0 {
            return Ok(());
        }
        for (n, &(end, start)) in self.limits.iter().enumerate() {
            if enabled & (1 << n) != 0 && a <= end && a as u64 + size as u64 > start as u64 {
                return Err("CPU write protection violation");
            }
        }
        Ok(())
    }
}
