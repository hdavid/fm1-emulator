// SPDX-License-Identifier: GPL-3.0-only
// Compiler-emitted forms, decoded against vendor disassembly and the pinned
// Quarkslab pi32v2 SLEIGH reference. Unknown/reserved forms still fault.
// Classification is in decode.rs; this file executes the classified forms.
use crate::code_cache::Operands;
use crate::cpu::{signed, Cpu, Fault};
use crate::decode::Op;

/// A deferred load or store: (register, base, address, size, store,
/// sign-extend, updated base).
type Memory = (usize, usize, u32, usize, bool, bool, Option<u32>);

pub(crate) fn packed(x: u32) -> u32 {
    match (x >> 10) & 3 {
        0 => match x & 0x300 {
            0x300 => (x & 255) * 0x01010101,
            0x100 => ((x & 255) << 24) | ((x & 255) << 8),
            _ => x & 255,
        },
        mode => ((0x80 | (x & 127)) << (32 - mode * 8)) >> ((x >> 7) & 7),
    }
}

fn unsupported(pc: u32, h: u32) -> Fault {
    Fault::Unsupported { pc, word: h as u16 }
}

/// Execute an extended.rs form; returns the next PC and the form's name.
pub(crate) fn execute(
    cpu: &mut Cpu,
    op: Op,
    h: u32,
    pc: u32,
    code: Operands,
) -> Result<(u32, &'static str), Fault> {
    let a = (h & 7) as usize;
    let b = ((h >> 4) & 7) as usize;
    let mut mem = None;
    let name;
    match op {
        Op::MoveRegisterPair => {
            let destination = (h & 14) as usize;
            let source = ((h >> 4) & 14) as usize;
            let values = [cpu.r[source], cpu.r[source + 1]];
            cpu.r[destination..destination + 2].copy_from_slice(&values);
            name = "move_register_pair";
        }
        Op::Multiply => {
            let n = (h & 15) as usize;
            cpu.r[n] = cpu.r[n].wrapping_mul(cpu.r[((h >> 4) & 15) as usize]);
            name = "multiply";
        }
        Op::ClearPair => {
            let destination = (h & 14) as usize;
            cpu.r[destination] = 0;
            cpu.r[destination + 1] = 0;
            name = "clear_pair";
        }
        Op::AddRegister => {
            let n = (h & 15) as usize;
            cpu.r[n] = cpu.r[n].wrapping_add(cpu.r[((h >> 4) & 15) as usize]);
            name = "add_register";
        }
        Op::ClearHighRegister => {
            cpu.r[8 + a] = 0;
            name = "clear_high_register";
        }
        Op::CacheFlushInvalidate => {
            name = "cache_flush_invalidate";
        }
        Op::StackWord => {
            mem = Some((
                a,
                0,
                // Bit 5 supplies offset bit 7; Felucca spills beyond 128 bytes.
                cpu.sr[14].wrapping_add((((h >> 8) & 31) | (h & 32)) * 4),
                4,
                h & 128 != 0,
                false,
                None,
            ));
            name = "stack_word";
        }
        Op::Extend => {
            let bits = if h & 0x80 == 0 { 8 } else { 16 };
            cpu.r[a] = if h & 8 != 0 {
                signed(cpu.r[b], bits) as u32
            } else {
                cpu.r[b] & ((1 << bits) - 1)
            };
            name = "extend";
        }
        Op::BitRegister => {
            let bit = 1 << ((h >> 8) & 31);
            match h & 0xf8 {
                0x30 => cpu.r[a] |= bit,
                0x38 => cpu.r[a] ^= bit,
                _ => cpu.r[a] &= !bit,
            }
            name = "bit_register";
        }
        Op::ShiftRegister => {
            let shift = cpu.r[b];
            cpu.r[a] = if shift >= 32 {
                if h & 0x88 == 0x88 && (cpu.r[a] as i32) < 0 {
                    u32::MAX
                } else {
                    0
                }
            } else {
                match h & 0x88 {
                    0 => cpu.r[a] << shift,
                    0x80 => cpu.r[a] >> shift,
                    _ => ((cpu.r[a] as i32) >> shift) as u32,
                }
            };
            name = "shift_register";
        }
        Op::AddStack => {
            cpu.r[a] = cpu.sr[14].wrapping_add((((h >> 5) & 3) << 5) | ((h >> 8) & 31));
            name = "add_stack";
        }
        Op::GotoRegister => {
            return Ok((cpu.r[(h & 15) as usize], "goto_register"));
        }
        Op::Ssync => {
            name = "ssync";
        }
        Op::TableBranch => {
            let size = if h & 0x10 == 0 { 1 } else { 2 };
            let next = (pc + 2)
                .wrapping_add(cpu.read((pc + 2).wrapping_add(cpu.r[(h & 15) as usize]), size)? * 2);
            return Ok((next, "table_branch"));
        }
        Op::MemorySmall => {
            let size = if h & 0x2000 == 0 { 1 } else { 2 };
            let address = cpu.r[b].wrapping_add((signed((h >> 8) & 31, 5) * size as i32) as u32);
            mem = Some((a, b, address, size, h & 0x80 != 0, false, None));
            name = "memory_small";
        }
        Op::MemoryPostincrement => {
            let kind = (h >> 7) & 7;
            let size = match kind {
                2 | 3 => 4,
                4 | 5 => 2,
                _ => 1,
            };
            let address = cpu.r[b];
            let delta = if h & 8 == 0 {
                size as i32
            } else {
                -(size as i32)
            };
            mem = Some((
                a,
                b,
                address,
                size,
                kind & 1 != 0,
                false,
                Some(address.wrapping_add(delta as u32)),
            ));
            name = "memory_postincrement";
        }
        _ => return execute_wide(cpu, op, h, pc, code),
    }
    access(cpu, pc, mem)?;
    Ok((pc + 2, name))
}

