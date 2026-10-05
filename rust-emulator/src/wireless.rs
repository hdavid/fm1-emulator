// SPDX-License-Identifier: GPL-3.0-only
// WL82.h: WL/WF analog configuration latches. Stock wfhw_init also configures
// two words at 0x30f00; their bit meanings are not published in that header.
// RF transmission, reception,
// LO frequency counting and serial-data DMA are not modeled. The PLL
// comparator uses measured nominal thresholds for this FM-1.
pub(crate) struct Wireless {
    registers: [u32; 26],
    radio_configuration: [u32; 2],
    mac: [u32; 10],
    bbp_command: u32,
    bbp: [u8; 256],
    analog: [u32; 31],
    sample_strobes: u8,
    sample_result: u32,
    filter_ticks: u32,
    filter_result: u8,
    bt_configuration: [u32; 29],
    bt_table: [u32; 128],
    bt_table_position: usize,
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
            sample_strobes: 0,
            sample_result: 0,
            filter_ticks: 0,
            filter_result: 0,
            bt_configuration: [0; 29],
            bt_table: [0; 128],
            bt_table_position: 0,
        }
    }
}
impl Wireless {
    // Vendor analog.c and the stock RF initialization routine. These are
    // configuration words; controller packet scheduling is still unsupported.
    fn bt_index(address: u32) -> Option<usize> {
        [
            0x20000, 0x2fc00, 0x2fc04, 0x2fc08, 0x2fc0c, 0x2fc10, 0x2fc14, 0x2fc18, 0x2fc1c,
            0x2fc20, 0x2fc24, 0x2fc28, 0x2fc40, 0x2fc48, 0x2fc70, 0x2fc78, 0x2fc7c, 0x2fc88,
            0x2fc98, 0x2fc9c, 0x2fca0, 0x2fd80, 0x2fd84, 0x2fd88, 0x2fd8c, 0x2fd90, 0x2fd94,
            0x2fd98, 0x2fd9c,
        ]
        .iter()
        .position(|a| *a == address)
    }
    pub(crate) fn advance(&mut self, ticks: u32) {
        self.filter_ticks = self.filter_ticks.saturating_sub(ticks);
    }
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
        if let Some(index) = Self::bt_index(address & !3) {
            return Some(if size != 4 {
                Err("wireless registers require word accesses")
            } else if address == 0x2fd9c {
                Err("Bluetooth RF table reads are not implemented")
            } else {
                Ok(self.bt_configuration[index])
            });
        }
        if (0x11900..=0x1197b).contains(&address) {
            return Some(if size == 4 {
                Ok(if address == 0x11978 {
                    self.sample_result
                } else {
                    self.analog[((address - 0x11900) / 4) as usize]
                })
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
        if let Some(index) = Self::bt_index(address & !3) {
            if size != 4 {
                return Some(Err("wireless registers require word accesses"));
            }
            if address == 0x2fd98 {
                if value != 0 {
                    return Some(Err("unsupported Bluetooth RF table position"));
                }
                self.bt_table_position = 0;
            }
            if address == 0x2fd9c {
                if self.bt_table_position == self.bt_table.len() {
                    return Some(Err("Bluetooth RF table write exceeds its capacity"));
                }
                self.bt_table[self.bt_table_position] = value;
                self.bt_table_position += 1;
            }
            self.bt_configuration[index] = value;
            return Some(Ok(()));
        }
        if (0x11900..=0x1197b).contains(&address) {
            return Some(if size == 4 {
                if address == 0x11978 {
                    match value {
                        0 => self.sample_strobes = 0,
                        1 => {
                            if self.analog[26] >> 28 != 1 {
                                return Some(Err(
                                    "wireless analog measurement mux is not implemented",
                                ));
                            }
                            self.sample_strobes = self.sample_strobes.saturating_add(1);
                            if self.sample_strobes >= 8 {
                                if self.analog[26] & 1 != 0 {
                                    self.sample_result = 0x80;
                                    if self.filter_ticks == 0 {
                                        self.sample_result |= 32 | (self.filter_result as u32) << 8;
                                    }
                                    return Some(Ok(()));
                                }
                                let cap = ((self.analog[14] >> 19) & 127) as usize;
                                let feedback = (self.analog[15] >> 5) & 255;
                                let (low, high) = PLL_THRESHOLDS[cap];
                                self.sample_result = 0x81;
                                if feedback < low as u32 {
                                    self.sample_result |= 1 << 17;
                                }
                                if feedback >= high as u32 {
                                    self.sample_result |= 1 << 18;
                                }
                            }
                        }
                        _ => return Some(Err("unsupported wireless analog sample strobe")),
                    }
                } else {
                    if address == 0x11968 && value & 1 != 0 && self.analog[26] & 1 == 0 {
                        let period = (value >> 1) & 511;
                        let window = (value >> 10) & 255;
                        if window != 30 || !matches!(period, 254 | 145 | 80) {
                            return Some(Err("unsupported wireless filter calibration settings"));
                        }
                        // A bounded calibration transaction, with nominal timing
                        // proportional to the requested reference window.
                        self.filter_ticks = window * period;
                        // FM-1_985's settled capture with stock analog settings:
                        // all three startup requests return sample 0x0000ffa0.
                        self.filter_result = 255;
                    }
                    self.analog[((address - 0x11900) / 4) as usize] = value;
                }
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

// Nominal comparator bounds measured on the connected FM-1 with FM-1_985.
// Index: seven-bit capacitor bank; values: eight-bit feedback divider at
// which LOW drops and HIGH rises. 256 means outside the divider range.
// The 16-bank discontinuities are measured hardware behavior. These are
// fixed device measurements, not an RF voltage/noise or temperature model.
const PLL_THRESHOLDS: [(u16, u16); 128] = [
    (256, 256),
    (256, 256),
    (256, 256),
    (256, 256),
    (256, 256),
    (256, 256),
    (256, 256),
    (256, 256),
    (251, 256),
    (243, 256),
    (235, 251),
    (228, 243),
    (220, 235),
    (212, 227),
    (205, 220),
    (197, 212),
    (256, 256),
    (253, 256),
    (245, 256),
    (237, 253),
    (230, 245),
    (222, 237),
    (214, 230),
    (207, 222),
    (199, 214),
    (192, 206),
    (184, 199),
    (177, 192),
    (170, 184),
    (162, 177),
    (155, 169),
    (148, 162),
    (208, 223),
    (201, 216),
    (193, 208),
    (186, 200),
    (178, 193),
    (171, 186),
    (164, 178),
    (157, 171),
    (150, 164),
    (143, 157),
    (136, 150),
    (129, 142),
    (122, 135),
    (115, 129),
    (108, 122),
    (102, 115),
    (159, 173),
    (151, 166),
    (144, 158),
    (137, 151),
    (131, 144),
    (124, 137),
    (117, 130),
    (110, 123),
    (103, 117),
    (97, 110),
    (90, 103),
    (84, 97),
    (77, 90),
    (71, 83),
    (64, 77),
    (58, 70),
    (110, 123),
    (103, 116),
    (96, 109),
    (90, 103),
    (83, 96),
    (77, 90),
    (70, 83),
    (64, 77),
    (58, 70),
    (51, 64),
    (45, 57),
    (39, 51),
    (33, 45),
    (27, 39),
    (21, 33),
    (15, 26),
    (65, 78),
    (59, 72),
    (53, 65),
    (47, 59),
    (40, 53),
    (34, 47),
    (28, 40),
    (22, 34),
    (16, 28),
    (10, 22),
    (4, 16),
    (0, 10),
    (0, 4),
    (0, 0),
    (0, 0),
    (0, 0),
    (23, 35),
    (17, 29),
    (11, 23),
    (5, 17),
    (0, 11),
    (0, 5),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
];

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(w: &mut Wireless) -> u32 {
        for _ in 0..8 {
            w.write(0x11978, 1, 4).unwrap().unwrap();
        }
        w.write(0x11978, 0, 4).unwrap().unwrap();
        w.read(0x11978, 4).unwrap().unwrap()
    }
    #[test]
    fn bluetooth_rf_table_retains_every_word_and_checks_its_capacity() {
        let mut w = Wireless::default();
        w.write(0x2fc40, 0xfcfc, 4).unwrap().unwrap();
        assert_eq!(w.read(0x2fc40, 4).unwrap().unwrap(), 0xfcfc);
        assert!(w.read(0x2fc44, 4).is_none());
        w.write(0x2fd98, 0, 4).unwrap().unwrap();
        for i in 0..128 {
            w.write(0x2fd9c, i ^ 0x13579bdf, 4).unwrap().unwrap();
        }
        for i in 0..128 {
            assert_eq!(w.bt_table[i], i as u32 ^ 0x13579bdf);
        }
        assert!(w.write(0x2fd9c, 0, 4).unwrap().is_err());
        assert!(w.read(0x2fd9c, 4).unwrap().is_err());
        w.write(0x2fd98, 0, 4).unwrap().unwrap();
        w.write(0x2fd9c, 7, 4).unwrap().unwrap();
        assert_eq!(w.bt_table[0], 7);
    }
    #[test]
    fn filter_calibration_completes_with_measured_results_and_can_restart() {
        let mut w = Wireless::default();
        for period in [254, 145, 80] {
            w.write(0x11968, 0, 4).unwrap().unwrap();
            w.write(0x11968, 0x10007801 | period << 1, 4)
                .unwrap()
                .unwrap();
            assert_eq!(sample(&mut w), 0x80);
            w.advance(30 * period - 1);
            assert_eq!(sample(&mut w), 0x80);
            w.advance(1);
            assert_eq!(sample(&mut w), 0xffa0);
        }
        w.write(0x11968, 0, 4).unwrap().unwrap();
        assert!(w.write(0x11968, 0x10007803, 4).unwrap().is_err());
        assert_eq!(w.read(0x11968, 4).unwrap().unwrap(), 0);
    }
}
