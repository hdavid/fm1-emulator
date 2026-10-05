// SPDX-License-Identifier: GPL-3.0-only
// P33 serial register bridge, as used by the hardware watchdog HAL.
pub struct System {
    pmu_control: u32,
    rtc_control: u32,
    osa_control: u32,
    control: u32,
    data: u8,
    transaction: Vec<u8>,
    registers: [u8; 2048],
    pub watchdog_feeds: u64,
    pub watchdog_ticks: u64,
    /// Diagnostics only (diagnose FM1_WATCHDOG_OFF=1): keep counting but
    /// never expire.
    pub watchdog_expiry_disabled: bool,
}
impl Default for System {
    fn default() -> Self {
        let mut registers = [0; 2048];
        registers[0x12] = 1; // Power-on reset.
        Self {
            pmu_control: 0x100,
            rtc_control: 0xe0,
            osa_control: 0,
            control: 0,
            data: 0,
            transaction: Vec::new(),
            registers,
            watchdog_feeds: 0,
            watchdog_ticks: 0,
            watchdog_expiry_disabled: false,
        }
    }
}
impl System {
    pub(crate) fn adc_pmu_selection(&self) -> u8 {
        (self.registers[4] >> 1) & 7
    }
    pub fn read(&self, address: u32) -> Option<u32> {
        match address {
            0x13400 => Some(self.osa_control),
            0x13e00 => Some(self.pmu_control),
            0x13e04 => Some(self.rtc_control),
            0x13e08 => Some(self.control),
            0x13e0c => Some(self.data as u32),
            0x100c0 => Some(0),
            _ => None,
        }
    }
    pub fn write(&mut self, address: u32, value: u32) -> Option<Result<(), &'static str>> {
        match address {
            // OSA IRQ wrapper acknowledges bit 6; no protection event is
            // pending while the emulator executes ordinary valid accesses.
            0x13400 => self.osa_control = value & !0x40,
            0x13e00 => self.pmu_control = value,
            0x13e04 => self.rtc_control = value,
            0x13e0c => self.data = value as u8,
            0x13e08 => {
                if value & 1 == 0 || self.control & 1 == 0 || (value ^ self.control) & 0x100 != 0 {
                    self.transaction.clear();
                }
                self.control = value & !0x12;
                if value & 0x11 == 0x11 {
                    self.transaction.push(self.data);
                    if self.transaction.len() == 3 {
                        let command = self.transaction[0];
                        let index = ((command as usize & 3) << 8)
                            | self.transaction[1] as usize
                            | if value & 0x100 != 0 { 1024 } else { 0 };
                        if command & 0x80 != 0 {
                            self.data = self.registers[index];
                        } else {
                            let old = self.registers[index];
                            let result = match (command >> 5) & 3 {
                                0 => self.data,
                                1 => old | self.data,
                                2 => old & self.data,
                                _ => old ^ self.data,
                            };
                            if index == 0xa0 && result & 0x10 != 0 {
                                return Some(Err("firmware requested a chip reset"));
                            }
                            if index == 0x80 && result & 0x40 != 0 {
                                self.watchdog_feeds += 1;
                                self.watchdog_ticks = 0;
                            }
                            self.registers[index] = if index == 0x80 {
                                result & !0x40
                            } else {
                                result
                            };
                        }
                    }
                }
            }
            _ => return None,
        }
        Some(Ok(()))
    }
    /// Oscillator ticks from a feed to expiry, if the watchdog is enabled.
    pub fn watchdog_timeout(&self) -> Option<u64> {
        let wdt = self.registers[0x80];
        (wdt & 0x10 != 0).then(|| 24_000_000u64 * (1u64 << (wdt & 15).saturating_sub(10)))
    }
    /// Oscillator ticks until the enabled watchdog expires (a fault).
    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        let timeout = self.watchdog_timeout()?;
        if self.watchdog_expiry_disabled && self.watchdog_ticks >= timeout {
            return None;
        }
        Some(timeout.saturating_sub(self.watchdog_ticks).max(1))
    }
    pub fn advance(&mut self, ticks: u32) -> Result<(), &'static str> {
        let wdt = self.registers[0x80];
        if wdt & 0x10 != 0 {
            self.watchdog_ticks += ticks as u64;
            let timeout = 24_000_000u64 * (1u64 << (wdt & 15).saturating_sub(10));
            if self.watchdog_ticks >= timeout && !self.watchdog_expiry_disabled {
                return Err("watchdog expired");
            }
        }
        Ok(())
    }
}
