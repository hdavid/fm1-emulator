// SPDX-License-Identifier: GPL-3.0-only
// Probe decodings mirror emu.py, checked against vendor objdump and the FM-1.
// Startup-only additions use the pinned Quarkslab pi32v2 reference; see README.
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

#[cfg(test)]
mod lock_tests {
    use super::*;
    #[test]
    fn two_cores_share_elapsed_oscillator_time() {
        let mut c = Cpu::new(Bus::new(vec![0; 128]).unwrap(), crate::XIP);
        c.bus.write(0x10014, 6, 4).unwrap();
        c.bus.write(0x10808, u32::MAX, 4).unwrap();
        c.bus.write(0x10800, 9, 4).unwrap();
        c.bus.write(0x01c7fff8, crate::RAM + 512, 4).unwrap();
        c.bus.write(0x1eee004, 8, 4).unwrap();
        for _ in 0..30 {
            c.step().unwrap();
        }
        assert_eq!(c.steps, 60);
        assert_eq!(c.bus.read(0x10804, 4).unwrap(), 2);
    }
    #[test]
    fn lock_instructions_change_ownership_without_changing_registers() {
        let mut c = Cpu::new(Bus::new(vec![0x41, 0, 0x40, 0]).unwrap(), crate::XIP);
        let before = c.r;
        assert_eq!(c.step().unwrap(), "lockset");
        assert!(c.bus_locked);
        assert_eq!(c.step().unwrap(), "lockclr");
        assert!(!c.bus_locked);
        assert_eq!(c.r, before);
    }

    #[test]
    fn secondary_uses_the_guest_handoff_and_serializes_bus_locks() {
        let mut c = Cpu::new(Bus::new(vec![0; 32]).unwrap(), crate::XIP);
        let entry = crate::RAM + 512;
        c.bus.write(entry, 0x00400041, 4).unwrap(); // lockset; lockclr
        c.bus.write(0x01c7fff8, entry, 4).unwrap();
        c.bus.write(0x1eee004, 8, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.pc, crate::XIP + 2);
        let secondary = c.secondary.as_ref().unwrap();
        assert_eq!(secondary.pc, entry + 2);
        assert_eq!(secondary.sr[6], 1);
        assert!(secondary.bus_locked);
        c.step().unwrap(); // Secondary owns the bus until LOCKCLR.
        assert_eq!(c.pc, crate::XIP + 2);
        assert!(!c.secondary.as_ref().unwrap().bus_locked);
        c.step().unwrap();
        assert_eq!(c.pc, crate::XIP + 4);
        c.bus.write(0x1eee004, 2, 4).unwrap();
        c.step().unwrap();
        assert!(c.secondary.is_none());
    }

    #[test]
    fn software_interrupt_latches_are_shared_and_masks_are_per_core() {
        use crate::devices::IRQ_CONFIG;
        let mut c = Cpu::new(Bus::new(vec![0; 32]).unwrap(), crate::XIP);
        c.bus
            .write(IRQ_CONFIG + 0x200 + 15 * 4, 3 << 28, 4)
            .unwrap();
        c.bus.write(0x1eef3a0, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 0), None);
        assert_eq!(c.bus.pending_irq_for(0x100, 1), Some(127));
        assert_eq!(c.bus.read(0x1eef38c, 4).unwrap(), 0x80000000);
        c.bus.write(0x1eef3a4, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 1), None);
        c.bus.write(0x1eef1a0, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 1), Some(127));
        assert_eq!(c.bus.read(0x1eef18c, 4).unwrap(), 0);
        c.bus.write(IRQ_CONFIG + 15 * 4, 3 << 28, 4).unwrap();
        assert_eq!(c.bus.read(0x1eef18c, 4).unwrap(), 0x80000000);
        c.bus.write(0x1eef3a4, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 0), None);
    }

    #[test]
    fn paused_secondary_retains_context_until_resume() {
        let mut c = Cpu::new(Bus::new(vec![0; 32]).unwrap(), crate::XIP);
        let entry = crate::RAM + 512;
        c.bus.write(0x01c7fff8, entry, 4).unwrap();
        c.bus.write(0x1eee004, 8, 4).unwrap();
        c.step().unwrap();
        let paused_pc = c.secondary.as_ref().unwrap().pc;
        c.bus.write(0x1eee004, 12, 4).unwrap();
        assert_eq!(c.bus.read(0x1eee004, 4).unwrap() & 0x1c, 16);
        c.step().unwrap();
        c.step().unwrap();
        assert_eq!(c.secondary.as_ref().unwrap().pc, paused_pc);
        c.bus.write(0x1eee004, 24, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.secondary.as_ref().unwrap().pc, paused_pc + 2);
    }
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

