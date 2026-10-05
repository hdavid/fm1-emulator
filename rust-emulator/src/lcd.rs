// SPDX-License-Identifier: GPL-3.0-only
// Functional SPI1/ST7789 subset used by firmware/hal/fm1_lcd_hw.h and src/lcd.c.
// Transfers complete synchronously; SPI baud timing and panel refresh are not modeled.
pub const SPI: u32 = 0x11d00;
pub const IOMAP: u32 = 0x51020;
pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 240;
pub(crate) const IRQ: usize = 16;
const GRAM_HEIGHT: usize = 320;

pub struct Lcd {
    pub pixels: Vec<u32>,
    pub pixels_written: u64,
    pub display_on: bool,
    pub sleeping: bool,
    command: u8,
    args: Vec<u8>,
    columns: [usize; 2],
    rows: [usize; 2],
    cursor: [usize; 2],
    high_byte: Option<u8>,
    format: u8,
    bgr: bool,
    registers: [u32; 5],
    iomap: u32,
    gram: Vec<u32>,
    view_row: usize,
    panel_configuration: [Vec<u8>; 256],
}

impl Default for Lcd {
    fn default() -> Self {
        Self {
            pixels: vec![0; WIDTH * HEIGHT],
            pixels_written: 0,
            display_on: false,
            sleeping: true,
            command: 0,
            args: Vec::new(),
            columns: [0, WIDTH - 1],
            rows: [0, GRAM_HEIGHT - 1],
            cursor: [0, 0],
            high_byte: None,
            format: 0,
            bgr: false,
            registers: [0; 5],
            iomap: 0,
            gram: vec![0; WIDTH * GRAM_HEIGHT],
            view_row: 0,
            panel_configuration: std::array::from_fn(|_| Vec::new()),
        }
    }
}

impl Lcd {
    pub(crate) fn pending_irq(&self) -> bool {
        // Vendor spi0.c: bit 13 enables transfer-complete interrupts,
        // bit 15 reports completion, and bit 14 acknowledges it.
        self.registers[0] & 0xa001 == 0xa001
    }
    pub fn read(&self, address: u32) -> Option<u32> {
        if address == IOMAP {
            return Some(self.iomap);
        }
        if (SPI..SPI + 20).contains(&address) && address.is_multiple_of(4) {
            Some(self.registers[((address - SPI) / 4) as usize])
        } else {
            None
        }
    }

    pub fn dma_address(&self) -> u32 {
        self.registers[3]
    }

