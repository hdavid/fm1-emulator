// SPDX-License-Identifier: GPL-3.0-only
// Polled SARADC subset used by Felucca's fm1_adc.h. Conversion timing is
// functional; inputs represent the battery divider, master potentiometer and
// the internal PMU channels sampled by the stock SDK.
pub const CONTROL: u32 = 0x13100;
pub const RESULT: u32 = 0x13104;
const CONVERSION_TICKS: u32 = 32;

pub struct Adc {
    control: u32,
    result: u16,
    remaining: u32,
    sample: u16,
    wireless_control: u32,
    pmu_channel: u8,
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
            pmu_channel: 0,
            master: 512,
            battery: 800,
            conversions: 0,
        }
    }
}

impl Adc {
    pub(crate) fn select_pmu(&mut self, channel: u8) {
        self.pmu_channel = channel;
    }
    pub(crate) fn pending_irq(&self) -> bool {
        self.control & 0xa0 == 0xa0
    }
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
                // Bit 6 acknowledges completion and restarts an enabled ADC.
                // Stock takes repeated samples by writing this strobe again.
                let start = value & 0x50 == 0x50;
                if start {
                    self.sample = match (value >> 8) & 15 {
                        3 => self.battery,
                        4 => self.master,
                        // P33 ANA_CON4 mux: WL82 adc_api.c uses CENTER0=800
                        // mV for the nominal untrimmed bandgap reference.
                        // The battery channel measures one quarter of 4.2 V;
                        // both sources are sampled against the same 3.3 V rail.
                        15 => match self.pmu_channel {
                            0 => (1023u32 * 800 / 3300) as u16,
                            // FM-1_997 measured VTEMP (mux 3) at 349..351 in
                            // 60 physical samples. Use its median as a fixed
                            // sensor reading; temperature drift is not modeled.
                            3 => 350,
                            5 => (1023u32 * 1050 / 3300) as u16,
                            _ => return Some(Err("unsupported PMU ADC source")),
                        },
                        _ => return Some(Err("unsupported ADC channel")),
                    } & 1023;
                    self.remaining = CONVERSION_TICKS;
                }
                if value & 0x10 == 0 {
                    self.remaining = 0;
                }
                // Conversion completion is hardware-owned, cleared on restart
                // or disable; a normal read/modify/write retains it.
                self.control = (value & !0xc0)
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