#[derive(Clone, Copy)]
struct Repeat {
    start: u32,
    end: u32,
    iterations: u32,
    register: Option<(usize, u32)>,
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
    repeat: Option<Repeat>,
    irq_repeat: Option<Repeat>,
    bus_locked: bool,
    secondary: Option<Core>,
}

// Per-core context. Memory and devices remain on the one shared bus.
struct Core {
    r: [u32; 16],
    sr: [u32; 16],
    pc: u32,
    interrupts_enabled: bool,
    in_interrupt: bool,
    predicate_skip: Option<(u32, u32)>,
    irq_predicate: Option<(u32, u32)>,
    repeat: Option<Repeat>,
    irq_repeat: Option<Repeat>,
    bus_locked: bool,
}
impl Core {
    fn reset(pc: u32) -> Self {
        let mut sr = [0; 16];
        sr[6] = 1;
        Self {
            r: [0; 16],
            sr,
            pc,
            interrupts_enabled: false,
            // ROM's secondary handoff is in supervisor/interrupt context.
            in_interrupt: true,
            predicate_skip: None,
            irq_predicate: None,
            repeat: None,
            irq_repeat: None,
            bus_locked: false,
        }
    }
    fn swap(&mut self, cpu: &mut Cpu) {
        use std::mem::swap;
        swap(&mut self.r, &mut cpu.r);
        swap(&mut self.sr, &mut cpu.sr);
        swap(&mut self.pc, &mut cpu.pc);
        swap(&mut self.interrupts_enabled, &mut cpu.interrupts_enabled);
        swap(&mut self.in_interrupt, &mut cpu.in_interrupt);
        swap(&mut self.predicate_skip, &mut cpu.predicate_skip);
        swap(&mut self.irq_predicate, &mut cpu.irq_predicate);
        swap(&mut self.repeat, &mut cpu.repeat);
        swap(&mut self.irq_repeat, &mut cpu.irq_repeat);
        swap(&mut self.bus_locked, &mut cpu.bus_locked);
    }
}

