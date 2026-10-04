// SPDX-License-Identifier: GPL-3.0-only
// SPI0 serial NOR. Application ELF supplies decrypted XIP separately.
use std::collections::BTreeMap;
pub struct Nor {
    regs: BTreeMap<u32, u32>,
    command: Vec<u8>,
    selected: bool,
    pub bytes: Vec<u8>,
    cursor: usize,
}

#[cfg(test)]
mod tests {
    use super::Nor;

    #[test]
    fn spi_and_plain_xip_read_the_same_physical_bytes() {
        let mut nor = Nor::default();
        nor.bytes[0xa0000..0xa0004].copy_from_slice(&[0x46, 0x53, 0x4d, 0x50]);
        nor.write(0x4030c, 0x0208f000).unwrap().unwrap();
        nor.write(0x40308, 0x07ffffff).unwrap().unwrap();
        nor.write(0x40300, 3).unwrap().unwrap();
        assert_eq!(nor.xip(0x0209c000, 4).unwrap().unwrap(), 0x504d5346);
        nor.chip_select(true);
        for byte in [0x03, 0x0a, 0x00, 0x00] {
            nor.write(0x11c08, byte).unwrap().unwrap();
        }
        for expected in [0x46, 0x53, 0x4d, 0x50] {
            nor.write(0x11c08, 0xff).unwrap().unwrap();
            assert_eq!(nor.read(0x11c08), Some(expected));
        }
    }
}
impl Default for Nor {
    fn default() -> Self {
        Self {
            regs: BTreeMap::from([(0x40200, 1), (0x5101c, 32), (0x40300, 1)]),
            command: vec![],
            selected: false,
            bytes: vec![255; 1024 * 1024],
            cursor: 0,
        }
    }
}
impl Nor {
    pub fn xip_active(&self) -> bool {
        self.read(0x40200).unwrap() & 1 != 0 && self.read(0x5101c).unwrap() & 32 != 0
    }

    pub fn xip(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        // The SFC maps flash offset 0x4000 at CPU address 0x02000000.
        let offset = address.checked_sub(0x0200_0000)? as usize + 0x4000;
        let bytes = self.bytes.get(offset..offset.checked_add(size)?)?;
        if !self.xip_active() {
            return Some(Err(
                "XIP unavailable while SFC or flash pin routing is disabled",
            ));
        }
        let control = self.read(0x40300).unwrap();
        let plain = control & 1 == 0
            || (control & 2 != 0
                && address >= self.read(0x4030c).unwrap()
                && address.checked_add(size as u32 - 1)? <= self.read(0x40308).unwrap());
        if !plain {
            return Some(Err(
                "encrypted XIP outside the supplied application is not available",
            ));
        }
        Some(Ok(bytes.iter().enumerate().fold(0, |value, (i, byte)| {
            value | ((*byte as u32) << (i * 8))
        })))
    }

    pub fn read(&self, a: u32) -> Option<u32> {
        if a == 0x1eee008 {
            return Some(0x4000);
        }
        matches!(
            a,
            0x40200 | 0x40300 | 0x40308 | 0x4030c | 0x5101c | 0x11c00 | 0x11c04 | 0x11c08
        )
        .then(|| *self.regs.get(&a).unwrap_or(&0))
    }
    pub fn chip_select(&mut self, selected: bool) {
        if self.selected != selected {
            self.command.clear();
            self.cursor = 0;
        }
        self.selected = selected;
    }
    pub fn write(&mut self, a: u32, v: u32) -> Option<Result<(), &'static str>> {
        self.read(a)?;
        if a == 0x1eee008 {
            return Some(Err("cache status is read-only"));
        }
        let mut value = v;
        if a == 0x11c00 {
            value = v & !0xc000;
            if v & 0x4000 == 0 {
                value |= self.read(a).unwrap() & 0x8000;
            }
        }
        if a == 0x11c08 {
            if !self.selected {
                return Some(Err("SPI0 transfer with flash deselected"));
            }
            self.command.push(v as u8);
            let len = self.command.len();
            value = 255;
            match self.command[0] {
                0x9f => {
                    if len > 1 {
                        value = [0x85, 0x60, 0x14].get(len - 2).copied().unwrap_or(255);
                    }
                }
                0x05 | 0x35 => value = 0,
                0x03 | 0x0b => {
                    let start = if self.command[0] == 3 { 4 } else { 5 };
                    if len == 4 {
                        self.cursor = ((self.command[1] as usize) << 16)
                            | ((self.command[2] as usize) << 8)
                            | self.command[3] as usize;
                    }
                    if len > start {
                        value = self.bytes[self.cursor % self.bytes.len()] as u32;
                        self.cursor += 1;
                    }
                }
                _ => return Some(Err("unimplemented SPI NOR command")),
            }
            self.regs
                .insert(0x11c00, self.read(0x11c00).unwrap() | 0x8000);
        }
        self.regs.insert(a, value);
        Some(Ok(()))
    }
}
