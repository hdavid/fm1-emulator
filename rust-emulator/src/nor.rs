// SPDX-License-Identifier: GPL-3.0-only
// SPI0 serial NOR. Application ELF supplies decrypted XIP separately.
use std::collections::BTreeMap;
pub struct Nor {
    regs: BTreeMap<u32, u32>,
    command: Vec<u8>,
    selected: bool,
    pub bytes: Vec<u8>,
    cursor: usize,
    decoded: Option<Vec<u8>>,
}

impl Default for Nor {
    fn default() -> Self {
        Self {
            regs: BTreeMap::from([(0x40200, 1), (0x4020c, 0x4000), (0x5101c, 32), (0x40300, 1)]),
            command: vec![],
            selected: false,
            bytes: vec![255; 1024 * 1024],
            cursor: 0,
            decoded: None,
        }
    }
}

impl Nor {
    pub fn load(&mut self, bytes: &[u8], key: u16) {
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        let mut decoded = self.bytes[0x4000..].to_vec();
        crate::package::sfc(&mut decoded, key);
        self.decoded = Some(decoded);
        self.regs
            .extend([(0x40200, 0x809803b5), (0x40204, 1), (0x40208, 0x8e17)]);
    }
    pub fn packaged(&self) -> bool {
        self.decoded.is_some()
    }
    pub fn xip_active(&self) -> bool {
        self.read(0x40200).unwrap() & 1 != 0 && self.read(0x5101c).unwrap() & 32 != 0
    }

    pub fn xip(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        // The SFC maps flash offset 0x4000 at CPU address 0x02000000.
        let offset = address.checked_sub(0x0200_0000)? as usize + self.read(0x4020c)? as usize;
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
            if let Some(decoded) = &self.decoded {
                if offset < 0x4000 {
                    return Some(Err(
                        "encrypted XIP below application area is not implemented",
                    ));
                }
                let bytes = &decoded[offset - 0x4000..offset - 0x4000 + size];
                return Some(Ok(bytes
                    .iter()
                    .enumerate()
                    .fold(0, |value, (i, byte)| value | ((*byte as u32) << (i * 8)))));
            } else {
                return Some(Err(
                    "encrypted XIP outside the supplied application is not available",
                ));
            }
        }
        Some(Ok(bytes.iter().enumerate().fold(0, |value, (i, byte)| {
            value | ((*byte as u32) << (i * 8))
        })))
    }

    pub fn read(&self, a: u32) -> Option<u32> {
        matches!(
            a,
            0x40200
                | 0x40204
                | 0x40208
                | 0x4020c
                | 0x40300
                | 0x40304
                | 0x40308
                | 0x4030c
                | 0x40310
                | 0x40314
                | 0x5101c
                | 0x11c00
                | 0x11c04
                | 0x11c08
        )
        .then(|| {
            let value = *self.regs.get(&a).unwrap_or(&0);
            // Bit 31 is transaction busy, not retained configuration. The
            // functional bus completes each access before a following read.
            if a == 0x40200 {
                value & !0x80000000
            } else {
                value
            }
        })
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
        if (a == 0x40304 && v != 0) || ((a == 0x40310 || a == 0x40314) && v != 0) {
            return Some(Err(
                "SFC dynamic key and encrypted-window changes are not implemented",
            ));
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
                0x4b => {
                    // Puya P25Q80H: four dummy bytes then a 128-bit ID.
                    // Stable emulator chip identity, not the physical unit's ID.
                    const UID: [u8; 16] = *b"FM1-EMU-NOR-0001";
                    if len > 5 {
                        value = UID.get(len - 6).copied().unwrap_or(255) as u32;
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::Nor;

    #[test]
    fn unique_id_consumes_four_dummy_bytes_and_restarts_on_chip_select() {
        let mut nor = Nor::default();
        for _ in 0..2 {
            nor.chip_select(true);
            for byte in [0x4b, 0, 0, 0, 0] {
                nor.write(0x11c08, byte).unwrap().unwrap();
                assert_eq!(nor.read(0x11c08), Some(255));
            }
            for &byte in b"FM1-EMU-NOR-0001" {
                nor.write(0x11c08, 255).unwrap().unwrap();
                assert_eq!(nor.read(0x11c08), Some(byte as u32));
            }
            nor.chip_select(false);
        }
    }

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
