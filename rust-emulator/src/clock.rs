// SPDX-License-Identifier: GPL-3.0-only
// Functional WL82 clock selectors/PLL configuration, not a PLL timing model.
// Addresses: vendor WL82.h. Reset handoff: physical FM-1_982/995 clocks probes.
pub(crate) struct Clock {
    system: [u32; 4],
    pll: [u32; 4],
    usb_phy: [u32; 6],
    instruction_phase: u64,
    phase_hz: u32,
    issue_clock: Option<(u32, u32)>,
    /// Instructions per second of guest time when set, instead of the
    /// firmware's system clock (an opt-in for real-time playback).
    pub(crate) issue_override: Option<u32>,
    /// CPU clock cycles issued: one per instruction step of shared time.
    cycles: u64,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            system: [0, 0x10200, 0x1c1, 2],
            pll: [0x45400203, 0x3f503026, 0x0940022b, 0x0750310c],
            usb_phy: [0, 0x8881c3, 0, 0, 0x6003f, 0],
            instruction_phase: 0,
            phase_hz: 360_000_000,
            issue_clock: None,
            issue_override: None,
            cycles: 0,
        }
    }
}
impl Clock {
    // Vendor clock.c: CLK_CON3 selects the system PLL path; CLK_CON1
    // divides it into HSB and LSB. Peripheral timers select LSB or OSC.
    pub(crate) fn system_hz(&self, clk_con3: u32) -> u32 {
        let pll = (24_000_000 / (((self.pll[2] >> 2) & 31) + 2)) * ((self.pll[3] & 4095) + 2);
        match clk_con3 & 15 {
            source @ 0..=4 => {
                let hz = [
                    192_000_000,
                    137_000_000,
                    320_000_000,
                    480_000_000,
                    107_000_000,
                ][source as usize];
                hz / [1, 3, 5, 7][((clk_con3 >> 4) & 3) as usize]
                    / [1, 2, 4, 8][((clk_con3 >> 6) & 3) as usize]
            }
            source @ 5..=7 => pll * 2 / [4, 3, 2][(source - 5) as usize],
            8 => 192_000_000,
            _ => 480_000_000,
        }
    }
    pub(crate) fn timer_hz(&self, clk_con3: u32) -> u32 {
        let sys = self.system_hz(clk_con3);
        sys / (((self.system[1] >> 16) & 3) + 1) / (((self.system[1] >> 8) & 7) + 1)
    }
    pub(crate) fn cycles(&self) -> u64 {
        self.cycles
    }
    pub(crate) fn instruction_ticks(&mut self, clk_con3: u32) -> u32 {
        self.cycles += 1;
        let hz = match (self.issue_override, self.issue_clock) {
            (Some(hz), _) => hz.max(1),
            (None, Some((selector, hz))) if selector == clk_con3 => hz,
            _ => {
                let hz = self.system_hz(clk_con3).max(1);
                self.issue_clock = Some((clk_con3, hz));
                hz
            }
        };
        if hz != self.phase_hz {
            self.instruction_phase = self.instruction_phase * hz as u64 / self.phase_hz as u64;
            self.phase_hz = hz;
        }
        // One nominal CPU issue per step. Latencies/cache stalls are not
        // cycle accurate; an instruction is no longer one full OSC cycle.
        self.instruction_phase += 24_000_000;
        if self.instruction_phase < hz as u64 {
            return 0;
        }
        if self.instruction_phase < 2 * hz as u64 {
            self.instruction_phase -= hz as u64;
            return 1;
        }
        let ticks = self.instruction_phase / hz as u64;
        self.instruction_phase %= hz as u64;
        ticks as u32
    }
    fn register(&self, a: u32) -> Option<&u32> {
        match a {
            0x10200 => Some(&0x6f01), // Physical FM-1 chip revision, read-only.
            0x10000 => Some(&self.system[0]),
            0x10008 => Some(&self.system[1]),
            0x1000c => Some(&self.system[2]),
            0x10018 => Some(&self.system[3]),
            0x16a00..=0x16a14 if a.is_multiple_of(4) => {
                Some(&self.usb_phy[((a - 0x16a00) / 4) as usize])
            }
            0x119a0..=0x119ac if a.is_multiple_of(4) => {
                Some(&self.pll[((a - 0x119a0) / 4) as usize])
            }
            _ => None,
        }
    }
    pub fn read(&self, a: u32) -> Option<u32> {
        self.register(a).copied()
    }
    pub fn write(&mut self, a: u32, v: u32) -> Option<()> {
        let r = match a {
            0x10000 => &mut self.system[0],
            0x10008 => &mut self.system[1],
            0x1000c => &mut self.system[2],
            0x10018 => &mut self.system[3],
            0x16a00..=0x16a14 if a.is_multiple_of(4) => {
                &mut self.usb_phy[((a - 0x16a00) / 4) as usize]
            }
            0x119a0..=0x119ac if a.is_multiple_of(4) => &mut self.pll[((a - 0x119a0) / 4) as usize],
            _ => return None,
        };
        *r = v;
        self.issue_clock = None;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usb_phy_handoff_matches_the_fm1_995_capture() {
        let c = Clock::default();
        for (index, expected) in [0, 0x8881c3, 0, 0, 0x6003f, 0].into_iter().enumerate() {
            assert_eq!(c.read(0x16a00 + index as u32 * 4), Some(expected));
        }
        assert_eq!(c.read(0x16a18), None);
    }
    #[test]
    fn cpu_clock_retains_fractional_oscillator_time_when_clock_changes() {
        let mut c = Clock::default();
        for _ in 0..14 {
            assert_eq!(c.instruction_ticks(6), 0);
        }
        assert_eq!(c.instruction_ticks(6), 1);
        for _ in 0..4 {
            assert_eq!(c.instruction_ticks(8), 0);
        }
        // Half an oscillator tick is retained across 192 -> 360 MHz.
        for _ in 0..7 {
            assert_eq!(c.instruction_ticks(6), 0);
        }
        assert_eq!(c.instruction_ticks(6), 1);
    }
    #[test]
    fn changing_the_pll_updates_issue_time_without_changing_the_selector() {
        let mut c = Clock::default();
        for _ in 0..14 {
            assert_eq!(c.instruction_ticks(6), 0);
        }
        // PLL goes from 540 to 480 MHz; selector 6 now supplies 320 MHz.
        // Retain 14/15 of the old oscillator tick across the clock change.
        c.write(0x119ac, 238).unwrap();
        assert_eq!(c.instruction_ticks(6), 1);
        for _ in 0..13 {
            assert_eq!(c.instruction_ticks(6), 0);
        }
        assert_eq!(c.instruction_ticks(6), 1);
    }
    #[test]
    fn timer_clock_follows_pll_selection_and_both_bus_dividers() {
        let mut c = Clock::default();
        assert_eq!(c.timer_hz(6), 60_000_000);
        assert_eq!(c.timer_hz(5), 45_000_000);
        assert_eq!(c.timer_hz(7), 90_000_000);
        c.write(0x10008, 0).unwrap();
        assert_eq!(c.timer_hz(8), 192_000_000);
        assert_eq!(c.timer_hz(3 | 1 << 4 | 2 << 6), 40_000_000);
        c.write(0x10008, 1 << 16 | 3 << 8).unwrap();
        assert_eq!(c.timer_hz(6), 45_000_000);
    }
}