    pub fn write(
        &mut self,
        address: u32,
        value: u32,
        selected: bool,
        data: bool,
        dma: &[u8],
    ) -> Result<(), &'static str> {
        if address == IOMAP {
            self.iomap = value;
            return Ok(());
        }
        let index = ((address - SPI) / 4) as usize;
        if index == 0 {
            let pending = if value & 0x4000 != 0 {
                0
            } else {
                self.registers[0] & 0x8000
            };
            self.registers[0] = (value & 0x3fff) | pending;
            return Ok(());
        }
        self.registers[index] = value;
        if index != 2 && index != 4 {
            return Ok(());
        }
        if self.registers[0] & 0x1fff != 0x21 || self.iomap & 0x10 == 0 {
            return Err("unsupported LCD SPI configuration or pin routing");
        }
        if selected {
            if index == 2 {
                self.transfer(value as u8, data)?;
            } else {
                for byte in dma {
                    self.transfer(*byte, data)?;
                }
            }
        }
        self.registers[0] |= 0x8000;
        Ok(())
    }

    fn transfer(&mut self, byte: u8, data: bool) -> Result<(), &'static str> {
        if !data {
            self.command = byte;
            self.args.clear();
            self.high_byte = None;
            match byte {
                0x01 => {
                    // Software reset affects the panel, not the host SPI registers.
                    self.pixels.fill(0);
                    self.gram.fill(0);
                    self.view_row = 0;
                    self.panel_configuration.iter_mut().for_each(Vec::clear);
                    self.display_on = false;
                    self.sleeping = true;
                    self.columns = [0, WIDTH - 1];
                    self.rows = [0, GRAM_HEIGHT - 1];
                    self.format = 0;
                    self.bgr = false;
                }
                0x10 => self.sleeping = true,
                0x11 => self.sleeping = false,
                0x28 => self.display_on = false,
                0x29 => self.display_on = true,
                0x2c => self.cursor = [self.columns[0], self.rows[0]],
                // Inversion is the panel's electrical drive mode. RGB565 values
                // represent visible colors for the FM-1's normal INVON setup.
                0x13 | 0x20 | 0x21 | 0x2a | 0x2b | 0x36 | 0x3a => (),
                c if Self::configuration_length(c).is_some() => (),
                _ => return Err("unsupported LCD command"),
            }
        } else if self.command == 0x2c {
            if self.format != 0x55 {
                return Err("LCD requires RGB565 (COLMOD 0x55)");
            }
            if let Some(high) = self.high_byte.take() {
                let pixel = u16::from_be_bytes([high, byte]) as u32;
                let mut r = (pixel >> 11) & 31;
                let g = (pixel >> 5) & 63;
                let mut b = pixel & 31;
                if self.bgr {
                    std::mem::swap(&mut r, &mut b);
                }
                let rgb = (((r << 3) | (r >> 2)) << 16)
                    | (((g << 2) | (g >> 4)) << 8)
                    | (b << 3)
                    | (b >> 2);
                self.gram[self.cursor[1] * WIDTH + self.cursor[0]] = rgb;
                if let Some(y) = self.cursor[1]
                    .checked_sub(self.view_row)
                    .filter(|y| *y < HEIGHT)
                {
                    self.pixels[y * WIDTH + self.cursor[0]] = rgb;
                }
                self.pixels_written += 1;
                self.cursor[0] += 1;
                if self.cursor[0] > self.columns[1] {
                    self.cursor[0] = self.columns[0];
                    self.cursor[1] += 1;
                    if self.cursor[1] > self.rows[1] {
                        self.cursor[1] = self.rows[0];
                    }
                }
            } else {
                self.high_byte = Some(byte);
            }
        } else {
            self.args.push(byte);
            match self.command {
                0x2a | 0x2b => {
                    if self.args.len() > 4 {
                        return Err("too many LCD window parameters");
                    }
                    if self.args.len() == 4 {
                        let start = u16::from_be_bytes([self.args[0], self.args[1]]) as usize;
                        let end = u16::from_be_bytes([self.args[2], self.args[3]]) as usize;
                        let limit = if self.command == 0x2a {
                            WIDTH
                        } else {
                            GRAM_HEIGHT
                        };
                        if start > end || end >= limit {
                            // ST7789 CASET/RASET ignore out-of-range addresses.
                            // The stock clear routine sends column end=240;
                            // retain the previous valid column window (0..239).
                            return Ok(());
                        }
                        if self.command == 0x2a {
                            self.columns = [start, end];
                        } else {
                            self.rows = [start, end];
                            if self.pixels_written == 0 && end - start + 1 == HEIGHT {
                                // Present the guest's initial full-panel window:
                                // stock uses rows 40..279, Felucca uses 0..239.
                                // This is a functional viewport; panel gate-line
                                // placement still needs physical verification.
                                self.view_row = start;
                            }
                        }
                    }
                }
                0x3a if self.args.len() == 1 && byte == 0x55 => self.format = byte,
                0x36 if self.args.len() == 1 && byte & !8 == 0 => self.bgr = byte & 8 != 0,
                c if Self::configuration_length(c).is_some() => {
                    if self.args.len() > Self::configuration_length(c).unwrap() {
                        return Err("too many LCD configuration parameters");
                    }
                    if c == 0xe7 && byte & 0x10 != 0 {
                        return Err("LCD dual data lane mode is not implemented");
                    }
                    self.panel_configuration[c as usize].clone_from(&self.args);
                }
                _ => return Err("unsupported LCD data or pixel orientation"),
            }
        }
        Ok(())
    }

    fn configuration_length(command: u8) -> Option<usize> {
        // ST7789V datasheet (Sitronix, v1.3), table 2. Preserve electrical,
        // gamma and brightness settings; analog panel response is not modeled.
        // https://dl.espressif.com/dl/schematics/LCD_ST7789.pdf
        Some(match command {
            0xb2 => 5,
            0xb7 | 0xbb | 0xc0 | 0xc2 | 0xc3 | 0xc4 | 0xc6 | 0xe7 | 0x51 => 1,
            0xd0 => 2,
            0xe0 | 0xe1 => 14,
            _ => return None,
        })
    }
}
