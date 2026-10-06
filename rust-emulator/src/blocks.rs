// SPDX-License-Identifier: GPL-3.0-only
// Bounded basic blocks. Fetch and operand reads remain live: neither a changed
// SRAM instruction nor a changed SFC mapping can execute stale prepared code.
use crate::{
    bus::Bus,
    cpu::{signed, Cpu, Fault, Step},
    decode::{Cache as Decode, Extended, First, Wide},
    RAM, RAM_SIZE, XIP, XIP_END,
};

const SLOTS: usize = 2048;
const MAX_BLOCK: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Reg(pub u8); // 0..15 general registers; 16..31 special registers.
impl Reg {
    pub(crate) fn get(self, cpu: &Cpu) -> u32 {
        if self.0 < 16 {
            cpu.r[self.0 as usize]
        } else {
            cpu.sr[(self.0 - 16) as usize]
        }
    }
    fn set(self, cpu: &mut Cpu, value: u32) {
        if self.0 < 16 {
            cpu.r[self.0 as usize] = value;
        } else {
            cpu.sr[(self.0 - 16) as usize] = value;
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Value {
    Register(Reg),
    Immediate(u32),
}
impl Value {
    fn get(self, cpu: &Cpu) -> u32 {
        match self {
            Self::Register(r) => r.get(cpu),
            Self::Immediate(v) => v,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    Move(Reg, Value),
    Arithmetic(Reg, Value, Value, bool),
    Logic(Reg, Value, Value, u8), // OR, XOR, AND, AND-NOT.
    Shift(Reg, Reg, u8, u8),      // left, logical right, arithmetic right, rotate right.
    Multiply(Reg, Reg, Value),
    Extend(Reg, Reg, u8, bool),
    Memory(Reg, Reg, u32, u8, bool),
    Jump(u32, bool),
    BranchZero(Reg, u32, bool),
    Nop,
    Fallback,
}
#[derive(Clone, Copy)]
pub(crate) struct Instruction {
    pc: u32,
    words: [u16; 3],
    pub(crate) length: u8,
    pub(crate) op: Op,
    pub(crate) name: &'static str,
}
impl Instruction {
    fn ends_block(self) -> bool {
        matches!(self.op, Op::Jump(..) | Op::BranchZero(..) | Op::Fallback)
    }
    pub(crate) fn execute(self, cpu: &mut Cpu) -> Step<&'static str> {
        let mut next = self.pc.wrapping_add(self.length as u32);
        match self.op {
            Op::Move(d, v) => d.set(cpu, v.get(cpu)),
            Op::Arithmetic(d, l, r, sub) => {
                let value = cpu.arithmetic(l.get(cpu), r.get(cpu), sub, 0);
                d.set(cpu, value);
            }
            Op::Logic(d, l, r, kind) => {
                let (l, r) = (l.get(cpu), r.get(cpu));
                d.set(
                    cpu,
                    match kind {
                        0 => l | r,
                        1 => l ^ r,
                        2 => l & r,
                        _ => l & !r,
                    },
                );
            }
            Op::Shift(d, s, count, kind) => {
                let value = s.get(cpu);
                d.set(
                    cpu,
                    match kind {
                        0 => value.checked_shl(count as u32).unwrap_or(0),
                        1 => value.checked_shr(count as u32).unwrap_or(0),
                        2 => ((value as i32) >> count.min(31)) as u32,
                        _ => value.rotate_right(count as u32),
                    },
                );
            }
            Op::Multiply(d, s, v) => d.set(cpu, s.get(cpu).wrapping_mul(v.get(cpu))),
            Op::Extend(d, s, bits, sign) => d.set(
                cpu,
                if sign {
                    signed(s.get(cpu), bits as u32) as u32
                } else {
                    s.get(cpu) & ((1 << bits) - 1)
                },
            ),
            Op::Memory(r, base, offset, size, store) => {
                let address = base.get(cpu).wrapping_add(offset);
                if store {
                    cpu.bus
                        .write(address, r.get(cpu), size as usize)
                        .map_err(|fault| Fault::Access { pc: self.pc, fault })?;
                } else {
                    r.set(cpu, cpu.read(address, size as usize)?);
                }
            }
            Op::Jump(target, link) => {
                if link {
                    cpu.sr[3] = next;
                }
                next = target;
            }
            Op::BranchZero(r, target, nonzero) => {
                if (r.get(cpu) != 0) == nonzero {
                    next = target;
                }
            }
            Op::Nop => {}
            Op::Fallback => unreachable!("fallbacks use the original interpreter"),
        }
        cpu.pc = next;
        Ok(self.name)
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Cursor {
    slot: u16,
    index: u8,
    start: u32,
    next: u32,
}
struct Block {
    start: u32,
    instructions: Vec<Instruction>,
    visits: u16,
    attempted: bool,
    native: Option<crate::jit::Code>,
}
pub(crate) struct Prepared<'a> {
    pub(crate) instruction: Instruction,
    pub(crate) native: Option<crate::jit::Entry<'a>>,
}
pub(crate) struct Cache {
    slots: Box<[Option<Block>]>,
    /// Off by default (`Cpu::set_block_cache`): on the measured firmware
    /// the per-instruction lookups cost more than the prepared forms save.
    pub(crate) enabled: bool,
    #[cfg(test)]
    pub(crate) jit_enabled: bool,
    #[cfg(test)]
    pub(crate) native_calls: u64,
}
impl Cache {
    pub(crate) fn new() -> Self {
        Self {
            slots: std::iter::repeat_with(|| None).take(SLOTS).collect(),
            enabled: false,
            #[cfg(test)]
            jit_enabled: true,
            #[cfg(test)]
            native_calls: 0,
        }
    }
    #[inline]
    pub(crate) fn can_execute(&self, decode: &Decode, cursor: Cursor, pc: u32, h: u16) -> bool {
        cursor.next == pc || candidate(decode.first(h as u32), h)
    }
    pub(crate) fn instruction(
        &mut self,
        bus: &Bus,
        decode: &mut Decode,
        cursor: &mut Cursor,
        pc: u32,
        h: u16,
    ) -> Option<Prepared<'_>> {
        if !self.enabled {
            return None;
        }
        // A core's sequential cursor avoids hashing and family decoding on
        // visits within a block. Complex forms need neither a block nor a JIT.
        let hit = if cursor.next == pc {
            self.slots[cursor.slot as usize]
                .as_ref()
                .filter(|b| b.start == cursor.start)
                .and_then(|b| b.instructions.get(cursor.index as usize))
                .filter(|i| i.pc == pc && i.words[0] == h)
                .copied()
        } else {
            None
        };
        if hit.is_none() && !candidate(decode.first(h as u32), h) {
            return None;
        }
        let instruction = if let Some(i) = hit {
            i
        } else {
            let slot = ((pc >> 1).wrapping_mul(0x9e3779b9) >> 21) as usize;
            let valid = self.slots[slot]
                .as_ref()
                .is_some_and(|b| b.start == pc && b.instructions[0].words[0] == h);
            if !valid {
                self.slots[slot] = Some(build(bus, decode, pc, h));
            }
            *cursor = Cursor {
                slot: slot as u16,
                index: 0,
                start: pc,
                next: pc,
            };
            self.slots[slot].as_ref().unwrap().instructions[0]
        };
        // Only validate the instruction about to execute. A future fetch fault
        // must not be raised early, or cause speculative MMIO reads.
        for n in 1..instruction.length as usize / 2 {
            if bus.read(pc + n as u32 * 2, 2).ok()? != instruction.words[n] as u32 {
                self.slots[cursor.slot as usize] = None;
                return self.instruction(bus, decode, cursor, pc, h);
            }
        }
        let block = self.slots[cursor.slot as usize].as_mut().unwrap();
        let use_jit = {
            #[cfg(test)]
            {
                self.jit_enabled
            }
            #[cfg(not(test))]
            {
                true
            }
        };
        if !block.attempted && use_jit {
            block.visits += 1;
            if block.visits >= 32 {
                block.attempted = true;
                block.native = crate::jit::Code::compile(block.instructions.iter().map(|i| i.op));
            }
        }
        let native = block
            .native
            .as_ref()
            .and_then(|code| code.entry(cursor.index as usize));
        #[cfg(test)]
        if native.is_some() {
            self.native_calls += 1;
        }
        cursor.index += 1;
        cursor.next = if instruction.ends_block() {
            0
        } else {
            pc.wrapping_add(instruction.length as u32)
        };
        (!matches!(instruction.op, Op::Fallback)).then_some(Prepared {
            instruction,
            native,
        })
    }
}
fn candidate(kind: First, h: u16) -> bool {
    // Small loads, moves, logic and branches are already cheap in the original
    // interpreter. Prioritize operations with literal/flag decoding to amortize
    // lookup and native-call costs; blocks still prepare the intervening forms.
    match kind {
        First::MoveImmediate32
        | First::MovImm16
        | First::Arithmetic
        | First::AddImm8
        | First::AddSmall => true,
        First::Extended(Extended::AddRegister) => true,
        First::Extended(Extended::Wide) => {
            h & 0xfff0 == 0xe1e0 || matches!(h, 0xe0b4 | 0xe190 | 0xe1c0 | 0xe1c4)
        }
        _ => false,
    }
}

fn code_word(bus: &Bus, pc: u32) -> Option<u16> {
    // Reading ahead is permitted only in memory, never device register space.
    if (RAM..RAM + RAM_SIZE as u32).contains(&pc) || (XIP..XIP_END).contains(&pc) {
        bus.fetch(pc).ok()
    } else {
        None
    }
}
fn build(bus: &Bus, decode: &mut Decode, start: u32, first: u16) -> Block {
    let mut instructions = Vec::with_capacity(MAX_BLOCK);
    let mut pc = start;
    let mut h = first;
    loop {
        let i = prepare(bus, decode, pc, h).unwrap_or(Instruction {
            pc,
            words: [h, 0, 0],
            length: 2,
            op: Op::Fallback,
            name: "fallback",
        });
        instructions.push(i);
        if i.ends_block() || instructions.len() == MAX_BLOCK {
            break;
        }
        pc = pc.wrapping_add(i.length as u32);
        let Some(next) = code_word(bus, pc) else {
            break;
        };
        h = next;
    }
    Block {
        start,
        instructions,
        visits: 0,
        attempted: false,
        native: None,
    }
}
fn prepare(bus: &Bus, decode: &mut Decode, pc: u32, word: u16) -> Option<Instruction> {
    let h = word as u32;
    // Parallel bundles retain their original simultaneous-source semantics.
    if h >> 13 == 6 || h & 0xf800 == 0xf000 {
        return None;
    }
    let mut i = Instruction {
        pc,
        words: [word, 0, 0],
        length: 2,
        op: Op::Fallback,
        name: "",
    };
    let a = Reg((h & 7) as u8);
    let b = Reg(((h >> 4) & 7) as u8);
    let n = Reg((h & 15) as u8);
    use Value::{Immediate as Imm, Register as R};
    let (op, name) = match decode.first(h) {
        First::MoveImmediate32 => {
            i.words[1] = code_word(bus, pc + 2)?;
            i.words[2] = code_word(bus, pc + 4)?;
            i.length = 6;
            let value = i.words[1] as u32 | ((i.words[2] as u32) << 16);
            if h & 0xfff0 == 0xffc0 {
                (Op::Move(n, Imm(value)), "mov_imm32")
            } else if matches!(n.0, 0 | 12 | 13 | 14) {
                (Op::Move(Reg(n.0 + 16), Imm(value)), "stack_imm32")
            } else {
                return None;
            }
        }
        First::MovImm16 => {
            i.words[1] = code_word(bus, pc + 2)?;
            i.length = 4;
            (
                Op::Move(n, Imm(signed(i.words[1] as u32, 16) as u32)),
                "mov_imm16",
            )
        }
        First::MovImm8 => (
            Op::Move(a, Imm((((h >> 3) & 7) << 5) | ((h >> 8) & 31))),
            "mov_imm8",
        ),
        First::MovNegative => (
            Op::Move(a, Imm(0xffffffe0 | ((h >> 8) & 31))),
            "mov_negative",
        ),
        First::MovReg => (Op::Move(n, R(Reg(((h >> 4) & 15) as u8))), "mov_reg"),
        First::Arithmetic => {
            let c = Reg((((h >> 7) & 3) * 2 + ((h >> 3) & 1)) as u8);
            let sub = h & 0xfe00 == 0x1e00;
            (
                Op::Arithmetic(a, R(b), R(c), sub),
                if sub { "sub" } else { "add" },
            )
        }
        First::AddImm8 => (
            Op::Arithmetic(
                a,
                R(a),
                Imm(signed((((h >> 3) & 7) << 5) | ((h >> 8) & 31), 8) as u32),
                false,
            ),
            "add_imm8",
        ),
        First::AddSmall => (
            Op::Arithmetic(a, R(b), Imm((h >> 8) & 31), false),
            "add_small",
        ),
        First::Logic => {
            let kind = match h & 0xff88 {
                0x1900 => 0,
                0x1908 => 1,
                0x1980 => 2,
                _ => 3,
            };
            // NOT is encoded as all ones AND-NOT source.
            (
                Op::Logic(a, if kind == 3 { Imm(u32::MAX) } else { R(a) }, R(b), kind),
                ["or", "xor", "and", "not"][kind as usize],
            )
        }
        First::Asr => (Op::Shift(a, b, ((h >> 8) & 31) as u8, 2), "asr"),
        First::ShiftImmediate => (
            Op::Shift(a, b, ((h >> 8) & 31) as u8, (h & 0x80 != 0) as u8),
            if h & 0x80 != 0 { "lsr" } else { "lsl" },
        ),
        First::MemoryWord => (
            Op::Memory(
                a,
                b,
                (signed((h >> 8) & 31, 5) * 4) as u32,
                4,
                h & 0x80 != 0,
            ),
            if h & 0x80 != 0 { "store32" } else { "load32" },
        ),
        First::GotoRel12 => {
            let delta = signed(
                ((h & 3) << 10) | (((h >> 4) & 15) << 6) | (((h >> 8) & 31) << 1),
                12,
            );
            (
                Op::Jump((pc + 2).wrapping_add(delta as u32), false),
                "goto_rel12",
            )
        }
        First::CallRel9 => {
            let delta = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9);
            (
                Op::Jump((pc + 2).wrapping_add(delta as u32), true),
                "call_rel9",
            )
        }
        First::BranchZero => {
            let delta = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9);
            let nonzero = h & 0x80 != 0;
            (
                Op::BranchZero(a, (pc + 2).wrapping_add(delta as u32), nonzero),
                if nonzero {
                    "branch_nonzero"
                } else {
                    "branch_zero"
                },
            )
        }
        First::SyncOrNop => (Op::Nop, if h == 0 { "nop" } else { "csync" }),
        First::Extended(Extended::Multiply) => (
            Op::Multiply(n, n, R(Reg(((h >> 4) & 15) as u8))),
            "multiply",
        ),
        First::Extended(Extended::AddRegister) => (
            Op::Arithmetic(n, R(n), R(Reg(((h >> 4) & 15) as u8)), false),
            "add_register",
        ),
        First::Extended(Extended::ClearHighRegister) => {
            (Op::Move(Reg(8 + a.0), Imm(0)), "clear_high_register")
        }
        First::Extended(Extended::Extend) => (
            Op::Extend(a, b, if h & 0x80 == 0 { 8 } else { 16 }, h & 8 != 0),
            "extend",
        ),
        First::Extended(Extended::StackWord) => (
            Op::Memory(
                n,
                Reg(30),
                (((h >> 8) & 31) | (h & 32)) * 4,
                4,
                h & 128 != 0,
            ),
            "stack_word",
        ),
        First::Extended(Extended::MemorySmall) => {
            let size = if h & 0x2000 == 0 { 1 } else { 2 };
            (
                Op::Memory(
                    a,
                    b,
                    (signed((h >> 8) & 31, 5) * size) as u32,
                    size as u8,
                    h & 0x80 != 0,
                ),
                "memory_small",
            )
        }
        First::Extended(Extended::Wide) => {
            i.words[1] = code_word(bus, pc + 2)?;
            i.length = 4;
            let x = i.words[1] as u32;
            let d = Reg((x >> 12) as u8);
            let s = Reg(((x >> 4) & 15) as u8);
            let c = Reg(((x >> 8) & 15) as u8);
            match decode.wide(h, x) {
                Wide::MultiplyImmediate => (
                    Op::Multiply(n, d, Imm(crate::extended::packed(x))),
                    "multiply_immediate",
                ),
                Wide::ArithmeticRegister if h == 0xe0b4 => (
                    Op::Arithmetic(d, R(s), R(c), x & 2 != 0),
                    if x & 2 != 0 {
                        "subtract_extended"
                    } else {
                        "add_extended"
                    },
                ),
                Wide::LogicThree => (Op::Logic(d, R(s), R(c), (x & 3) as u8), "logic_three"),
                Wide::ShiftExtended => (
                    Op::Shift(
                        d,
                        s,
                        (((x >> 8) & 3) * 16 + (x & 15)) as u8,
                        match (x >> 10) & 3 {
                            0 => 0,
                            2 => 1,
                            _ => 2,
                        },
                    ),
                    "shift_extended",
                ),
                Wide::RotateRightImmediate => (
                    Op::Shift(d, s, (((x >> 8) & 1) * 16 + (x & 15)) as u8, 3),
                    "rotate_right_immediate",
                ),
                _ => return None,
            }
        }
        _ => return None,
    };
    i.op = op;
    i.name = name;
    Some(i)
}
