// SPDX-License-Identifier: GPL-3.0-only
// STUB: JL_WL Bluetooth/Wi-Fi baseband and RF registers (WL82.h: lsfr
// 0x4000-0x40ff, WL_CON0.., LOFC, WL_ANL_CON, WF_CON, SD_CON). The stock
// "btctrler" task (created at 0x0205d834) configures them at 0x0205c610.
// No radio is emulated: registers only retain what the guest writes and
// read back zero until written. The same applies to the undocumented
// Bluetooth core window (WL82.h bsfr_base 0x20000-0x3ffff; the controller
// first reads 0x30f04 at 0x0205c6ee). `accesses` makes the stub visible in
// diagnostics; any behaviour that depends on radio status is unverified.
// One inferred behaviour: the RF register port at 0x3101c (writer at
// 0x02083468: wait while bit 17 is set, then write command words with
// bit 17 set) completes at once, so bit 17 reads back clear.
// Second inferred behaviour: WLA_CON30 (0x11978) bit 5 reads as ready. The
// poller at 0x0208340c strobes WLA_CON30 bit 0 eight times, then the caller
// at 0x02083552 loops until bit 5 of the read-back is set.
use std::{cell::Cell, collections::BTreeMap};

pub const BASE: u32 = 0x14000;
const WORDS: usize = 64;
const BT_CORE: std::ops::Range<u32> = 0x20000..0x40000;
// JL_ANA WLA_CON1..WLA_CON39 (WL82.h lsfr 0x1904-0x199c; WLA_CON0 lives in
// adc.rs, PLL_CON at 0x19a0 in clock.rs). First read: WLA_CON12 at 0x0205c7a4.
const ANALOG: std::ops::Range<u32> = 0x11904..0x119a0;
const RF_PORT: u32 = 0x3101c;
const RF_BUSY: u32 = 1 << 17;
const ANALOG_STATUS: u32 = 0x11978;
const ANALOG_READY: u32 = 1 << 5;

pub struct Radio {
    registers: [u32; WORDS],
    core: BTreeMap<u32, u32>,
    pub accesses: Cell<u64>,
}

impl Default for Radio {
    fn default() -> Self {
        Self {
            registers: [0; WORDS],
            core: BTreeMap::new(),
            accesses: Cell::new(0),
        }
    }
}

impl Radio {
    pub fn contains(address: u32) -> bool {
        (BASE..BASE + 4 * WORDS as u32).contains(&address)
            || BT_CORE.contains(&address)
            || ANALOG.contains(&address)
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        Self::contains(address).then(|| {
            self.accesses.set(self.accesses.get() + 1);
            if BT_CORE.contains(&address) || ANALOG.contains(&address) {
                let value = self.core.get(&(address & !3)).copied().unwrap_or(0);
                if address & !3 == ANALOG_STATUS {
                    value | ANALOG_READY
                } else {
                    value
                }
            } else {
                self.registers[((address - BASE) / 4) as usize]
            }
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<()> {
        Self::contains(address).then(|| {
            self.accesses.set(self.accesses.get() + 1);
            if BT_CORE.contains(&address) || ANALOG.contains(&address) {
                let value = if address & !3 == RF_PORT {
                    value & !RF_BUSY
                } else {
                    value
                };
                self.core.insert(address & !3, value);
            } else {
                self.registers[((address - BASE) / 4) as usize] = value;
            }
        })
    }
}
