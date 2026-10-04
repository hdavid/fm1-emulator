// SPDX-License-Identifier: GPL-3.0-only
// Probe decodings mirror emu.py, checked against vendor objdump and the FM-1.
// Startup-only additions use the pinned Quarkslab pi32v2 reference; see README.
use crate::devices::OSC_TICKS_PER_INSTRUCTION;
use crate::{
    bus::{AccessFault, Bus},
    PROBE_RETURN, RESULT, USER_STACK,
};
use std::{fmt, io::Write};

#[derive(Debug, PartialEq, Eq)]
pub enum Fault {
    Access { pc: u32, fault: AccessFault },
    Unsupported { pc: u32, word: u16 },
    Limit { pc: u32, limit: u64 },
    Preservation,
    Trace(String),
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access { pc, fault } => write!(f, "at PC 0x{pc:08x}: {fault}"),
            Self::Unsupported { pc, word } => {
                write!(f, "unsupported instruction 0x{word:04x} at PC 0x{pc:08x}")
            }
            Self::Limit { pc, limit } => write!(f, "instruction limit {limit} at PC 0x{pc:08x}"),
            Self::Preservation => write!(f, "probe did not preserve registers or stack"),
            Self::Trace(message) => write!(f, "trace: {message}"),
        }
    }
}

