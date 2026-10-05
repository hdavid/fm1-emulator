// SPDX-License-Identifier: GPL-3.0-only
// SPI0 serial NOR. Application ELF supplies decrypted XIP separately.
/// The SFC/SPI0 registers this model retains, each at a fixed index (`slot`).
/// An array, not a map: XIP reads several of them on every instruction fetch.
const NREG: usize = 14;
fn slot(a: u32) -> Option<usize> {
    Some(match a {
        0x40200 => 0,
        0x40204 => 1,
        0x40208 => 2,
        0x4020c => 3,
        0x40300 => 4,
        0x40304 => 5,
        0x40308 => 6,
        0x4030c => 7,
        0x40310 => 8,
        0x40314 => 9,
        0x5101c => 10,
        0x11c00 => 11,
        0x11c04 => 12,
        0x11c08 => 13,
        _ => return None,
    })
}
fn registers(values: &[(u32, u32)]) -> [u32; NREG] {
    let mut regs = [0; NREG];
    for &(a, v) in values {
        regs[slot(a).unwrap()] = v;
    }
    regs
}
/// XIP settings decoded from the registers (`Nor::refresh`), read on every
/// instruction fetch from flash.
#[derive(Default, Clone, Copy)]
struct XipConfig {
    active: bool,
    base: usize,
    encrypted: bool,
    window: Option<(u32, u32)>,
}
pub struct Nor {
    regs: [u32; NREG],
    xip: XipConfig,
    command: Vec<u8>,
    selected: bool,
    pub bytes: Vec<u8>,
    cursor: usize,
    decoded: Option<Vec<u8>>,
    key: u16,
    write_enabled: bool,
    /// Completed sector/block erases and page programs (diagnostics).
    pub erases: u64,
    pub programs: u64,
}

impl Default for Nor {
    fn default() -> Self {
        let mut nor = Self {
            regs: registers(&[(0x40200, 1), (0x4020c, 0x4000), (0x5101c, 32), (0x40300, 1)]),
            xip: XipConfig::default(),
            command: vec![],
            selected: false,
            bytes: vec![255; 1024 * 1024],
            cursor: 0,
            decoded: None,
            key: 0,
            write_enabled: false,
            erases: 0,
            programs: 0,
        };
        nor.refresh();
        nor
    }
}

impl Nor {
    pub fn load(&mut self, bytes: &[u8], key: u16) {
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        let mut decoded = self.bytes[0x4000..].to_vec();
        crate::package::sfc(&mut decoded, key);
        self.decoded = Some(decoded);
        self.key = key;
        for (a, v) in [(0x40200, 0x809803b5), (0x40204, 1), (0x40208, 0x8e17)] {
            self.regs[slot(a).unwrap()] = v;
        }
        self.refresh();
    }
    /// Re-decode the XIP settings after any register change.
    fn refresh(&mut self) {
        let reg = |a| self.regs[slot(a).unwrap()];
        let control = reg(0x40300);
        self.xip = XipConfig {
            active: reg(0x40200) & 1 != 0 && reg(0x5101c) & 32 != 0,
            base: reg(0x4020c) as usize,
            encrypted: control & 1 != 0,
            window: (control & 2 != 0).then(|| (reg(0x4030c), reg(0x40308))),
        };
    }
    pub fn packaged(&self) -> bool {
        self.decoded.is_some()
    }
    pub fn xip_active(&self) -> bool {
        self.xip.active
    }

