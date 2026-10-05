// SPDX-License-Identifier: GPL-3.0-only
// JL_RAND hardware random number generator (WL82.h: lsfr 0x3b00, read-only
// R64L/R64H). The stock application reads it at 0x02003472. A fixed-seed
// xorshift stands in for the hardware entropy source so runs repeat exactly.
use std::cell::Cell;

pub const BASE: u32 = 0x13b00;

pub struct Rng {
    state: Cell<u64>,
}

impl Default for Rng {
    fn default() -> Self {
        Self {
            state: Cell::new(0x9e37_79b9_7f4a_7c15),
        }
    }
}

impl Rng {
    fn next(&self) -> u64 {
        let mut x = self.state.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state.set(x);
        x
    }

    /// Each word read returns fresh random bits.
    pub fn read(&self, address: u32) -> Option<u32> {
        match address {
            BASE => Some(self.next() as u32),
            a if a == BASE + 4 => Some((self.next() >> 32) as u32),
            _ => None,
        }
    }

    pub fn contains(address: u32) -> bool {
        address == BASE || address == BASE + 4
    }
}
