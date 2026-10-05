// SPDX-License-Identifier: GPL-3.0-only
// Functional SPI1/ST7789 subset used by firmware/hal/fm1_lcd_hw.h and src/lcd.c.
// Transfers complete synchronously; SPI baud timing and panel refresh are not modeled.
pub const SPI: u32 = 0x11d00;
pub const IOMAP: u32 = 0x51020;
/// SPI1 interrupt source (Felucca fm1_irq.h FM1_IRQ_SPI1).
pub const IRQ: usize = 16;
const LSB_PER_OSC: u64 = 4;
pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 240;
/// ST7789V frame memory rows (240 x 320). Which 240 rows the FM-1 glass
/// shows is not established: Felucca draws rows 0-239, the stock app 40-279
/// (RASET 0x28..0x117, both with MADCTL 0). `pixels` keeps rows 0-239.
pub const RAM_ROWS: usize = 320;
const CONFIG_COMMANDS: [u8; 13] = [
    0x51, 0xb2, 0xb7, 0xbb, 0xc0, 0xc2, 0xc3, 0xc4, 0xc6, 0xd0, 0xe0, 0xe1, 0xe7,
];

pub struct Lcd {
    pub pixels: Vec<u32>,
    /// Frame memory rows 240..320.
    offscreen: Vec<u32>,
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
    /// lsb ticks until an interrupt-mode transfer completes (see write).
    busy: u64,
}

impl Default for Lcd {
    fn default() -> Self {
        Self {
            pixels: vec![0; WIDTH * HEIGHT],
            offscreen: vec![0; WIDTH * (RAM_ROWS - HEIGHT)],
            pixels_written: 0,
            display_on: false,
            sleeping: true,
            command: 0,
            args: Vec::new(),
            columns: [0, WIDTH - 1],
            rows: [0, HEIGHT - 1],
            cursor: [0, 0],
            high_byte: None,
            format: 0,
            bgr: false,
            registers: [0; 5],
            iomap: 0,
            busy: 0,
        }
    }
}

impl Lcd {
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

    /// The whole 240 x 320 frame memory, rows 0-239 then 240-319.
    pub fn frame_memory(&self) -> Vec<u32> {
        let mut all = self.pixels.clone();
        all.extend_from_slice(&self.offscreen);
        all
    }

    /// SPI1 interrupt request: pending (bit 15) with IE (bit 13).
    pub fn pending_irq(&self) -> bool {
        self.registers[0] & 0xa000 == 0xa000
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
        // CON 0x21 (Felucca HAL) or 0x2021: the stock driver also sets bit
        // 13, the interrupt enable (same layout as its SPI2 CON 0x6020).
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
        if self.registers[0] & 0x2000 != 0 {
            // Interrupt mode (stock CON 0x2021): pending, and with it IRQ 16,
            // comes after the shift time, bytes * 8 * (BAUD + 1) lsb ticks,
            // with lsb_clk ASSUMED to be 4x the oscillator (as spi2.rs).
            // The stock driver chains the next DMA from the interrupt, so
            // instant completion kept the CPU in the handler until a queue
            // lock overflowed (FreeRTOS assert at 0x0205a1fa). Polled mode
            // (Felucca) keeps completing at once, as before.
            let bytes = if index == 2 { 1 } else { dma.len() as u64 };
            self.busy = (bytes * 8 * ((self.registers[1] & 0xff) as u64 + 1)).max(1);
        } else {
            self.registers[0] |= 0x8000;
        }
        Ok(())
    }

    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        (self.busy != 0).then(|| self.busy.div_ceil(LSB_PER_OSC))
    }

    /// Advance by oscillator ticks; a finished transfer sets pending.
    pub fn advance(&mut self, ticks: u32) {
        if self.busy == 0 {
            return;
        }
        self.busy = self.busy.saturating_sub(ticks as u64 * LSB_PER_OSC);
        if self.busy == 0 {
            self.registers[0] |= 0x8000;
        }
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
                    self.offscreen.fill(0);
                    self.display_on = false;
                    self.sleeping = true;
                    self.columns = [0, WIDTH - 1];
                    self.rows = [0, HEIGHT - 1];
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
                // ST7789V porch, gate, VCOM, power, frame-rate, gamma, SPI and
                // brightness settings from the stock init table (flash
                // 0x0204f8d4..): analog setup with no frame-memory effect.
                c if CONFIG_COMMANDS.contains(&c) => (),
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
                let [x, y] = self.cursor;
                if x < WIDTH && y < HEIGHT {
                    self.pixels[y * WIDTH + x] = rgb;
                    self.pixels_written += 1;
                } else if x < WIDTH && y < RAM_ROWS {
                    self.offscreen[(y - HEIGHT) * WIDTH + x] = rgb;
                    self.pixels_written += 1;
                }
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
                        // ST7789V CASET/RASET: XS <= XE is required; addresses
                        // beyond the frame memory are accepted and the data
                        // sent to them is ignored (datasheet note). The stock
                        // app sends CASET 0..240 (0x02022286).
                        if start > end {
                            return Err("LCD window start after its end");
                        }
                        if self.command == 0x2a {
                            self.columns = [start, end];
                        } else {
                            self.rows = [start, end];
                        }
                    }
                }
                c if CONFIG_COMMANDS.contains(&c) => (),
                0x3a if self.args.len() == 1 && byte == 0x55 => self.format = byte,
                0x36 if self.args.len() == 1 && byte & !8 == 0 => self.bgr = byte & 8 != 0,
                _ => return Err("unsupported LCD data or pixel orientation"),
            }
        }
        Ok(())
    }
}
