// SPDX-License-Identifier: GPL-3.0-only
// Functional corex2 cache controls/tag storage and RAM reclaimed from ways.
// Register layout: vendor SDK asm/csfr.h; startup ranges: unchanged stock code.
pub(crate) struct Cache {
    regs: [u32; 3],
    cores: [u32; 2],
    tags: Vec<u8>,
    ram: Vec<u8>,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            regs: [0; 3],
            cores: [0, 2],
            tags: vec![0; 0xc000],
            ram: vec![0; 0x10000],
        }
    }
}
impl Cache {
    pub(crate) fn core_control(&self, core: usize) -> u32 {
        self.cores[core]
    }
    fn offset(a: u32, n: usize, base: u32, len: usize) -> Option<usize> {
        let offset = a.checked_sub(base)? as usize;
        (offset.checked_add(n)? <= len).then_some(offset)
    }
    pub fn read(&self, a: u32, n: usize) -> Option<u32> {
        if n == 4 && matches!(a, 0x1eee000 | 0x1eee004) {
            return Some(self.cores[((a - 0x1eee000) / 4) as usize]);
        }
        if n == 4 && matches!(a, 0x40400 | 0x40500 | 0x40438) {
            // SRAM/SFC machine: SDR and PSRAM controllers are disabled.
            return Some(0);
        }
        if n == 4 && (0x1eee008..=0x1eee010).contains(&a) {
            let mut value = self.regs[((a - 0x1eee008) / 4) as usize];
            if a == 0x1eee008 {
                value |= 0x4000;
            } // No outstanding cache fill.
            return Some(value);
        }
        let data = if let Some(offset) = Self::offset(a, n, 0x1f00000, self.tags.len()) {
            &self.tags[offset..offset + n]
        } else {
            let offset = Self::offset(a, n, 0x1f20000, self.ram.len())?;
            &self.ram[offset..offset + n]
        };
        Some(
            data.iter()
                .enumerate()
                .fold(0, |v, (i, b)| v | ((*b as u32) << (8 * i))),
        )
    }
    pub fn write(&mut self, a: u32, n: usize, v: u32) -> Option<()> {
        if n == 4 && matches!(a, 0x1eee000 | 0x1eee004) {
            let core = &mut self.cores[((a - 0x1eee000) / 4) as usize];
            // Stock flash exclusion: pause request bit 2, stopped status bit 4,
            // and resume command bit 3. Commands complete at a bundle boundary.
            *core = if v & 4 != 0 {
                (v & !0x1c) | 16
            } else if v & 8 != 0 {
                v & !0x14
            } else {
                v
            };
            return Some(());
        }
        if n == 4 && matches!(a, 0x40400 | 0x40500 | 0x40438) && v == 0 {
            return Some(());
        }
        if n == 4 && (0x1eee008..=0x1eee010).contains(&a) {
            self.regs[((a - 0x1eee008) / 4) as usize] =
                if a == 0x1eee008 { v & !0x4000 } else { v };
            return Some(());
        }
        let data = if let Some(offset) = Self::offset(a, n, 0x1f00000, self.tags.len()) {
            &mut self.tags[offset..offset + n]
        } else {
            let offset = Self::offset(a, n, 0x1f20000, self.ram.len())?;
            &mut self.ram[offset..offset + n]
        };
        data.copy_from_slice(&v.to_le_bytes()[..n]);
        Some(())
    }
}
