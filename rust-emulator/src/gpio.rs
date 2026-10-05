// SPDX-License-Identifier: GPL-3.0-only
// GPIO and 2x74HC595 matrix wiring from firmware/hal/fm1_input.h/fm1_gpio.h.
pub const GPIO: u32 = 0x50000;
pub const OUT: u32 = 0;
pub const IN: u32 = 4;
pub const DIR: u32 = 8;
pub const DIE: u32 = 12;
pub const PU: u32 = 16;
pub const PD: u32 = 20;
/// JL_IOMAP CON2-CON4 and CON6-CON8 (SDK WL82.h psfr 0x1007: CON0 at
/// 0x5101c). CON0, CON1 and CON5 are modelled with their users (nor.rs,
/// lcd.rs, audio.rs). Pin-mux routing only; reset value unmeasured (0).
const IOMAP_OTHER: [u32; 6] = [0x51024, 0x51028, 0x5102c, 0x51034, 0x51038, 0x5103c];

pub struct Gpio {
    ports: [[u32; 8]; 8],
    matrix: [u8; 11],
    shift: u16,
    pub latched: u16,
    previous_driven_a: u32,
    /// Port A input reads while each matrix column was selected: how many
    /// times the guest has sampled that column's contacts (encoder pacing).
    scans: [std::cell::Cell<u32>; 11],
    iomap: [u32; 6],
}

impl Default for Gpio {
    fn default() -> Self {
        let mut ports = [[0; 8]; 8];
        for port in &mut ports {
            port[DIR as usize / 4] = u32::MAX;
        }
        Self {
            ports,
            matrix: [0; 11],
            shift: u16::MAX,
            latched: u16::MAX,
            previous_driven_a: 0,
            scans: Default::default(),
            iomap: [0; 6],
        }
    }
}

impl Gpio {
    pub fn press(&mut self, column: usize, row: usize, pressed: bool) -> Result<(), &'static str> {
        if column >= self.matrix.len() || row >= 6 {
            return Err("matrix key must be COLUMN:ROW (columns 0..10, rows 0..5)");
        }
        if pressed {
            self.matrix[column] |= 1 << row;
        } else {
            self.matrix[column] &= !(1 << row);
        }
        Ok(())
    }

    fn selected_rows(&self) -> u8 {
        self.matrix
            .iter()
            .enumerate()
            .fold(0, |rows, (column, keys)| {
                if self.latched & (1 << column) == 0 {
                    rows | keys
                } else {
                    rows
                }
            })
    }

    /// Times the guest has read the rows while `column` was selected.
    pub fn column_scans(&self, column: usize) -> u32 {
        self.scans[column].get()
    }

    fn input(&self, port: usize) -> u32 {
        if port == 0 {
            for (column, count) in self.scans.iter().enumerate() {
                if self.latched & (1 << column) == 0 {
                    count.set(count.get().wrapping_add(1));
                }
            }
        }
        let registers = self.ports[port];
        let mut input = registers[PU as usize / 4] & !registers[PD as usize / 4];
        let rows = self.selected_rows() as u32;
        let low = if port == 0 {
            (rows & 1) | ((rows & 0x1e) << 4)
        } else if port == 1 {
            (rows & 0x20) << 2
        } else {
            0
        };
        input &= !low;
        let direction = registers[DIR as usize / 4];
        ((input & direction) | (registers[OUT as usize / 4] & !direction))
            & registers[DIE as usize / 4]
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        if let Some(i) = IOMAP_OTHER.iter().position(|&a| a == address) {
            return Some(self.iomap[i]);
        }
        let offset = address.checked_sub(GPIO)?;
        let port = (offset / 0x40) as usize;
        let register = offset % 0x40;
        if port >= 8 || register > 0x1c || register % 4 != 0 {
            return None;
        }
        Some(if register == IN {
            self.input(port)
        } else {
            self.ports[port][register as usize / 4]
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<Result<(), &'static str>> {
        self.read(address)?;
        if let Some(i) = IOMAP_OTHER.iter().position(|&a| a == address) {
            self.iomap[i] = value;
            return Some(Ok(()));
        }
        let offset = address - GPIO;
        let port = (offset / 0x40) as usize;
        let register = offset % 0x40;
        if register == IN {
            return Some(Err("GPIO input register is read-only"));
        }
        self.ports[port][register as usize / 4] = value;
        if port == 0 {
            let a = self.ports[0];
            let driven = a[OUT as usize / 4] & !a[DIR as usize / 4] & a[DIE as usize / 4];
            if driven & 8 != 0 && self.previous_driven_a & 8 == 0 {
                self.shift = (self.shift << 1) | ((driven >> 4) & 1) as u16;
            }
            if driven & 2 != 0 && self.previous_driven_a & 2 == 0 {
                self.latched = self.shift;
            }
            self.previous_driven_a = driven;
        }
        Some(Ok(()))
    }
}
