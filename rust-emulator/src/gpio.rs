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
    /// The panel LEDs these pins light (an observer: the guest never sees it).
    pub leds: crate::leds::Leds,
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
            leds: Default::default(),
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

    /// The LED lines and the latched columns, after a write that may move them.
    #[inline]
    fn observe_leds(&mut self) {
        let lines =
            crate::leds::LINE_PINS
                .iter()
                .enumerate()
                .fold(0, |lines, (line, &(port, pin))| {
                    let p = &self.ports[port];
                    let driven = p[OUT as usize / 4] & !p[DIR as usize / 4];
                    lines | (((driven >> pin) & 1) as u8) << line
                });
        self.leds.update(self.latched, lines);
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
            // DIE enables the input buffer; DIR controls the output driver.
            // Stock leaves PA1's input buffer disabled while pulsing its latch.
            let driven = a[OUT as usize / 4] & !a[DIR as usize / 4];
            if driven & 8 != 0 && self.previous_driven_a & 8 == 0 {
                self.shift = (self.shift << 1) | ((driven >> 4) & 1) as u16;
            }
            if driven & 2 != 0 && self.previous_driven_a & 2 == 0 {
                self.latched = self.shift;
            }
            self.previous_driven_a = driven;
        }
        if port == 0 || port == 7 {
            self.observe_leds(); // The latch and the LED lines are on PA and PH.
        }
        Some(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leds::{by_key, KEYMAP};

    const PA: u32 = GPIO;
    const PH: u32 = GPIO + 7 * 0x40;

    fn set(gpio: &mut Gpio, address: u32, value: u32) {
        gpio.write(address, value).unwrap().unwrap();
    }

    /// Shift `word` into the 595 chain on PA4/PA3 and latch it with PA1.
    fn latch(gpio: &mut Gpio, word: u16) {
        for bit in (0..16).rev() {
            let serial = if word & (1 << bit) != 0 { 16 } else { 0 };
            set(gpio, PA + OUT, serial);
            set(gpio, PA + OUT, serial | 8);
        }
        set(gpio, PA + OUT, 2);
        set(gpio, PA + OUT, 0);
    }

    #[test]
    fn an_led_line_lights_the_led_of_the_latched_column() {
        let mut gpio = Gpio::default();
        set(&mut gpio, PA + DIR, !0x1a); // PA1, PA3, PA4 drive the 595s.
        set(&mut gpio, PH + DIR, !0x240); // PH6 and PH9 drive LED lines.
                                          // Four scan frames: PH6 (row PA7) is driven high on column 3 only,
                                          // for the second half of that column's time.
        for _ in 0..4 {
            for column in 0..11 {
                set(&mut gpio, PH + OUT, 0);
                latch(&mut gpio, !(1 << column));
                gpio.leds.advance(100);
                set(&mut gpio, PH + OUT, if column == 3 { 0x40 } else { 0 });
                gpio.leds.advance(100);
            }
        }
        set(&mut gpio, PH + OUT, 0);
        let keys = by_key(&gpio.leds.take());
        let lit = KEYMAP[2][3] as usize;
        assert_eq!(keys[lit], 0.5);
        assert!(keys
            .iter()
            .enumerate()
            .all(|(id, &level)| id == lit || level == 0.));
        // An input pin with its output latch high lights nothing.
        set(&mut gpio, PH + DIR, u32::MAX);
        set(&mut gpio, PH + OUT, 0x40);
        gpio.leds.advance(1000);
        assert!(gpio.leds.take().iter().flatten().all(|&level| level == 0.));
    }
}