    pub fn xip(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        // The SFC maps flash offset 0x4000 at CPU address 0x02000000.
        let xip = self.xip;
        let offset = address.checked_sub(0x0200_0000)? as usize + xip.base;
        let bytes = self.bytes.get(offset..offset.checked_add(size)?)?;
        if !xip.active {
            return Some(Err(
                "XIP unavailable while SFC or flash pin routing is disabled",
            ));
        }
        let plain = !xip.encrypted
            || match xip.window {
                Some((low, high)) => address >= low && address.checked_add(size as u32 - 1)? <= high,
                None => false,
            };
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
        slot(a).map(|i| {
            let value = self.regs[i];
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
        if self.selected && !selected {
            self.finish();
        }
        if self.selected != selected {
            self.command.clear();
            self.cursor = 0;
        }
        self.selected = selected;
    }
    /// Complete a command at chip-select rise, as a P25Q80H-class NOR does.
    /// Timing is functional: erase/program finish instantly (WIP stays 0).
    fn finish(&mut self) {
        let Some(&command) = self.command.first() else {
            return;
        };
        let len = self.command.len();
        let address = || {
            ((self.command[1] as usize) << 16)
                | ((self.command[2] as usize) << 8)
                | self.command[3] as usize
        };
        match command {
            0x06 if len == 1 => self.write_enabled = true,
            0x04 if len == 1 => self.write_enabled = false,
            0x20 | 0x52 | 0xd8 if len == 4 && self.write_enabled => {
                let size = match command {
                    0x20 => 0x1000,
                    0x52 => 0x8000,
                    _ => 0x10000,
                };
                let start = (address() & !(size - 1)) % self.bytes.len();
                self.bytes[start..start + size].fill(0xff);
                self.redecode(start, size);
                self.write_enabled = false;
                self.erases += 1;
            }
            0x02 if len > 4 && self.write_enabled => {
                let base = address() % self.bytes.len();
                let page = base & !0xff;
                for (i, &data) in self.command[4..].iter().enumerate() {
                    // Program clears bits only; addresses wrap within the page.
                    self.bytes[page | ((base + i) & 0xff)] &= data;
                }
                self.redecode(page, 0x100);
                self.write_enabled = false;
                self.programs += 1;
            }
            _ => {}
        }
    }

    // Keep the decrypted XIP view coherent with SPI writes (32-byte SFC blocks).
    fn redecode(&mut self, start: usize, length: usize) {
        let Some(decoded) = self.decoded.as_mut() else {
            return;
        };
        let first = start.max(0x4000);
        let end = start + length;
        if end <= first {
            return;
        }
        let block_start = (first - 0x4000) & !31;
        let block_end = (end - 0x4000).div_ceil(32) * 32;
        for block in (block_start..block_end.min(decoded.len())).step_by(32) {
            let stop = (block + 32).min(decoded.len());
            decoded[block..stop].copy_from_slice(&self.bytes[0x4000 + block..0x4000 + stop]);
            crate::package::enc(&mut decoded[block..stop], self.key ^ (block / 32 * 8) as u16);
        }
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
                // Status 1: WIP (bit 0) is never set because erase/program
                // complete at chip deselect; WEL is bit 1.
                0x05 => value = u32::from(self.write_enabled) << 1,
                0x35 => value = 0,
                // Write enable/disable, sector/block erase and page program
                // take effect when chip select rises (see finish).
                0x06 | 0x04 | 0x20 | 0x52 | 0xd8 | 0x02 => {}
                0x03 | 0x0b | 0x6b => {
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
            self.regs[slot(0x11c00).unwrap()] |= 0x8000;
        }
        self.regs[slot(a).unwrap()] = value;
        self.refresh();
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

    #[test]
    fn quad_output_read_uses_the_raw_package_bytes_after_its_dummy_byte() {
        let mut nor = Nor::default();
        nor.bytes[0x92ff0..0x92ff4].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        nor.chip_select(true);
        for byte in [0x6b, 9, 0x2f, 0xf0, 0] {
            nor.write(0x11c08, byte).unwrap().unwrap();
        }
        for expected in [0x12, 0x34, 0x56, 0x78] {
            nor.write(0x11c08, 255).unwrap().unwrap();
            assert_eq!(nor.read(0x11c08), Some(expected));
        }
    }

    fn transaction(nor: &mut Nor, bytes: &[u8]) -> Vec<u32> {
        nor.chip_select(true);
        let replies = bytes
            .iter()
            .map(|&byte| {
                nor.write(0x11c08, byte as u32).unwrap().unwrap();
                nor.read(0x11c08).unwrap()
            })
            .collect();
        nor.chip_select(false);
        replies
    }

    #[test]
    fn erase_and_program_need_write_enable_and_complete_at_deselect() {
        let mut nor = Nor::default();
        nor.bytes[0xd9000..0xda000].fill(0x5a);
        transaction(&mut nor, &[0x20, 0x0d, 0x90, 0x00]); // ignored: WEL clear
        assert_eq!(nor.bytes[0xd9000], 0x5a);
        transaction(&mut nor, &[0x06]);
        assert_eq!(transaction(&mut nor, &[0x05, 0xff])[1], 2);
        transaction(&mut nor, &[0x20, 0x0d, 0x98, 0x76]);
        assert!(nor.bytes[0xd9000..0xda000].iter().all(|&b| b == 0xff));
        assert_eq!(nor.bytes[0xda000], 0xff); // default erased storage
        assert_eq!(transaction(&mut nor, &[0x05, 0xff])[1], 0);
        transaction(&mut nor, &[0x06]);
        transaction(&mut nor, &[0x02, 0x0d, 0x90, 0xfe, 0x0f, 0xf0, 0x12]);
        assert_eq!(&nor.bytes[0xd90fe..0xd9100], &[0x0f, 0xf0]);
        assert_eq!(nor.bytes[0xd9000], 0x12); // wrapped within the page
        transaction(&mut nor, &[0x06]);
        transaction(&mut nor, &[0x02, 0x0d, 0x90, 0xfe, 0xf1]);
        assert_eq!(nor.bytes[0xd90fe], 0x01); // programming only clears bits
        assert_eq!((nor.erases, nor.programs), (1, 2));
    }

    #[test]
    fn programmed_bytes_stay_coherent_with_encrypted_xip() {
        let mut nor = Nor::default();
        nor.load(&vec![0xff; 0x8000], 0x980f);
        let address = 0x0200_1000; // physical 0x5000, decrypted window
        transaction(&mut nor, &[0x06]);
        transaction(&mut nor, &[0x20, 0x00, 0x50, 0x00]);
        let mut plain = [0x11u8, 0x22, 0x33, 0x44];
        crate::package::enc(&mut plain, 0x980f ^ (0x1000 / 32 * 8) as u16);
        transaction(&mut nor, &[0x06]);
        transaction(&mut nor, &[0x02, 0x00, 0x50, 0x00, plain[0], plain[1], plain[2], plain[3]]);
        assert_eq!(nor.xip(address, 4).unwrap().unwrap(), 0x4433_2211);
    }
}
