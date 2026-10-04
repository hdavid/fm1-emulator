// SPDX-License-Identifier: GPL-3.0-only
// JL_CRC REG/FIFO, vendor WL82.h. CRC16 poly 0x1021; FIFO consumes its low byte.
#[derive(Default)]
pub(crate) struct Crc {
    value: u16,
}
impl Crc {
    pub fn read(&self, a: u32) -> Option<u32> {
        (a == 0x13504).then_some(self.value as u32)
    }
    pub fn write(&mut self, a: u32, value: u32) -> Option<()> {
        match a {
            0x13504 => self.value = value as u16,
            0x13500 => {
                self.value ^= (value as u16 & 255) << 8;
                for _ in 0..8 {
                    self.value =
                        (self.value << 1) ^ if self.value & 0x8000 != 0 { 0x1021 } else { 0 };
                }
            }
            _ => return None,
        }
        Some(())
    }
}