pub fn signed(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

pub struct Cpu {
    pub bus: Bus,
    pub r: [u32; 16],
    pub sr: [u32; 16],
    pub pc: u32,
    pub steps: u64,
    pub interrupts_enabled: bool,
    pub irq_entries: u64,
    in_interrupt: bool,
    predicate_skip: Option<(u32, u32)>,
    irq_predicate: Option<(u32, u32)>,
}

impl Cpu {
    pub fn new(bus: Bus, entry: u32) -> Self {
        Self {
            bus,
            r: [0; 16],
            sr: [0; 16],
            pc: entry,
            steps: 0,
            interrupts_enabled: false,
            irq_entries: 0,
            in_interrupt: false,
            predicate_skip: None,
            irq_predicate: None,
        }
    }

    pub(crate) fn read(&self, address: u32, size: usize) -> Result<u32, Fault> {
        self.bus
            .read(address, size)
            .map_err(|fault| Fault::Access { pc: self.pc, fault })
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        self.bus
            .write(address, value, 4)
            .map_err(|fault| Fault::Access { pc: self.pc, fault })
    }

    fn push(&mut self, value: u32) -> Result<(), Fault> {
        let address = self.sr[14].wrapping_sub(4);
        self.write(address, value)?;
        self.sr[14] = address;
        Ok(())
    }

    fn pop(&mut self) -> Result<u32, Fault> {
        let value = self.read(self.sr[14], 4)?;
        self.sr[14] = self.sr[14].wrapping_add(4);
        Ok(value)
    }

    pub fn step(&mut self) -> Result<&'static str, Fault> {
        if let Some((at, end)) = self.predicate_skip {
            if self.pc == at {
                self.pc = end;
                self.predicate_skip = None;
            }
        }
        let pc = self.pc;
        let h = self
            .bus
            .fetch(pc)
            .map_err(|fault| Fault::Access { pc, fault })? as u32;
        let parallel = h >> 13 == 6 || h & 0xf800 == 0xf000;
        let op = if parallel {
            let length = if h >> 13 == 6 { 2 } else { 4 };
            let normalized = if length == 2 { h & 0x1fff } else { h & !0x1000 };
            self.pc = pc + length;
            let following = self.read(self.pc, 2)?;
            let before = self.r;
            let specials_before = self.sr;
            self.execute(following)?;
            let following_registers = self.r;
            let following_specials = self.sr;
            let continuation = self.pc;
            self.r = before;
            self.sr = specials_before;
            self.pc = pc;
            let op = self.execute(normalized)?;
            // Both slots read the incoming registers. Compiler bundles have
            // distinct destinations; retain writes from the following slot
            // where the primary slot did not change that register.
            for i in 0..16 {
                if self.r[i] == before[i] {
                    self.r[i] = following_registers[i];
                }
                if self.sr[i] == specials_before[i] {
                    self.sr[i] = following_specials[i];
                }
            }
            self.pc = continuation;
            op
        } else {
            self.execute(h)?
        };
        self.steps += 1;
        self.bus.devices.advance(OSC_TICKS_PER_INSTRUCTION);
        self.bus
            .system
            .advance(OSC_TICKS_PER_INSTRUCTION)
            .map_err(|reason| Fault::Access {
                pc,
                fault: AccessFault {
                    address: 0x13e08,
                    size: 4,
                    operation: "watchdog",
                    reason,
                },
            })?;
        self.bus
            .advance_usb(OSC_TICKS_PER_INSTRUCTION)
            .map_err(|fault| Fault::Access { pc, fault })?;
        self.dispatch_interrupt()?;
        Ok(op)
    }

    pub(crate) fn conditional(&mut self, test: bool, counts: u32) -> Result<u32, Fault> {
        let mut cursor = self.pc + 4;
        let mut then_end = cursor;
        let then_count = (counts >> 14) + 1;
        let else_count = (counts >> 12) & 3;
        for i in 0..then_count + else_count {
            let h = self.read(cursor, 2)?;
            cursor += if matches!(h & 0xffe0, 0xffc0 | 0xffe0) {
                6
            } else if h >> 13 == 7 {
                4
            } else {
                2
            };
            if i + 1 == then_count {
                then_end = cursor;
            }
        }
        if test {
            self.predicate_skip = Some((then_end, cursor));
            Ok(self.pc + 4)
        } else {
            Ok(then_end)
        }
    }

    fn execute(&mut self, h: u32) -> Result<&'static str, Fault> {
        let pc = self.pc;
        let a = (h & 7) as usize;
        let b = ((h >> 4) & 7) as usize;
        let mut next = pc.wrapping_add(2);
        let op;
        if matches!(h & 0xfff0, 0xffc0 | 0xffe0) {
            let value = self.read(pc + 2, 2)? | (self.read(pc + 4, 2)? << 16);
            let n = (h & 15) as usize;
            if h & 0xfff0 == 0xffc0 {
                self.r[n] = value;
                op = "mov_imm32";
            } else if n == 13 || n == 14 {
                self.sr[n] = value;
                op = "stack_imm32";
            } else {
                return Err(Fault::Unsupported { pc, word: h as u16 });
            }
            next = pc + 6;
        } else if h == 0xe064 {
            let extra = self.read(pc + 2, 2)?;
            let reg = ((extra >> 12) & 15) as usize;
            let special = ((extra >> 8) & 15) as usize;
            // Deliberately exclude PC writes and unrecognized reserved encodings.
            if special == 15 || !matches!(extra & 255, 0 | 128) {
                return Err(Fault::Unsupported { pc, word: h as u16 });
            }
            if extra & 255 == 128 {
                self.sr[special] = self.r[reg];
            } else {
                self.r[reg] = self.sr[special];
            }
            next = pc + 4;
            op = "mov_special";
        } else if h == 0xe060 {
            let extra = self.read(pc + 2, 2)?;
            let byte = extra & 255;
            let mode = (extra >> 10) & 3;
            let value = if mode != 0 {
                ((0x80 | (extra & 127)) << (32 - mode * 8)) >> ((extra >> 7) & 7)
            } else if extra & 0x0f00 == 0x0300 {
                byte * 0x0101_0101
            } else if extra & 0x0f00 == 0x0100 {
                (byte << 24) | (byte << 8)
            } else if extra & 0x0f00 == 0 {
                byte
            } else {
                return Err(Fault::Unsupported { pc, word: h as u16 });
            };
            self.r[((extra >> 12) & 15) as usize] = value;
            next = pc + 4;
            op = "mov_mask";
        } else if h & 0xfff0 == 0xe040 {
            self.r[(h & 15) as usize] = signed(self.read(pc + 2, 2)?, 16) as u32;
            next = pc + 4;
            op = "mov_imm16";
        } else if h & 0xe0c0 == 0x2040 {
            self.r[a] = (((h >> 3) & 7) << 5) | ((h >> 8) & 31);
            op = "mov_imm8";
        } else if h & 0xe0f8 == 0x2010 {
            self.r[a] = 0xffff_ffe0 | ((h >> 8) & 31);
            op = "mov_negative";
        } else if h & 0xff00 == 0x1600 {
            self.r[(h & 15) as usize] = self.r[((h >> 4) & 15) as usize];
            op = "mov_reg";
        } else if matches!(h & 0xfe00, 0x1c00 | 0x1e00) {
            let c = (((h >> 7) & 3) * 2 + ((h >> 3) & 1)) as usize;
            if h & 0xfe00 == 0x1e00 {
                self.r[a] = self.r[b].wrapping_sub(self.r[c]);
                op = "sub";
            } else {
                self.r[a] = self.r[b].wrapping_add(self.r[c]);
                op = "add";
            }
        } else if h & 0xe0c0 == 0x20c0 {
            let imm = signed((((h >> 3) & 7) << 5) | ((h >> 8) & 31), 8);
            self.r[a] = self.r[a].wrapping_add(imm as u32);
            op = "add_imm8";
        } else if h & 0xe01f == 0x8002 {
            let imm = (signed((h >> 5) & 7, 3) << 7) | (((h >> 8) & 31) << 2) as i32;
            self.sr[14] = self.sr[14].wrapping_add(imm as u32);
            op = "add_sp";
        } else if h & 0xe088 == 0x8008 {
            self.r[a] = self.r[b].wrapping_add((h >> 8) & 31);
            op = "add_small";
        } else if matches!(h & 0xff88, 0x1900 | 0x1908 | 0x1980 | 0x1988) {
            match h & 0xff88 {
                0x1900 => {
                    self.r[a] |= self.r[b];
                    op = "or";
                }
                0x1908 => {
                    self.r[a] ^= self.r[b];
                    op = "xor";
                }
                0x1980 => {
                    self.r[a] &= self.r[b];
                    op = "and";
                }
                _ => {
                    self.r[a] = !self.r[b];
                    op = "not";
                }
            }
        } else if h & 0xe008 == 0xa000 {
            let shift = (h >> 8) & 31;
            if h & 0x80 != 0 {
                self.r[a] = self.r[b] >> shift;
                op = "lsr";
            } else {
                self.r[a] = self.r[b] << shift;
                op = "lsl";
            }
        } else if h & 0xe008 == 0x6000 {
            let address = self.r[b].wrapping_add((signed((h >> 8) & 31, 5) * 4) as u32);
            if h & 0x80 != 0 {
                self.write(address, self.r[a])?;
                op = "store32";
            } else {
                self.r[a] = self.read(address, 4)?;
                op = "load32";
            }
        } else if h == 0xe8d8 || h == 0xe8d4 {
            let mask = self.read(pc + 2, 2)?;
            if h == 0xe8d8 {
                for n in (0..16).rev() {
                    if mask & (1 << n) != 0 {
                        self.push(self.r[n])?;
                    }
                }
                op = "push_mask";
            } else {
                for n in 0..16 {
                    if mask & (1 << n) != 0 {
                        self.r[n] = self.pop()?;
                    }
                }
                op = "pop_mask";
            }
            next = pc + 4;
        } else if h & 0xfff0 == 0x0460 {
            let boundary = (h & 15) as usize;
            let range = if boundary < 4 {
                boundary..=3
            } else {
                4..=boundary
            };
            for n in range.rev() {
                self.push(self.r[n])?;
            }
            op = "push_regs";
        } else if h == 0x0400 {
            next = self.pop()?;
            op = "pop_pc";
        } else if h == 0x0410 {
            self.push(self.sr[3])?;
            op = "push_rets";
        } else if h & 0xfff0 == 0x0440 {
            let boundary = (h & 15) as usize;
            let range = if boundary < 4 {
                boundary..=3
            } else {
                4..=boundary
            };
            for n in range {
                self.r[n] = self.pop()?;
            }
            op = "pop_regs";
        } else if h & 0xfff0 == 0x0470 && h & 15 >= 4 {
            self.push(self.sr[3])?;
            for n in (4..=(h & 15) as usize).rev() {
                self.push(self.r[n])?;
            }
            op = "push_rets_regs";
        } else if h & 0xfff0 == 0x0430 && h & 15 >= 4 {
            for n in 4..=(h & 15) as usize {
                self.r[n] = self.pop()?;
            }
            self.sr[3] = self.pop()?;
            op = "pop_rets_regs";
        } else if h & 0xfff0 == 0x0450 && h & 15 >= 4 {
            for n in 4..=(h & 15) as usize {
                self.r[n] = self.pop()?;
            }
            next = self.pop()?;
            op = "pop_pc_regs";
        } else if h == 0x04e9 {
            self.push(self.sr[5])?;
            self.push(self.sr[3])?;
            self.push(self.sr[0])?;
            op = "push_irq_frame";
        } else if h == 0x04a9 {
            self.sr[0] = self.pop()?;
            self.sr[3] = self.pop()?;
            self.sr[5] = self.pop()?;
            op = "pop_irq_frame";
        } else if matches!(h & 0xffc0, 0xea80 | 0xeac0) {
            let displacement = signed(((h & 63) << 16) | self.read(pc + 2, 2)?, 22) * 2;
            if h & 0xffc0 == 0xea80 {
                self.sr[3] = pc + 4;
                op = "call_rel22";
            } else {
                op = "goto_rel22";
            }
            next = (pc + 4).wrapping_add(displacement as u32);
        } else if h & 0xe08f == 0x8001 {
            // Vendor assembler's short relative call (signed 9-bit byte offset).
            let displacement = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9);
            self.sr[3] = pc + 2;
            next = (pc + 2).wrapping_add(displacement as u32);
            op = "call_rel9";
        } else if h & 0xe00c == 0x8004 {
            let displacement = signed(
                ((h & 3) << 10) | (((h >> 4) & 15) << 6) | (((h >> 8) & 31) << 1),
                12,
            );
            next = (pc + 2).wrapping_add(displacement as u32);
            op = "goto_rel12";
        } else if h & 0xe008 == 0x4000 {
            let displacement = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9);
            let nonzero = h & 0x80 != 0;
            if (self.r[a] != 0) == nonzero {
                next = (pc + 2).wrapping_add(displacement as u32);
            }
            op = if nonzero {
                "branch_nonzero"
            } else {
                "branch_zero"
            };
        } else if h == 0x0080 {
            next = self.sr[3];
            op = "return";
        } else if h & 0xfff0 == 0x00c0 {
            self.sr[3] = pc + 2;
            next = self.r[(h & 15) as usize];
            op = "call_reg";
        } else if h == 0x0081 && self.in_interrupt {
            next = self.sr[0];
            self.sr[13] = self.sr[14];
            self.sr[14] = self.sr[12];
            self.in_interrupt = false;
            self.predicate_skip = self.irq_predicate.take();
            self.interrupts_enabled = true;
            op = "rti";
        } else if h == 0x0060 {
            self.interrupts_enabled = false;
            op = "cli";
        } else if h == 0x0061 {
            self.interrupts_enabled = true;
            op = "sti";
        } else if h == 0x0020 || h == 0x0000 {
            op = if h == 0x0020 { "csync" } else { "nop" };
        } else if let Some((destination, name)) = crate::extended::execute(self, h, pc)? {
            next = destination;
            op = name;
        } else {
            return Err(Fault::Unsupported { pc, word: h as u16 });
        }
        self.pc = next;
        Ok(op)
    }

    fn dispatch_interrupt(&mut self) -> Result<(), Fault> {
        if !self.interrupts_enabled || self.in_interrupt {
            return Ok(());
        }
        if let Some(source) = self.bus.devices.pending_irq(self.sr[11]) {
            let handler = self.read(0x01c7_fe00 + source as u32 * 4, 4)?;
            self.bus
                .fetch(handler)
                .map_err(|fault| Fault::Access { pc: self.pc, fault })?;
            self.sr[0] = self.pc;
            self.sr[12] = self.sr[14];
            self.sr[14] = self.sr[13];
            self.pc = handler;
            self.in_interrupt = true;
            self.irq_predicate = self.predicate_skip.take();
            self.interrupts_enabled = false;
            self.irq_entries += 1;
        }
        Ok(())
    }

    pub fn run(
        &mut self,
        stop: Option<u32>,
        limit: u64,
        mut trace: Option<&mut dyn Write>,
    ) -> Result<(), Fault> {
        while stop != Some(self.pc) {
            if self.steps >= limit {
                return Err(Fault::Limit { pc: self.pc, limit });
            }
            let pc = self.pc;
            let op = self.step()?;
            if let Some(writer) = trace.as_mut() {
                writeln!(
                    writer,
                    "{{\"pc\":{pc},\"op\":\"{op}\",\"next_pc\":{},\"registers\":{:?},\"sp\":{}}}",
                    self.pc, self.r, self.sr[14]
                )
                .map_err(|error| Fault::Trace(error.to_string()))?;
            }
        }
        Ok(())
    }

    pub fn probe(&mut self, limit: u64, trace: Option<&mut dyn Write>) -> Result<[u32; 12], Fault> {
        self.r = std::array::from_fn(|n| 0x1020_3040 + n as u32 * 0x0101_0101);
        self.r[0] = RESULT;
        let before = self.r;
        self.sr[14] = USER_STACK;
        self.sr[3] = PROBE_RETURN;
        self.run(Some(PROBE_RETURN), limit, trace)?;
        if self.r[1..] != before[1..] || self.sr[14] != USER_STACK {
            return Err(Fault::Preservation);
        }
        let mut values = [0; 12];
        for (i, value) in values.iter_mut().enumerate() {
            *value = self.read(RESULT + i as u32 * 4, 4)?;
        }
        Ok(values)
    }
}
