// SPDX-License-Identifier: GPL-3.0-only
// Functional WL82 clock selectors/PLL configuration, not a PLL timing model.
// Addresses: vendor WL82.h. Reset handoff: physical FM-1_982 clocks probe.
pub(crate) struct Clock {
    system: [u32; 4],
    pll: [u32; 4],
    usb_phy: [u32; 6],
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            system: [0, 0x10200, 0x1c1, 2],
            pll: [0x45400203, 0x3f503026, 0x0940022b, 0x0750310c],
            // HUSB_COM_CON0..2 measured; HUSB_PHY0_CON0..2 (0x16a0c..14, SDK
            // WL82.h husb_phy_base) first touched by the stock app at
            // 0x020356a8. Their reset value is unmeasured; 0 is assumed.
            usb_phy: [0, 0x8881c3, 0, 0, 0, 0],
        }
    }
}
impl Clock {
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
        Some(())
    }
}
