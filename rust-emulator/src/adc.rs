// SPDX-License-Identifier: GPL-3.0-only
// Polled SARADC subset used by Felucca's fm1_adc.h. Conversion timing is
// functional; inputs represent the battery divider and master potentiometer.
pub const CONTROL: u32 = 0x13100;
pub const RESULT: u32 = 0x13104;
const CONVERSION_TICKS: u32 = 32;

pub struct Adc {
    control: u32,
    result: u16,
    remaining: u32,
    sample: u16,
    wireless_control: u32,
    pub master: u16,
    pub battery: u16,
    pub conversions: u64,
}

impl Default for Adc {
    fn default() -> Self {
        Self {
            control: 0,
            result: 0,
            remaining: 0,
            sample: 0,
            wireless_control: 0,
            master: 512,
            battery: 800,
            conversions: 0,
        }
    }
}

impl Adc {
    pub fn read(&self, address: u32) -> Option<u32> {
        match address {
            CONTROL => Some(self.control),
            RESULT => Some(self.result as u32),
            0x11900 => Some(self.wireless_control),
            _ => None,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<Result<(), &'static str>> {
        match address {
            CONTROL => {
                let start = value & 0x50 == 0x50 && self.control & 0x50 != 0x50;
                if start {
                    self.sample = match (value >> 8) & 15 {
                        3 => self.battery,
                        4 => self.master,
                        _ => return Some(Err("unsupported ADC channel")),
                    } & 1023;
                    self.remaining = CONVERSION_TICKS;
                }
                if value & 0x10 == 0 {
                    self.remaining = 0;
                }
                // Conversion completion is hardware-owned, cleared on restart
                // or disable; a normal read/modify/write retains it.
                self.control = (value & !0x80)
                    | if !start && value & 0x10 != 0 {
                        self.control & 0x80
                    } else {
                        0
                    };
            }
            RESULT => return Some(Err("ADC result is read-only")),
            0x11900 => self.wireless_control = value,
            _ => return None,
        }
        Some(Ok(()))
    }

    pub fn advance(&mut self, ticks: u32) {
        if self.remaining == 0 {
            return;
        }
        self.remaining = self.remaining.saturating_sub(ticks);
        if self.remaining == 0 {
            self.result = self.sample;
            self.control |= 0x80;
            self.conversions += 1;
        }
    }
}
