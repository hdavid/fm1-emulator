// SPDX-License-Identifier: GPL-3.0-only
// USB0 register bridge and a small USB host for the CDC console.
use crate::RAM;
use std::collections::VecDeque;
#[derive(Default)]
pub struct Usb {
    regs: [u32; 16],
    io: u32,
    clock: u32,
    sie: [u8; 16],
    endpoints: [[u8; 8]; 4],
    index: usize,
    ticks: u64,
    deadline: u64,
    attached: bool,
    phase: usize,
    waiting: bool,
    response: Vec<u8>,
    cdc_interface: Option<u8>,
    cdc_endpoint: Option<usize>,
    pub serial: VecDeque<u8>,
    pub setups: u64,
    pub packets: u64,
}
impl Usb {
    pub fn read(&self, a: u32) -> Option<u32> {
        match a {
            0x51000 => Some(self.io),
            0x10010 => Some(self.clock),
            0x11800..=0x1183c if a.is_multiple_of(4) => {
                Some(self.regs[((a - 0x11800) / 4) as usize])
            }
            _ => None,
        }
    }
    fn dma(ram: &mut [u8], a: u32, n: usize) -> Result<&mut [u8], &'static str> {
        let start = a.checked_sub(RAM).ok_or("USB DMA must address SRAM")? as usize;
        ram.get_mut(start..start + n).ok_or("USB DMA exceeds SRAM")
    }
    fn complete(&mut self) {
        if self.phase == 2 {
            let mut i = 0;
            let mut cdc_data = false;
            while i + 2 <= self.response.len() {
                let n = self.response[i] as usize;
                if n < 2 || i + n > self.response.len() {
                    break;
                }
                let d = &self.response[i..i + n];
                if d[1] == 4 && n >= 9 {
                    if d[5] == 2 {
                        self.cdc_interface = Some(d[2]);
                    }
                    cdc_data = d[5] == 10;
                }
                if d[1] == 5 && n >= 7 && cdc_data && d[2] & 128 != 0 && d[3] & 3 == 2 {
                    self.cdc_endpoint = Some((d[2] & 15) as usize);
                }
                i += n;
            }
        }
        self.phase += 1;
        self.waiting = false;
        self.deadline = self.ticks + 24000;
    }
    fn send(&mut self, ep: usize, ram: &mut [u8]) -> Result<(), &'static str> {
        let n = self.regs[2 + ep] as usize;
        if n > 64 {
            return Err("USB full-speed packet exceeds 64 bytes");
        }
        let a = if ep == 0 {
            self.regs[6]
        } else {
            self.regs[7 + (ep - 1) * 2]
        };
        let bytes = Self::dma(ram, a, n)?;
        if ep == 0 {
            self.response.extend_from_slice(bytes);
        } else if Some(ep) == self.cdc_endpoint {
            self.serial.extend(bytes.iter().copied());
            self.packets += 1;
        }
        self.sie[2] |= 1 << ep;
        Ok(())
    }
    pub fn write(&mut self, a: u32, v: u32, ram: &mut [u8]) -> Option<Result<(), &'static str>> {
        self.read(a)?;
        Some(self.write_inner(a, v, ram))
    }
    fn write_inner(&mut self, a: u32, v: u32, ram: &mut [u8]) -> Result<(), &'static str> {
        match a {
            0x51000 => self.io = v,
            0x10010 => self.clock = v,
            0x11800 => {
                self.regs[0] = v & !0x1000;
                if v & 0x1000 != 0 {
                    self.regs[0] &= !0x2000;
                }
                if v & 4 == 0 {
                    self.attached = false;
                    self.phase = 0;
                    self.waiting = false;
                    self.sie = [0; 16];
                    self.endpoints = [[0; 8]; 4];
                }
            }
            0x11804 => {
                if self.regs[0] & 4 == 0 {
                    self.regs[1] = v;
                    return Ok(());
                }
                let r = ((v >> 8) & 0x3f) as usize;
                if r > 23 {
                    return Err("unimplemented USB SIE register");
                }
                let data = if v & 0x4000 != 0 {
                    let data = if r < 16 {
                        self.sie[r]
                    } else {
                        self.endpoints[self.index][r - 16]
                    };
                    if (2..=6).contains(&r) {
                        self.sie[r] = 0;
                    }
                    data
                } else {
                    let data = v as u8;
                    if r == 14 {
                        if data > 3 {
                            return Err("USB endpoint index exceeds modeled controller");
                        }
                        self.index = data as usize;
                        self.sie[r] = data;
                    } else if r < 16 {
                        self.sie[r] = data;
                    } else if r == 17 {
                        if self.index == 0 {
                            if data & 0x20 != 0 {
                                return Err("guest stalled host USB control request");
                            }
                            if data & 0x40 != 0 {
                                self.endpoints[0][1] &= !1;
                            }
                            if data & 2 != 0 {
                                self.send(0, ram)?;
                            }
                            if data & 8 != 0 && self.waiting {
                                self.complete();
                            }
                        } else {
                            self.endpoints[self.index][1] = data & !0xc9;
                            if data & 1 != 0 {
                                self.send(self.index, ram)?;
                            }
                        }
                    } else {
                        self.endpoints[self.index][r - 16] = data;
                    }
                    data
                };
                self.regs[1] = 0x8000 | data as u32;
            }
            _ => self.regs[((a - 0x11800) / 4) as usize] = v,
        }
        Ok(())
    }
    pub fn advance(&mut self, ticks: u32, ram: &mut [u8]) -> Result<(), &'static str> {
        self.ticks += ticks as u64;
        if self.regs[0] & 4 == 0 || self.io & 0x40 == 0 {
            return Ok(());
        }
        let frame = (self.ticks / 24000) as u16 & 2047;
        self.sie[12] = frame as u8;
        self.sie[13] = (frame >> 8) as u8;
        if self.ticks % 24000 < ticks as u64 {
            self.regs[0] |= 0x2000;
        }
        if !self.attached {
            self.attached = true;
            self.sie[6] |= 4;
            self.deadline = self.ticks + 120000;
        }
        if self.waiting || self.ticks < self.deadline || self.phase >= 5 {
            return Ok(());
        }
        let setup = match self.phase {
            0 => [0x80, 6, 0, 1, 0, 0, 18, 0],
            1 => [0, 5, 1, 0, 0, 0, 0, 0],
            2 => [0x80, 6, 0, 2, 0, 0, 255, 0],
            3 => [0, 9, 1, 0, 0, 0, 0, 0],
            _ => [
                0x21,
                0x22,
                1,
                0,
                self.cdc_interface
                    .ok_or("USB configuration has no CDC interface")?,
                0,
                0,
                0,
            ],
        };
        Self::dma(ram, self.regs[6], 8)?.copy_from_slice(&setup);
        self.response.clear();
        self.endpoints[0][1] = 1;
        self.endpoints[0][6] = 8;
        self.sie[2] |= 1;
        self.waiting = true;
        self.setups += 1;
        Ok(())
    }
}
