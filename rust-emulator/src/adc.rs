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
    /// P33 P3_ANA_CON4 at conversion start: bit 0 PMU_DET_EN, bits 1-3
    /// ADC_CHANNEL_SEL (AC79 SDK p33.h / adc_api.h).
    pub pmu_select: u8,
}

/// ADC full scale (VDDIO) in millivolts.
const VDDIO_MV: u32 = 3300;
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
            pmu_select: 0,
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
                        15 => match self.pmu_sample() {
                            Ok(sample) => sample,
                            Err(reason) => return Some(Err(reason)),
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

    /// A PMU sub-channel as the 10-bit SARADC reads it. The ADC's full scale
    /// is VDDIO (AC79 SDK adc_api.c vddiom_trim: vddio = vbg * 1023 / adc),
    /// modelled at 3.3 V; the sources are nominal supply voltages. The SDK's
    /// wvdd_trim raises the WVDD LDO until vbg-scaled WVDD exceeds 700 mV.
    fn pmu_sample(&self) -> Result<u16, &'static str> {
        if self.pmu_select & 1 == 0 {
            return Err("PMU ADC channel sampled with PMU_DET_EN clear");
        }
        let millivolts: u32 = match (self.pmu_select >> 1) & 7 {
            0 => 800,  // VBG, the trim centre (CENTER0)
            1 => 1400, // VDC14
            2 => 1200, // SYSVDD
            3 => return Err("unmodeled PMU ADC sub-channel VTEMP"),
            4 => return Err("unmodeled PMU ADC sub-channel PROGF"),
            5 => 1050, // VBAT/4: 4.2 V, powered over USB
            6 => 1250, // LDO5V/4: 5 V USB
            _ => 750,  // WVDD, the radio LDO's target
        };
        Ok((millivolts * 1023 / VDDIO_MV) as u16)
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
