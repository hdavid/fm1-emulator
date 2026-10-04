// SPDX-License-Identifier: GPL-3.0-only
// WL82.h: WL/WF analog configuration latches. Stock wfhw_init also configures
// two words at 0x30f00; their bit meanings are not published in that header.
// RF transmission, reception,
// frequency calibration and serial-data DMA are not modeled.
pub(crate) struct Wireless {
    registers: [u32; 26],
    radio_configuration: [u32; 2],
    mac: [u32; 10],
    bbp_command: u32,
    bbp: [u8; 256],
    analog: [u32; 31],
}
impl Default for Wireless {
    fn default() -> Self {
        Self {
            registers: [0; 26],
            radio_configuration: [0; 2],
            mac: [0; 10],
            bbp_command: 0,
            bbp: [0; 256],
            analog: [0; 31],
        }
    }
}
impl Wireless {
    // Vendor wf_phy_mac_init/wl_hw_init setup words, reached through
    // wl30_mmc_io_rw_extended's direct MAC mapping (0x30000 + offset).
    fn mac_index(address: u32) -> Option<usize> {
        [
            0x30300, 0x30308, 0x3030c, 0x31004, 0x31100, 0x31104, 0x31330, 0x31334, 0x31338,
            0x31348,
        ]
        .iter()
        .position(|a| *a == address)
    }
    fn index(address: u32) -> Option<usize> {
        if address & 3 != 0 {
            return None;
        }
        match address {
            0x14000..=0x14034 | 0x14040..=0x14064 => Some(((address - 0x14000) / 4) as usize),
            _ => None,
        }
    }
    pub(crate) fn read(&self, address: u32, size: usize) -> Option<Result<u32, &'static str>> {
        if (0x11900..=0x1197b).contains(&address) {
            return Some(if size == 4 {
                Ok(self.analog[((address - 0x11900) / 4) as usize])
            } else {
                Err("wireless registers require word accesses")
            });
        }
        if address & !3 == 0x3101c || Self::mac_index(address & !3).is_some() {
            return Some(if size != 4 {
                Err("wireless registers require word accesses")
            } else {
                Ok(if address == 0x3101c {
                    self.bbp_command
                } else {
                    self.mac[Self::mac_index(address).unwrap()]
                })
            });
        }
        if matches!(address & !3, 0x30f00 | 0x30f04) {
            return Some(if size == 4 {
                Ok(self.radio_configuration[((address - 0x30f00) / 4) as usize])
            } else {
                Err("wireless registers require word accesses")
            });
        }
        let index = Self::index(address & !3)?;
        Some(if size == 4 {
            Ok(self.registers[index])
        } else {
            Err("wireless registers require word accesses")
        })
    }
    pub(crate) fn write(
        &mut self,
        address: u32,
        value: u32,
        size: usize,
    ) -> Option<Result<(), &'static str>> {
        if (0x11900..=0x1197b).contains(&address) {
            return Some(if size == 4 {
                self.analog[((address - 0x11900) / 4) as usize] = value;
                Ok(())
            } else {
                Err("wireless registers require word accesses")
            });
        }
        if address & !3 == 0x3101c || Self::mac_index(address & !3).is_some() {
            if size != 4 {
                return Some(Err("wireless registers require word accesses"));
            }
            if address == 0x3101c {
                // bbp_set/bbp_rd: bit 17 starts the byte transaction,
                // bit 16 selects read, bits 8..15 are the BBP register.
                let index = ((value >> 8) & 255) as usize;
                self.bbp_command = value & !(1 << 17);
                if value & (1 << 17) != 0 {
                    if value & (1 << 16) != 0 {
                        self.bbp_command = (self.bbp_command & !255) | self.bbp[index] as u32;
                    } else {
                        self.bbp[index] = value as u8;
                    }
                }
            } else {
                self.mac[Self::mac_index(address).unwrap()] = value;
            }
            return Some(Ok(()));
        }
        if matches!(address & !3, 0x30f00 | 0x30f04) {
            return Some(if size == 4 {
                self.radio_configuration[((address - 0x30f00) / 4) as usize] = value;
                Ok(())
            } else {
                Err("wireless registers require word accesses")
            });
        }
        let index = Self::index(address & !3)?;
        if size != 4 {
            return Some(Err("wireless registers require word accesses"));
        }
        if address == 0x14024 {
            return Some(Err("wireless frequency result is read-only"));
        }
        if address == 0x14020 && value != 0 {
            return Some(Err("wireless frequency calibration is not implemented"));
        }
        self.registers[index] = value;
        Some(Ok(()))
    }
}