impl Cpu {
    pub(crate) fn arithmetic(&mut self, left: u32, right: u32, subtract: bool, extra: u32) -> u32 {
        let (wide, signed_result, carry) = if subtract {
            let amount = right as u64 + extra as u64;
            (
                (left as u64).wrapping_sub(amount),
                left as i32 as i64 - right as i32 as i64 - extra as i64,
                left as u64 >= amount,
            )
        } else {
            let wide = left as u64 + right as u64 + extra as u64;
            (
                wide,
                left as i32 as i64 + right as i32 as i64 + extra as i64,
                wide > u32::MAX as u64,
            )
        };
        let result = wide as u32;
        let overflow = !(i32::MIN as i64..=i32::MAX as i64).contains(&signed_result);
        self.sr[5] = (self.sr[5] & !15)
            | overflow as u32
            | ((carry as u32) << 1)
            | (((result == 0) as u32) << 2)
            | ((result >> 31) << 3);
        result
    }
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
            repeat: None,
            irq_repeat: None,
            bus_locked: false,
            secondary: None,
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
        let control = self.read(0x1eee004, 4)?;
        if control & 2 != 0 {
            self.secondary = None;
        }
        if self.secondary.is_none() && control & 10 == 8 {
            // SPL's RAM handoff vector, written by the unchanged stock guest.
            let entry = self.read(0x01c7fff8, 4)?;
            self.bus
                .fetch(entry)
                .map_err(|fault| Fault::Access { pc: self.pc, fault })?;
            self.secondary = Some(Core::reset(entry));
        }
        let secondary_running = control & 0x18 == 8 && self.secondary.is_some();
        if secondary_running
            && (self.read(0x1eee000, 4)? & 16 != 0
                || self.secondary.as_ref().is_some_and(|core| core.bus_locked))
        {
            return self.step_secondary(true);
        }
        let op = self.step_core(true)?;
        if !self.bus_locked && secondary_running {
            self.step_secondary(false)?;
        }
        Ok(op)
    }

    fn step_secondary(&mut self, advance_time: bool) -> Result<&'static str, Fault> {
        let mut secondary = self.secondary.take().unwrap();
        secondary.swap(self);
        let result = self.step_core(advance_time);
        secondary.swap(self);
        self.secondary = Some(secondary);
        result
    }

    fn step_core(&mut self, advance_time: bool) -> Result<&'static str, Fault> {
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
        // FM-1_988: conditional bundles finish and skip their unselected
        // arm before a pending interrupt can enter.
        if let Some((at, end)) = self.predicate_skip {
            if self.pc == at {
                self.pc = end;
                self.predicate_skip = None;
            }
        }
        if let Some(mut repeat) = self.repeat {
            if self.pc == repeat.end {
                if let Some((register, count)) = repeat.register {
                    // Hardware writes back its captured counter after each
                    // iteration, even when the body overwrote that register.
                    self.r[register] = count - 1;
                    repeat.register = Some((register, count - 1));
                }
                if repeat.iterations > 1 {
                    self.pc = repeat.start;
                    repeat.iterations -= 1;
                    self.repeat = Some(repeat);
                } else {
                    self.repeat = None;
                }
            }
        }
        self.steps += 1;
        let ticks = if advance_time {
            self.bus.instruction_ticks()
        } else {
            0
        };
        if ticks != 0 {
            self.bus.advance_devices(ticks);
            self.bus.advance_nor(ticks);
            self.bus.advance_wireless(ticks);
            self.bus
                .system
                .advance(ticks)
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
                .advance_usb(ticks)
                .map_err(|fault| Fault::Access { pc, fault })?;
            self.bus
                .advance_audio(ticks)
                .map_err(|fault| Fault::Access { pc, fault })?;
        }
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
            let length = if matches!(h & 0xffe0, 0xffc0 | 0xffe0) || h == 0xff80 {
                6
            } else if h >> 13 == 7 {
                4
            } else {
                2
            };
            cursor += length;
            // A parallel pair counts as one conditional instruction bundle.
            if h >> 13 == 6 || h & 0xf800 == 0xf000 {
                let following = self.read(cursor, 2)?;
                cursor += if matches!(following & 0xffe0, 0xffc0 | 0xffe0) || following == 0xff80 {
                    6
                } else if following >> 13 == 7 {
                    4
                } else {
                    2
                };
            }
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
            } else if matches!(n, 0 | 12 | 13 | 14) {
                self.sr[n] = value;
                op = "stack_imm32";
            } else {
                return Err(Fault::Unsupported { pc, word: h as u16 });
            }
            next = pc + 6;
        } else if h & 0xff00 == 0x0300 {
            if self.repeat.is_some() {
                return Err(Fault::Unsupported { pc, word: h as u16 });
            }
            let register = (h & 15) as usize;
            let length = (((h >> 4) & 15) + 1) * 2;
            let count = self.r[register];
            if count == 0 {
                next = pc + 2 + length;
            } else {
                // Measured on FM-1: 31 executes 31 times; 32 executes once
                // leaving 31; 63 executes 32 times leaving 31. Compiler loops
                // reissue REP until the remaining register count reaches zero.
                let iterations = if count < 32 { count } else { (count & 31) + 1 };
                self.repeat = Some(Repeat {
                    start: pc + 2,
                    end: pc + 2 + length,
                    iterations,
                    register: Some((register, count)),
                });
            }
            op = "repeat_register";
        } else if h & 0xe00f == 0x8000 {
            if self.repeat.is_some() {
                return Err(Fault::Unsupported { pc, word: h as u16 });
            }
            let length = (((h >> 4) & 15) + 1) * 2;
            let count = ((h >> 8) & 31) + 1;
            self.repeat = Some(Repeat {
                start: pc + 2,
                end: pc + 2 + length,
                iterations: count,
                register: None,
            });
            op = "repeat_immediate";
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
                if special == 11 {
                    self.interrupts_enabled = self.sr[11] & 0x200 != 0;
                }
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
                self.r[a] = self.arithmetic(self.r[b], self.r[c], true, 0);
                op = "sub";
            } else {
                self.r[a] = self.arithmetic(self.r[b], self.r[c], false, 0);
                op = "add";
            }
        } else if h & 0xe0c0 == 0x20c0 {
            let imm = signed((((h >> 3) & 7) << 5) | ((h >> 8) & 31), 8);
            self.r[a] = self.arithmetic(self.r[a], imm as u32, false, 0);
            op = "add_imm8";
        } else if h & 0xe01f == 0x8002 {
            let imm = (signed((h >> 5) & 7, 3) << 7) | (((h >> 8) & 31) << 2) as i32;
            self.sr[14] = self.sr[14].wrapping_add(imm as u32);
            op = "add_sp";
        } else if h & 0xe088 == 0x8008 {
            self.r[a] = self.arithmetic(self.r[b], (h >> 8) & 31, false, 0);
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
        } else if h & 0xe088 == 0xa088 {
            self.r[a] = ((self.r[b] as i32) >> ((h >> 8) & 31)) as u32;
            op = "asr";
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
        } else if matches!(h, 0xe8d4 | 0xe8d5 | 0xe8d8 | 0xe8d9) {
            let mask = self.read(pc + 2, 2)?;
            next = pc + 4;
            if h & 4 == 0 {
                if h & 1 != 0 {
                    self.push(self.sr[3])?;
                }
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
                if h & 1 != 0 {
                    next = self.pop()?;
                }
                op = "pop_mask";
            }
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
        } else if h == 0x04c1 {
            self.push(self.sr[0])?;
            op = "push_reti";
        } else if matches!(h, 0x0481 | 0x0488) {
            self.sr[if h == 0x0481 { 0 } else { 3 }] = self.pop()?;
            op = "pop_return_register";
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
        } else if matches!(h, 0x1440..=0x1443) {
            match h {
                0x1440 => self.sr[14] = self.sr[12],
                0x1441 => self.sr[14] = self.sr[13],
                0x1442 => self.sr[12] = self.sr[14],
                _ => self.sr[13] = self.sr[14],
            }
            op = "move_stack_pointer";
        } else if matches!(h, 0x04e1 | 0x04e8 | 0x04e9) {
            self.push(self.sr[5])?;
            if h != 0x04e1 {
                self.push(self.sr[3])?;
            }
            if h != 0x04e8 {
                self.push(self.sr[0])?;
            }
            op = "push_irq_frame";
        } else if matches!(h, 0x04a1 | 0x04a8 | 0x04a9) {
            if h != 0x04a8 {
                self.sr[0] = self.pop()?;
            }
            if h != 0x04a1 {
                self.sr[3] = self.pop()?;
            }
            self.sr[5] = self.pop()?;
            op = "pop_irq_frame";
        } else if h == 0xff80 {
            // Vendor startup uses a signed byte displacement after a 6-byte call.
            let displacement = self.read(pc + 2, 2)? | (self.read(pc + 4, 2)? << 16);
            self.sr[3] = pc + 6;
            next = (pc + 6).wrapping_add(displacement);
            op = "call_rel32";
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
        } else if h & 0xfff0 == 0x00b0 {
            let address = self.r[(h & 15) as usize];
            let old = self.read(address, 1)?;
            self.bus
                .write(address, 0xff, 1)
                .map_err(|fault| Fault::Access { pc, fault })?;
            // FM-1_982 physical probe: the old byte's low nibble is copied
            // into the four PSR condition bits, not a comparison result.
            self.sr[5] = (self.sr[5] & !15) | (old & 15);
            op = "testset_byte";
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
            self.repeat = self.irq_repeat.take();
            self.interrupts_enabled = true;
            self.sr[11] = (self.sr[11] & !255) | 0x600;
            op = "rti";
        } else if h == 0x0060 {
            self.interrupts_enabled = false;
            self.sr[11] &= !0x200;
            op = "cli";
        } else if matches!(h, 0x0040 | 0x0041) {
            // CPU bus ownership latch. With one executing core acquisition
            // cannot contend; this does not replace the guest's memory locks.
            self.bus_locked = h == 0x0041;
            op = if self.bus_locked {
                "lockset"
            } else {
                "lockclr"
            };
        } else if h == 0x0061 {
            self.interrupts_enabled = true;
            self.sr[11] |= 0x200;
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
        if !self.interrupts_enabled
            || self.in_interrupt
            || self.repeat.is_some()
            || self.predicate_skip.is_some()
        {
            return Ok(());
        }
        if let Some(source) = self.bus.pending_irq_for(self.sr[11], self.sr[6] as usize) {
            let priority = self
                .bus
                .devices
                .irq_priority_for(source, self.sr[11], self.sr[6] as usize)
                .unwrap();
            let handler = self.read(0x01c7_fe00 + source as u32 * 4, 4)?;
            self.bus
                .fetch(handler)
                .map_err(|fault| Fault::Access { pc: self.pc, fault })?;
            self.sr[0] = self.pc;
            self.sr[12] = self.sr[14];
            self.sr[14] = self.sr[13];
            self.pc = handler;
            // FM-1_989: ICFG records source/priority above the active
            // priority bitmap. Entry preserves the global enable and clears
            // thread mode; INTPRI is a guest mask, not the active priority.
            self.sr[11] = (self.sr[11] & !0x077f04ff)
                | ((source as u32) << 16)
                | (priority << 24)
                | (1 << priority);
            self.in_interrupt = true;
            self.irq_predicate = self.predicate_skip.take();
            self.irq_repeat = self.repeat.take();
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
