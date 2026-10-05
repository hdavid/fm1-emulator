// SPDX-License-Identifier: GPL-3.0-only
// STUB: the high-speed USB controller (SDK WL82.h husb0: SIE registers from
// lsfr 0x6000, H0_SIE_CON at 0x6800; the PHY registers at 0x6a00 live in
// clock.rs). The stock app enables it for usb id 1 (0x02006f64, called from
// 0x02035908): b[0x16001] = 0 (POWER), [0x16800] |= 3, then it polls
// [0x16800] bit 4 until set. Inferred: bit 4 is a ready flag that follows
// bits 0-1 (no documentation). Nothing is attached to this port: other
// registers only keep what the guest writes (reset value 0, unmeasured).
use std::collections::BTreeMap;

pub const BASE: u32 = 0x16000;
pub const END: u32 = 0x16a00;
const SIE_CON: u32 = 0x16800;

#[derive(Default)]
pub struct Husb {
    bytes: BTreeMap<u32, u8>,
    pub accesses: u64,
}

impl Husb {
    pub fn contains(address: u32) -> bool {
        (BASE..END).contains(&address)
    }

    pub fn read(&self, address: u32, size: usize) -> u32 {
        let mut value = (0..size as u32).fold(0, |v, i| {
            v | (*self.bytes.get(&(address + i)).unwrap_or(&0) as u32) << (8 * i)
        });
        if address == SIE_CON && size == 4 && value & 3 == 3 {
            value |= 1 << 4;
        }
        value
    }

    pub fn write(&mut self, address: u32, value: u32, size: usize) {
        self.accesses += 1;
        for i in 0..size as u32 {
            self.bytes.insert(address + i, (value >> (8 * i)) as u8);
        }
    }
}
