// SPDX-License-Identifier: GPL-3.0-only
// WL82.h: WL/WF analog configuration latches. Stock wfhw_init also configures
// two words at 0x30f00; their bit meanings are not published in that header.
// RF transmission, reception,
// frequency calibration and serial-data DMA are not modeled.
#[derive(Default)]
pub(crate) struct Wireless {
    registers: [u32; 26],
    radio_configuration: [u32; 2],
}
impl Wireless {
    fn index(address: u32) -> Option<usize> {
        if address & 3 != 0 {
            return None;
        }
        match address {
            0x14000..=0x14034 | 0x14040..=0x14064 => Some(((address - 0x14000) / 4) as usize),
            _ => None,
        }
    }
    pub(crate) fn read(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        if matches!(address & !3, 0x30f00 | 0x30f04) {
            return Some(if size == 4 {
                Ok(self.radio_configuration[((address - 0x30f00) / 4) as usize])
            } else {
                Err("wireless registers require word accesses")
            });
        }
        let index = Self::index(address & !3)?;
        Some(if size == 4 {
            Ok(self.registers[index])
        } else {
            Err("wireless registers require word accesses")
        })
    }
    pub(crate) fn write(
        &mut self,
        address: u32,
        value: u32,
        size: usize,
    ) -> Option<Result<(), &'static str>> {
        if matches!(address & !3, 0x30f00 | 0x30f04) {
            return Some(if size == 4 {
                self.radio_configuration[((address - 0x30f00) / 4) as usize] = value;
                Ok(())
            } else {
                Err("wireless registers require word accesses")
            });
        }
        let index = Self::index(address & !3)?;
        if size != 4 {
            return Some(Err("wireless registers require word accesses"));
        }
        if address == 0x14024 {
            return Some(Err("wireless frequency result is read-only"));
        }
        if address == 0x14020 && value != 0 {
            return Some(Err("wireless frequency calibration is not implemented"));
        }
        self.registers[index] = value;
        Some(Ok(()))
    }
}
