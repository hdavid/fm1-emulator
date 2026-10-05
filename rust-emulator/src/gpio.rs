// SPDX-License-Identifier: GPL-3.0-only
// GPIO and 2x74HC595 matrix wiring from firmware/hal/fm1_input.h/fm1_gpio.h.
pub const GPIO: u32 = 0x50000;
pub const OUT: u32 = 0;
pub const IN: u32 = 4;
pub const DIR: u32 = 8;
pub const DIE: u32 = 12;
pub const PU: u32 = 16;
pub const PD: u32 = 20;

pub struct Gpio {
    ports: [[u32; 8]; 8],
    matrix: [u8; 11],
    shift: u16,
    pub latched: u16,
    previous_driven_a: u32,
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
        }
    }
}

impl Gpio {
    pub(crate) fn shift_spi(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.shift = (self.shift << 8) | byte as u16;
        }
    }
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

    fn input(&self, port: usize) -> u32 {
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
