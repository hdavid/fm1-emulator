// SPDX-License-Identifier: GPL-3.0-only
// JL_SRC hardware sample-rate converter (WL82.h: lsfr 0x4300, registers
// CON0..CON3, IDAT_ADR/LEN, ODAT_ADR/LEN, FLTB_ADR). Stock audio init
// (0x0203abb2) disables it with CON0 = 0. Only the register file is modelled:
// a CON0 write that enables the block faults until conversions are emulated.
pub const BASE: u32 = 0x14300;
const REGISTERS: usize = 9;

#[derive(Default)]
pub struct Resampler {
    registers: [u32; REGISTERS],
}

impl Resampler {
    fn index(address: u32) -> Option<usize> {
        let offset = address.checked_sub(BASE)?;
        (offset.is_multiple_of(4) && (offset / 4) < REGISTERS as u32)
            .then_some((offset / 4) as usize)
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        Self::index(address).map(|index| self.registers[index])
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<Result<(), &'static str>> {
        let index = Self::index(address)?;
        if index == 0 && value != 0 {
            return Some(Err("JL_SRC conversions are not emulated (CON0 enable)"));
        }
        self.registers[index] = value;
        Some(Ok(()))
    }
}