/// Perform a deferred load or store and its base-register update.
fn access(cpu: &mut Cpu, pc: u32, mem: Option<Memory>) -> Result<(), Fault> {
    if let Some((reg, base, address, size, store, sign, updated)) = mem {
        if store {
            cpu.bus
                .write(address, cpu.r[reg], size)
                .map_err(|fault| Fault::Access { pc, fault })?;
        } else {
            let value = cpu.read(address, size)?;
            cpu.r[reg] = if sign {
                signed(value, (size * 8) as u32) as u32
            } else {
                value
            };
        }
        if let Some(value) = updated {
            cpu.r[base] = value;
        }
    }
    Ok(())
}

/// Execute a 32- or 48-bit form; the extension halfword is already known.
fn execute_wide(
    cpu: &mut Cpu,
    op: Op,
    h: u32,
    pc: u32,
    code: Operands,
) -> Result<(u32, &'static str), Fault> {
    let x = cpu.operand(code.x, pc + 2)?;
    let n = (h & 15) as usize;
    let d = (x >> 12) as usize;
    let s = ((x >> 4) & 15) as usize;
    let c = ((x >> 8) & 15) as usize;
    let mut next = pc + 4;
    let mut mem = None;
    let name;
    match op {
        Op::BranchRegisterMask => {
            let test = (cpu.r[n] & cpu.r[((h >> 4) & 15) as usize] != 0) == (h & 0x100 != 0);
            if test {
                next = next.wrapping_add((signed(x, 16) * 2) as u32);
            }
            name = "branch_register_mask";
        }
        Op::MemoryShift => {
            // [rD + off] shifted by an immediate: c plus h bit 0 as bit 4
            // (btctrler 0x0205d304: e86d 1607 = [r1+4] >>= 22, arithmetic,
            // sign-extending a 10-bit field built at bits 22-31). Mode in x
            // bits 0-1: 0 left, 2 logical right, 3 arithmetic right.
            let address = cpu.r[d].wrapping_add(x & 252);
            let shift = ((h & 1) << 4) | c as u32;
            let value = cpu.read(address, 4)?;
            cpu.write(
                address,
                match x & 3 {
                    0 => value << shift,
                    2 => value >> shift,
                    _ => ((value as i32) >> shift) as u32,
                },
            )?;
            name = match x & 3 {
                0 => "memory_shift_left",
                2 => "memory_shift_right",
                _ => "memory_shift_arithmetic",
            };
        }
        Op::MemoryAddRegister => {
            let addr = cpu.r[d] + (x & 252);
            cpu.write(addr, cpu.read(addr, 4)?.wrapping_add(cpu.r[c]))?;
            name = "memory_add_register";
        }
        Op::HalfwordExtended => {
            let store = x & 1 != 0;
            let high = if store {
                signed(h & 7, 3)
            } else {
                (h & 1) as i32
            };
            let offset = (high << 8) | (((x >> 8) & 15) << 4) as i32 | (x & 14) as i32;
            let addr = cpu.r[s].wrapping_add(offset as u32);
            mem = Some((
                d,
                s,
                addr,
                2,
                store,
                !store && h & 4 != 0,
                if h & 8 != 0 { Some(addr) } else { None },
            ));
            name = "halfword_extended";
        }
        Op::HalfwordPostincrement => {
            // x bit 0 selects the store (as in the edd8 register forms); the
            // increment is even.
            let store = x & 1 != 0;
            let increment = ((x >> 8) & 15) * 16 + (x & 14);
            let address = cpu.r[s];
            mem = Some((
                d,
                s,
                address,
                2,
                store,
                !store && h & 4 != 0,
                Some(address.wrapping_add(increment)),
            ));
            name = "halfword_postincrement";
        }
        Op::BytePostincrementStore => {
            let increment = ((x >> 8) & 15) * 16 + (x & 15);
            let address = cpu.r[s];
            mem = Some((
                d,
                s,
                address,
                1,
                true,
                false,
                Some(address.wrapping_add(increment)),
            ));
            name = "byte_postincrement_store";
        }
        Op::BytePostincrementLoad => {
            let off = ((x >> 8) & 15) * 16 + (x & 15);
            let addr = cpu.r[s];
            mem = Some((
                d,
                s,
                addr,
                1,
                false,
                h & 4 != 0,
                Some(addr.wrapping_add(off)),
            ));
            name = "byte_postincrement_load";
        }
        Op::MultiplyImmediate => {
            cpu.r[n] = cpu.r[d].wrapping_mul(packed(x));
            name = "multiply_immediate";
        }
        Op::BitMask => {
            let mask = 1u32 << (cpu.r[c] & 31);
            cpu.r[d] = match x & 3 {
                0 => cpu.r[s] | mask,
                1 => cpu.r[s] ^ mask,
                2 => cpu.r[s] & mask,
                _ => cpu.r[s] & !mask,
            };
            name = "bit_mask";
        }
        Op::StackPair => {
            let addr = cpu.sr[14] + (x & 4092);
            let r = d & 14;
            if x & 1 != 0 {
                cpu.write(addr, cpu.r[r])?;
                cpu.write(addr + 4, cpu.r[r + 1])?;
            } else {
                cpu.r[r] = cpu.read(addr, 4)?;
                cpu.r[r + 1] = cpu.read(addr + 4, 4)?;
            }
            name = "stack_pair";
        }
        Op::ReverseBytes => {
            cpu.r[d] = cpu.r[c].swap_bytes();
            name = "reverse_bytes";
        }
        Op::SubtractPackedImmediate => {
            cpu.r[n] = cpu.r[d].wrapping_sub(packed(x));
            name = "subtract_packed_immediate";
        }
        Op::ReverseSubtract => {
            cpu.r[n] = packed(x).wrapping_sub(cpu.r[d]);
            name = "reverse_subtract";
        }
        Op::MultiplyLong => {
            // rD+1:rD = rS * rC (64-bit product). x bit 12 selects signed;
            // the stock 64-bit multiply uses bit 12 clear for its unsigned
            // low*low partial product (Quarkslab lists the opposite sense
            // for mul/mul.z but muladd uses this one).
            let pair = d & 14;
            let product = if x & 0x1000 != 0 {
                (cpu.r[s] as i32 as i64).wrapping_mul(cpu.r[c] as i32 as i64) as u64
            } else {
                cpu.r[s] as u64 * cpu.r[c] as u64
            };
            cpu.r[pair] = product as u32;
            cpu.r[pair + 1] = (product >> 32) as u32;
            name = "multiply_long";
        }
        Op::MultiplyAccumulateLong => {
            // rD+1:rD += rS * rC, fields as MultiplyLong. Felucca's signed
            // Q30 filters chain e1fc bde0; stock soft-double uses e1fc ae60.
            let pair = d & 14;
            let product = if x & 0x1000 != 0 {
                (cpu.r[s] as i32 as i64).wrapping_mul(cpu.r[c] as i32 as i64) as u64
            } else {
                cpu.r[s] as u64 * cpu.r[c] as u64
            };
            let sum =
                ((cpu.r[pair] as u64) | ((cpu.r[pair + 1] as u64) << 32)).wrapping_add(product);
            cpu.r[pair] = sum as u32;
            cpu.r[pair + 1] = (sum >> 32) as u32;
            name = "multiply_accumulate_long";
        }
        Op::DivideLong => {
            // rD+1:rD = (rS+1:rS) / rC, unsigned 64-by-32 division. The stock
            // microsecond conversion divides by 1000000 then rebuilds the
            // remainder from both quotient words.
            // x bit 12 selects the signed form (Felucca's formant filter:
            // an int64 divided by an int32).
            let pair = d & 14;
            let dividend = cpu.r[s] as u64 | ((cpu.r[s + 1] as u64) << 32);
            let quotient = if x & 0x1000 != 0 {
                (dividend as i64)
                    .checked_div(cpu.r[c] as i32 as i64)
                    .map(|q| q as u64)
            } else {
                dividend.checked_div(cpu.r[c] as u64)
            }
            .ok_or(Fault::Unsupported { pc, word: h as u16 })?;
            cpu.r[pair] = quotient as u32;
            cpu.r[pair + 1] = (quotient >> 32) as u32;
            name = "divide_long";
        }
        Op::CarryArithmetic => {
            // addc/subc: carry in from PSR.C, carry out to PSR.C.
            let (lhs, rhs) = (cpu.r[s] as u64, cpu.r[c] as u64);
            if x & 2 == 0 {
                let sum = lhs + rhs + u64::from(cpu.carry());
                cpu.r[d] = sum as u32;
                cpu.set_carry(sum >> 32 != 0);
                name = "add_with_carry";
            } else {
                let borrow = u64::from(!cpu.carry());
                cpu.r[d] = lhs.wrapping_sub(rhs + borrow) as u32;
                cpu.set_carry(lhs >= rhs + borrow);
                name = "subtract_with_carry";
            }
        }
        Op::MultiplyExtended => {
            cpu.r[d] = cpu.r[s].wrapping_mul(cpu.r[c]);
            name = "multiply_extended";
        }
        Op::Divide => {
            cpu.r[d] = if x & 1 == 0 {
                cpu.r[s].checked_div(cpu.r[c])
            } else {
                (cpu.r[s] as i32)
                    .checked_div(cpu.r[c] as i32)
                    .map(|v| v as u32)
            }
            .ok_or(Fault::Unsupported { pc, word: h as u16 })?;
            name = if x & 1 == 0 {
                "divide_unsigned"
            } else {
                "divide_signed"
            };
        }
        Op::CountLeadingZeros => {
            cpu.r[d] = cpu.r[c].leading_zeros();
            name = "count_leading_zeros";
        }
        Op::Absolute => {
            cpu.r[d] = (cpu.r[c] as i32).wrapping_abs() as u32;
            name = "absolute";
        }
        Op::Maximum => {
            cpu.r[d] = if x & 1 == 0 {
                cpu.r[s].max(cpu.r[c])
            } else {
                (cpu.r[s] as i32).max(cpu.r[c] as i32) as u32
            };
            name = if x & 1 == 0 {
                "maximum_unsigned"
            } else {
                "maximum_signed"
            };
        }
        Op::Minimum => {
            cpu.r[d] = if x & 1 == 0 {
                cpu.r[s].min(cpu.r[c])
            } else {
                (cpu.r[s] as i32).min(cpu.r[c] as i32) as u32
            };
            name = if x & 1 == 0 {
                "minimum_unsigned"
            } else {
                "minimum_signed"
            };
        }
        Op::MemoryAdd => {
            let addr = cpu.r[d] + (h & 31) * 4;
            cpu.write(
                addr,
                cpu.read(addr, 4)?.wrapping_add(signed(x & 4095, 12) as u32),
            )?;
            name = "memory_add";
        }
        Op::DecrementBranch => {
            cpu.r[n] = cpu.r[n].wrapping_sub(1);
            if cpu.r[n] != 0 {
                next = next.wrapping_add((signed(x, 16) * 2) as u32);
            }
            name = "decrement_branch";
        }
        Op::MemoryBit => {
            let addr = cpu.r[d] + (x & 252);
            let old = cpu.read(addr, 4)?;
            let mask = 1u32 << (cpu.r[c] & 31);
            cpu.write(
                addr,
                match x & 3 {
                    0 => old | mask,
                    1 => old ^ mask,
                    3 => old & !mask,
                    _ => return Err(unsupported(pc, h)),
                },
            )?;
            name = "memory_bit";
        }
        Op::AddSubtractExtended => {
            // Carry out feeds a following addc/subc (64-bit arithmetic).
            let (lhs, rhs) = (cpu.r[s], cpu.r[c]);
            if x & 2 == 0 {
                let (sum, carry) = lhs.overflowing_add(rhs);
                cpu.r[d] = sum;
                cpu.set_carry(carry);
            } else {
                cpu.r[d] = lhs.wrapping_sub(rhs);
                cpu.set_carry(lhs >= rhs);
            }
            name = if x & 2 == 0 {
                "add_extended"
            } else {
                "subtract_extended"
            };
        }
        Op::ShiftRegisterExtended => {
            // Quarkslab pi32v2 imm1619: 0 and 1 lsl, 2 lsr, 3 asr (qasr).
            let shift = cpu.r[c];
            cpu.r[d] = match x & 15 {
                0 | 1 => cpu.r[s].checked_shl(shift).unwrap_or(0),
                2 => cpu.r[s].checked_shr(shift).unwrap_or(0),
                3 => ((cpu.r[s] as i32) >> shift.min(31)) as u32,
                _ => return Err(unsupported(pc, h)),
            };
            name = "shift_register_extended";
        }
        Op::BytePreincrement => {
            let addr = cpu.r[s].wrapping_add(cpu.r[c]);
            mem = Some((d, s, addr, 1, x & 1 != 0, x & 2 != 0, Some(addr)));
            name = "byte_preincrement";
        }
        Op::ShiftExtended => {
            let shift = ((x >> 8) & 3) * 16 + (x & 15);
            let mode = (x >> 10) & 3;
            cpu.r[d] = match mode {
                0 => cpu.r[s].checked_shl(shift).unwrap_or(0),
                2 => cpu.r[s].checked_shr(shift).unwrap_or(0),
                _ => ((cpu.r[s] as i32) >> shift.min(31)) as u32,
            };
            name = "shift_extended";
        }
        Op::ShiftPair => {
            // rD+1:rD shifted as one 64-bit value, in place (same shift
            // fields as ShiftExtended). Felucca sign-extends r4 into r5:r4
            // with e1d0 4200; e1d0 4e00 and scales Q30 products with 490e.
            let shift = ((x >> 8) & 3) * 16 + (x & 15);
            let pair = (cpu.r[d] as u64) | ((cpu.r[d + 1] as u64) << 32);
            let value = match (x >> 10) & 3 {
                0 => pair.checked_shl(shift).unwrap_or(0),
                2 => pair.checked_shr(shift).unwrap_or(0),
                _ => ((pair as i64) >> shift.min(63)) as u64,
            };
            cpu.r[d] = value as u32;
            cpu.r[d + 1] = (value >> 32) as u32;
            name = "shift_pair";
        }
        Op::ShiftPairRegister => {
            // rD+1:rD shifted in place by rC (stock unsigned-to-double:
            // e1d8 4700 normalises the mantissa by 47 for the value 48).
            let shift = cpu.r[c];
            let pair = (cpu.r[d] as u64) | ((cpu.r[d + 1] as u64) << 32);
            let value = match x & 3 {
                0 | 1 => pair.checked_shl(shift).unwrap_or(0),
                2 => pair.checked_shr(shift).unwrap_or(0),
                _ => ((pair as i64) >> shift.min(63)) as u64,
            };
            cpu.r[d] = value as u32;
            cpu.r[d + 1] = (value >> 32) as u32;
            name = "shift_pair_register";
        }
        Op::FloatOp => {
            // e53f, x = d c s op on IEEE single bits: rD = rS op rC with
            // 0 add, 1 sub, 2 mul, 3 div (SDK rx_net_samples_avg: sum / n),
            // 5 min, 6 max (clamp pairs), 7 rD += rS*rC, 8 rD -= rS*rC
            // (stock complex multiply at 0x0208bd30). Op 15 is unary on rC
            // with the sub-operation in s: 8 (float)i32, 9 (float)u32,
            // 1 (i32) truncation. Rounding of 7/8 (fused or not), NaN
            // ordering in min/max and conversion saturation are unverified.
            let fl = |v: u32| f32::from_bits(v);
            let (a, b) = (fl(cpu.r[s]), fl(cpu.r[c]));
            cpu.r[d] = match x & 15 {
                0 => (a + b).to_bits(),
                1 => (a - b).to_bits(),
                2 => (a * b).to_bits(),
                3 => (a / b).to_bits(),
                5 => a.min(b).to_bits(),
                6 => a.max(b).to_bits(),
                7 => (fl(cpu.r[d]) + a * b).to_bits(),
                8 => (fl(cpu.r[d]) - a * b).to_bits(),
                _ => match (x >> 4) & 15 {
                    8 => (cpu.r[c] as i32 as f32).to_bits(),
                    9 => (cpu.r[c] as f32).to_bits(),
                    _ => b as i32 as u32,
                },
            };
            name = "float_op";
        }
        Op::BranchLong => {
            let value = match h & 0x60 {
                0 => x & 4095,
                0x20 => packed(x),
                0x40 => cpu.r[c],
                _ => packed(x),
            };
            let lhs = cpu.r[d];
            let test = if h & 0x60 == 0x60 {
                if h & 1 == 0 {
                    lhs & value == 0
                } else {
                    lhs & value != 0
                }
            } else {
                match h & 15 {
                    0 => lhs == value,
                    1 => lhs != value,
                    2 => lhs >= value,
                    3 => lhs < value,
                    8 => lhs > value,
                    9 => lhs <= value,
                    10 => (lhs as i32) >= (value as i32),
                    11 => (lhs as i32) < (value as i32),
                    12 => (lhs as i32) > (value as i32),
                    _ => (lhs as i32) <= (value as i32),
                }
            };
            next = pc + 6;
            if test {
                next = next.wrapping_add((signed(cpu.operand(code.y, pc + 4)?, 16) * 2) as u32);
            }
            name = "branch_long";
        }
        Op::LogicThree => {
            cpu.r[d] = match x & 3 {
                0 => cpu.r[s] | cpu.r[c],
                1 => cpu.r[s] ^ cpu.r[c],
                2 => cpu.r[s] & cpu.r[c],
                // and-not, as in memory_logic and logic_immediate mode 7:
                // stock `r1 = 1; r0 = r1 & ~r0` negates a 0/1 flag.
                _ => cpu.r[s] & !cpu.r[c],
            };
            name = "logic_three";
        }
        // Register lists transfer the lowest register at the base address and
        // ascend (struct fields: Felucca's gr_grain_t {z, pos}, the stock
        // linked-list nodes {next, prev}); the base register is unchanged.
        Op::StoreRegisterList => {
            // Lowest register at the lowest address (Felucca fm1_fault_c's
            // {r5, r1} = [r4+] reads magic into r1; stock list_add_tail).
            let mut address = cpu.r[n];
            for register in 0..16 {
                if x & (1 << register) != 0 {
                    cpu.write(address, cpu.r[register])?;
                    address = address.wrapping_add(4);
                }
            }
            name = "store_register_list";
        }
        Op::LoadRegisterList => {
            let mut address = cpu.r[n];
            for register in 0..16 {
                if x & (1 << register) != 0 {
                    cpu.r[register] = cpu.read(address, 4)?;
                    address = address.wrapping_add(4);
                }
            }
            name = "load_register_list";
        }
        Op::StackSubword => {
            let halfword = h & 4 == 0;
            let offset = if halfword { x & 4094 } else { x & 4095 };
            mem = Some((
                d,
                0,
                cpu.sr[14].wrapping_add(offset),
                if halfword { 2 } else { 1 },
                if halfword {
                    h == 0xe9d8 && x & 1 != 0
                } else {
                    h == 0xe9de
                },
                h & 1 != 0,
                None,
            ));
            name = "stack_subword";
        }
        Op::StackExtended => {
            mem = Some((d, 0, cpu.sr[14] + (x & 4092), 4, x & 1 != 0, false, None));
            name = "stack_extended";
        }
        Op::MemoryPair => {
            let offset = (signed(h & 7, 3) << 8) | (((x >> 8) & 15) << 4) as i32 | (x & 12) as i32;
            let addr = cpu.r[s].wrapping_add(offset as u32);
            let reg = d & 14;
            if x & 1 != 0 {
                cpu.write(addr, cpu.r[reg])?;
                cpu.write(addr + 4, cpu.r[reg + 1])?;
            } else {
                cpu.r[reg] = cpu.read(addr, 4)?;
                cpu.r[reg + 1] = cpu.read(addr + 4, 4)?;
            }
            if x & 3 == 3 {
                // x & 3 == 3: a store that writes the address back to the base.
                cpu.r[s] = addr;
            }
            name = "memory_pair";
        }
        Op::BitField => {
            let pos = (x >> 7) & 31;
            let len = (x >> 2) & 31;
            let mask = (1u32 << len) - 1;
            cpu.r[n] = if h & 0x10 == 0 {
                (cpu.r[n] & !(mask << pos)) | ((cpu.r[d] & mask) << pos)
            } else {
                (cpu.r[d] >> pos) & mask
            };
            name = "bit_field";
        }
        Op::BranchEqualFlag => {
            if cpu.sr[5] & 4 != 0 {
                next = next.wrapping_add((signed(x, 16) * 2) as u32);
            }
            name = "branch_equal_flag";
        }
        Op::BranchBit => {
            let test = (cpu.r[n] & (1 << ((x >> 11) & 31)) != 0) == (x & 512 != 0);
            if test {
                next = next.wrapping_add((signed(x & 511, 9) * 2) as u32);
            }
            name = "branch_bit";
        }
        Op::MemoryMask => {
            let addr = cpu.r[d].wrapping_add((h & 31) * 4);
            let old = cpu.read(addr, 4)?;
            let value = packed(x);
            cpu.write(
                addr,
                match h & 0xc0 {
                    0 => old | value,
                    0x80 => old & value,
                    _ => old & !value,
                },
            )?;
            name = "memory_mask";
        }
        Op::ConditionalBlock => {
            let kind = (h >> 4) & 255;
            let lhs = cpu.r[n];
            let rhs = if kind & 7 == 1 {
                cpu.r[c]
            } else if matches!(kind, 0x83 | 0x93 | 0x9b | 0xd3 | 0xdb | 0xe3 | 0xeb) {
                signed(x & 4095, 12) as u32
            } else if kind == 0xcb {
                // Plain imm12, not packed as Quarkslab lists it: SLOOP's
                // reverb wraps `++line_i[3] >= 2791` with ecb1 0ae6 (<= 2790).
                x & 4095
            } else {
                packed(x)
            };
            let test = match kind {
                0x81..=0x83 => lhs == rhs,
                0x89..=0x8b => lhs != rhs,
                0x91..=0x93 => lhs >= rhs,
                0x99 | 0x9b => lhs < rhs,
                0xa1 => {
                    if x & 128 == 0 {
                        lhs & rhs == 0
                    } else {
                        lhs & rhs != 0
                    }
                }
                0xa2 => lhs & rhs == 0,
                0xa3 => lhs & rhs != 0,
                0xc1 | 0xc3 => lhs > rhs,
                0xc9..=0xcb => lhs <= rhs,
                0xd1..=0xd3 => (lhs as i32) >= (rhs as i32),
                0xd9..=0xdb => (lhs as i32) < (rhs as i32),
                // Same condition order as the compare-branches: 0xee00 is
                // signed >, 0xee80 signed <= (FM-1_093 0x02002bea: ee15).
                0xe1 | 0xe3 => (lhs as i32) > (rhs as i32),
                _ => (lhs as i32) <= (rhs as i32),
            };
            next = cpu.conditional(test, x)?;
            name = "conditional_block";
        }
        Op::MemoryLogic => {
            let addr = cpu.r[d].wrapping_add(x & 0xfc);
            let old = cpu.read(addr, 4)?;
            let value = cpu.r[c];
            cpu.write(
                addr,
                match x & 3 {
                    0 => old | value,
                    1 => old ^ value,
                    2 => old & value,
                    _ => old & !value,
                },
            )?;
            name = "memory_logic";
        }
        Op::LogicImmediate => {
            let mode = (h >> 4) & 15;
            let value = if mode == 6 && x & 0xc00 == 0 {
                x & 1023
            } else {
                packed(x)
            };
            cpu.r[n] = match mode {
                4 => cpu.r[d] | value,
                5 => cpu.r[d] ^ value,
                6 => cpu.r[d] & value,
                _ => cpu.r[d] & !value,
            };
            name = "logic_immediate";
        }
        Op::StoreImmediate => {
            cpu.write(cpu.r[d].wrapping_add((h & 31) * 4), packed(x))?;
            name = "store_immediate";
        }
        Op::AddImmediate => {
            let value = match (h >> 4) & 15 {
                0 => x & 4095,
                1 => (x & 4095) + 4096,
                2 => (x & 4095) | 0xffffe000,
                3 => (x & 4095) | 0xfffff000,
                _ => packed(x),
            };
            cpu.r[n] = cpu.r[d].wrapping_add(value);
            name = "add_immediate";
        }
        Op::AddStackExtended => {
            cpu.r[d] = cpu.sr[14].wrapping_add(x & 4095);
            name = "add_stack_extended";
        }
        Op::AdjustStackExtended => {
            // sp += signed 13-bit immediate: stock prologues pair e8f0 1d98
            // (-616) with epilogues e8f0 0268 (+616) around push/pop.
            cpu.sr[14] = cpu.sr[14].wrapping_add(signed(x, 13) as u32);
            name = "adjust_stack_extended";
        }
        Op::BranchCompareImmediate => {
            let kind = (h >> 7) & 63;
            // The ordered unsigned forms (jae jb ja jbe) take an unsigned imm10
            // (SLOOP's fm1_delay_us: jb r1, #960); equality (je jne) and the
            // signed forms sign-extend it (Baud Girl 0x02001820: je against
            // -2, f874 fc04). Quarkslab's spec lists je as unsigned too; the
            // stock-toolchain code says otherwise.
            let raw = (((h >> 4) & 7) << 7) | (x >> 9);
            let immediate = signed(raw, 10) as u32;
            let v = cpu.r[n];
            let test = match kind {
                0x30 => v == immediate,
                0x31 => v != immediate,
                0x32 => v >= raw,
                0x33 => v < raw,
                0x38 => v > raw,
                0x39 => v <= raw,
                0x3a => (v as i32) >= signed(immediate, 10),
                0x3b => (v as i32) < signed(immediate, 10),
                0x3c => (v as i32) > signed(immediate, 10),
                _ => (v as i32) <= signed(immediate, 10),
            };
            if test {
                next = next.wrapping_add((signed(x & 511, 9) * 2) as u32);
            }
            name = "branch_compare_immediate";
        }
        Op::BranchCompareRegister => {
            let lhs = cpu.r[d];
            let rhs = cpu.r[n];
            let test = match h & 0xfff0 {
                0xe800 => lhs == rhs,
                0xe880 => lhs != rhs,
                0xe900 => lhs >= rhs,
                0xe980 => lhs < rhs,
                0xec00 => lhs > rhs,
                0xec80 => lhs <= rhs,
                0xed00 => (lhs as i32) >= (rhs as i32),
                0xed80 => (lhs as i32) < (rhs as i32),
                0xee00 => (lhs as i32) > (rhs as i32),
                _ => (lhs as i32) <= (rhs as i32),
            };
            if test {
                next = next.wrapping_add((signed(x & 511, 9) * 2) as u32);
            }
            name = "branch_compare_register";
        }
        Op::ByteExtended => {
            // Bit 0 is the sign of a 9-bit byte offset (Quarkslab 0xe59:
            // "[++rB=-imm8]"); stock string scans use b[r3+-18].
            let off = signed((h & 1) << 8 | ((x >> 8) & 15) << 4 | (x & 15), 9) as u32;
            mem = Some((
                d,
                s,
                cpu.r[s].wrapping_add(off),
                1,
                h & 2 != 0,
                h & 6 == 4,
                if h & 8 != 0 {
                    Some(cpu.r[s].wrapping_add(off))
                } else {
                    None
                },
            ));
            name = "byte_extended";
        }
        Op::WordExtended => {
            let offset = (signed(h & 7, 3) << 8) | (((x >> 8) & 15) << 4) as i32 | (x & 12) as i32;
            let addr = cpu.r[s].wrapping_add(offset as u32);
            let mode = x & 3;
            mem = Some((
                d,
                s,
                addr,
                4,
                mode & 1 != 0,
                false,
                if mode & 2 != 0 { Some(addr) } else { None },
            ));
            name = "word_extended";
        }
        Op::WordRegisterPreincrementStore => {
            let address = cpu.r[s].wrapping_add(cpu.r[c]);
            cpu.write(address, cpu.r[d])?;
            cpu.r[s] = address;
            name = "word_register_preincrement_store";
        }
        Op::WordRegisterPreincrement => {
            // Vendor form: rD = [++rS=rC]. Commit the base before the
            // destination so a load into its own base retains the loaded word.
            let address = cpu.r[s].wrapping_add(cpu.r[c]);
            let value = cpu.read(address, 4)?;
            cpu.r[s] = address;
            cpu.r[d] = value;
            name = "word_register_preincrement";
        }
        Op::HalfwordRegisterPreincrement => {
            let address = cpu.r[s].wrapping_add(cpu.r[c]);
            let value = cpu.read(address, 2)?;
            cpu.r[s] = address;
            cpu.r[d] = if x & 2 != 0 {
                signed(value, 16) as u32
            } else {
                value
            };
            name = "halfword_register_preincrement";
        }
        Op::WordPostincrementStore => {
            let increment = (((x >> 8) & 15) << 4) | (x & 12);
            let address = cpu.r[s];
            mem = Some((
                d,
                s,
                address,
                4,
                true,
                false,
                Some(address.wrapping_add(increment)),
            ));
            name = "word_postincrement_store";
        }
        Op::WordPostincrementLoad => {
            // rD = [rS++=imm]: the load twin of the store above (Quarkslab
            // "lw eregA, [eregB++=imm8]"). Word accesses have no signed
            // indexed form, so x&3==0 is not [rS+rC]; the stock device table
            // walk at 0x020347fc advances by 28 bytes this way.
            let increment = (((x >> 8) & 15) << 4) | (x & 12);
            let address = cpu.r[s];
            mem = Some((
                d,
                s,
                address,
                4,
                false,
                false,
                Some(address.wrapping_add(increment)),
            ));
            name = "word_postincrement_load";
        }
        Op::MemoryIndexed => {
            let size = match h {
                0xecd8 => 4,
                0xedd8 => 2,
                _ => 1,
            };
            let scaled = x & 8 != 0;
            let mode = x & 7;
            let addr =
                cpu.r[s].wrapping_add(cpu.r[c].wrapping_mul(if scaled { size as u32 } else { 1 }));
            mem = Some((
                d,
                s,
                addr,
                size,
                if size == 4 { mode == 3 } else { mode == 1 },
                mode == 2 && size != 4,
                None,
            ));
            name = "memory_indexed";
        }
        _ => return Err(unsupported(pc, h)),
    }
    access(cpu, pc, mem)?;
    Ok((next, name))
}
