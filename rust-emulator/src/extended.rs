// SPDX-License-Identifier: GPL-3.0-only
// Compiler-emitted forms, decoded against vendor disassembly and the pinned
// Quarkslab pi32v2 SLEIGH reference. Unknown/reserved forms still fault.
use crate::cpu::{signed, Cpu, Fault};
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
pub(crate) fn execute(
    cpu: &mut Cpu,
    h: u32,
    pc: u32,
) -> Result<Option<(u32, &'static str)>, Fault> {
    let a = (h & 7) as usize;
    let b = ((h >> 4) & 7) as usize;
    let mut next = pc + 2;
    let mut mem = None;
    let op;
    if h & 0xff11 == 0x1500 {
        let destination = (h & 14) as usize;
        let source = ((h >> 4) & 14) as usize;
        let values = [cpu.r[source], cpu.r[source + 1]];
        cpu.r[destination..destination + 2].copy_from_slice(&values);
        op = "move_register_pair";
    } else if h & 0xff00 == 0x1b00 {
        let n = (h & 15) as usize;
        cpu.r[n] = cpu.r[n].wrapping_mul(cpu.r[((h >> 4) & 15) as usize]);
        op = "multiply";
    } else if h & 0xfff1 == 0x1480 {
        let destination = (h & 14) as usize;
        cpu.r[destination] = 0;
        cpu.r[destination + 1] = 0;
        op = "clear_pair";
    } else if h & 0xff00 == 0x1800 {
        let n = (h & 15) as usize;
        cpu.r[n] = cpu.r[n].wrapping_add(cpu.r[((h >> 4) & 15) as usize]);
        op = "add_register";
    } else if h & 0xfff8 == 0x14c0 {
        cpu.r[8 + a] = 0;
        op = "clear_high_register";
    } else if h & 0xfff0 == 0x0230 {
        op = "cache_flush_invalidate";
    } else if h & 0xe058 == 0x2000 {
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
        op = "stack_word";
    } else if matches!(h & 0xff88, 0x1700 | 0x1708 | 0x1780 | 0x1788) {
        let bits = if h & 0x80 == 0 { 8 } else { 16 };
        cpu.r[a] = if h & 8 != 0 {
            signed(cpu.r[b], bits) as u32
        } else {
            cpu.r[b] & ((1 << bits) - 1)
        };
        op = "extend";
    } else if h & 0xe0f8 == 0x2010 {
        cpu.r[a] = (h >> 8) | 0xffffffe0;
        op = "mov_negative";
    } else if matches!(h & 0xe0f8, 0x2030 | 0x2038 | 0x20b8) {
        let bit = 1 << ((h >> 8) & 31);
        match h & 0xf8 {
            0x30 => cpu.r[a] |= bit,
            0x38 => cpu.r[a] ^= bit,
            _ => cpu.r[a] &= !bit,
        }
        op = "bit_register";
    } else if matches!(h & 0xff88, 0x1a00 | 0x1a80 | 0x1a88) {
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
        op = "shift_register";
    } else if h & 0xe098 == 0x8088 {
        cpu.r[a] = cpu.sr[14].wrapping_add((((h >> 5) & 3) << 5) | ((h >> 8) & 31));
        op = "add_stack";
    } else if h & 0xfff0 == 0x00d0 {
        next = cpu.r[(h & 15) as usize];
        op = "goto_register";
    } else if h == 0x0022 {
        op = "ssync";
    } else if matches!(h & 0xfff0, 0x0100 | 0x0110) {
        let size = if h & 0x10 == 0 { 1 } else { 2 };
        next = (pc + 2)
            .wrapping_add(cpu.read((pc + 2).wrapping_add(cpu.r[(h & 15) as usize]), size)? * 2);
        op = "table_branch";
    } else if matches!(h & 0xe088, 0x4008 | 0x4088 | 0x6008 | 0x6088) {
        let size = if h & 0x2000 == 0 { 1 } else { 2 };
        let address = cpu.r[b].wrapping_add((signed((h >> 8) & 31, 5) * size as i32) as u32);
        mem = Some((a, b, address, size, h & 0x80 != 0, false, None));
        op = "memory_small";
    } else if (0x0500..0x0800).contains(&(h & 0xff80)) {
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
        op = "memory_postincrement";
    } else if h >> 13 == 7 {
        let x = cpu.read(pc + 2, 2)?;
        let n = (h & 15) as usize;
        let d = (x >> 12) as usize;
        let s = ((x >> 4) & 15) as usize;
        let c = ((x >> 8) & 15) as usize;
        next = pc + 4;
        if matches!(h & 0xff00, 0xfa00 | 0xfb00) {
            let test = (cpu.r[n] & cpu.r[((h >> 4) & 15) as usize] != 0) == (h & 0x100 != 0);
            if test {
                next = next.wrapping_add((signed(x, 16) * 2) as u32);
            }
            op = "branch_register_mask";
        } else if h == 0xe86c && x & 3 == 0 {
            let address = cpu.r[d].wrapping_add(x & 252);
            cpu.write(address, cpu.read(address, 4)? << c)?;
            op = "memory_shift_left";
        } else if h == 0xe868 {
            let addr = cpu.r[d] + (x & 252);
            cpu.write(addr, cpu.read(addr, 4)?.wrapping_add(cpu.r[c]))?;
            op = "memory_add_register";
        } else if h & 0xfff8 == 0xed50 || h & 0xfff8 == 0xed58 {
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
            op = "halfword_extended";
        } else if h == 0xedd0 {
            let increment = ((x >> 8) & 15) * 16 + (x & 15);
            let address = cpu.r[s];
            mem = Some((
                d,
                s,
                address,
                2,
                false,
                false,
                Some(address.wrapping_add(increment)),
            ));
            op = "halfword_postincrement";
        } else if h == 0xeed2 {
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
            op = "byte_postincrement_store";
        } else if matches!(h, 0xeed0 | 0xeed4) {
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
            op = "byte_postincrement_load";
        } else if h & 0xfff0 == 0xe1e0 {
            cpu.r[n] = cpu.r[d].wrapping_mul(packed(x));
            op = "multiply_immediate";
        } else if h == 0xe194 && x & 15 <= 3 {
            let mask = 1u32 << (cpu.r[c] & 31);
            cpu.r[d] = match x & 3 {
                0 => cpu.r[s] | mask,
                1 => cpu.r[s] ^ mask,
                2 => cpu.r[s] & mask,
                _ => cpu.r[s] & !mask,
            };
            op = "bit_mask";
        } else if h == 0xe9d0 {
            let addr = cpu.sr[14] + (x & 4092);
            let r = d & 14;
            if x & 1 != 0 {
                cpu.write(addr, cpu.r[r])?;
                cpu.write(addr + 4, cpu.r[r + 1])?;
            } else {
                cpu.r[r] = cpu.read(addr, 4)?;
                cpu.r[r + 1] = cpu.read(addr + 4, 4)?;
            }
            op = "stack_pair";
        } else if h == 0xe070 && x & 255 == 0 {
            cpu.r[d] = cpu.r[c].swap_bytes();
            op = "reverse_bytes";
        } else if h & 0xfff0 == 0xe0f0 {
            cpu.r[n] = cpu.r[d].wrapping_sub(packed(x));
            op = "subtract_packed_immediate";
        } else if h & 0xfff0 == 0xe0a0 {
            cpu.r[n] = packed(x).wrapping_sub(cpu.r[d]);
            op = "reverse_subtract";
        } else if h == 0xe1f0 && x & 15 == 0 {
            cpu.r[d] = cpu.r[s].wrapping_mul(cpu.r[c]);
            op = "multiply_extended";
        } else if h == 0xe1f4 && x & 15 <= 1 {
            cpu.r[d] = if x & 1 == 0 {
                cpu.r[s].checked_div(cpu.r[c])
            } else {
                (cpu.r[s] as i32)
                    .checked_div(cpu.r[c] as i32)
                    .map(|v| v as u32)
            }
            .ok_or(Fault::Unsupported { pc, word: h as u16 })?;
            op = if x & 1 == 0 {
                "divide_unsigned"
            } else {
                "divide_signed"
            };
        } else if h == 0xe430 && x & 255 == 0 {
            cpu.r[d] = (cpu.r[c] as i32).wrapping_abs() as u32;
            op = "absolute";
        } else if h == 0xe434 && x & 15 <= 1 {
            cpu.r[d] = if x & 1 == 0 {
                cpu.r[s].max(cpu.r[c])
            } else {
                (cpu.r[s] as i32).max(cpu.r[c] as i32) as u32
            };
            op = if x & 1 == 0 {
                "maximum_unsigned"
            } else {
                "maximum_signed"
            };
        } else if h == 0xe435 && x & 15 <= 1 {
            cpu.r[d] = if x & 1 == 0 {
                cpu.r[s].min(cpu.r[c])
            } else {
                (cpu.r[s] as i32).min(cpu.r[c] as i32) as u32
            };
            op = if x & 1 == 0 {
                "minimum_unsigned"
            } else {
                "minimum_signed"
            };
        } else if h & 0xffe0 == 0xebc0 {
            let addr = cpu.r[d] + (h & 31) * 4;
            cpu.write(
                addr,
                cpu.read(addr, 4)?.wrapping_add(signed(x & 4095, 12) as u32),
            )?;
            op = "memory_add";
        } else if h & 0xfff0 == 0xea00 {
            cpu.r[n] = cpu.r[n].wrapping_sub(1);
            if cpu.r[n] != 0 {
                next = next.wrapping_add((signed(x, 16) * 2) as u32);
            }
            op = "decrement_branch";
        } else if h == 0xe866 {
            let addr = cpu.r[d] + (x & 252);
            let old = cpu.read(addr, 4)?;
            let mask = 1u32 << (cpu.r[c] & 31);
            cpu.write(
                addr,
                match x & 3 {
                    0 => old | mask,
                    1 => old ^ mask,
                    3 => old & !mask,
                    _ => return Ok(None),
                },
            )?;
            op = "memory_bit";
        } else if h == 0xe0b4 && matches!(x & 15, 0 | 2) {
            cpu.r[d] = if x & 2 == 0 {
                cpu.r[s].wrapping_add(cpu.r[c])
            } else {
                cpu.r[s].wrapping_sub(cpu.r[c])
            };
            op = if x & 2 == 0 {
                "add_extended"
            } else {
                "subtract_extended"
            };
        } else if h == 0xe1c8 {
            let shift = cpu.r[c];
            cpu.r[d] = match x & 3 {
                0 => cpu.r[s].checked_shl(shift).unwrap_or(0),
                2 => cpu.r[s].checked_shr(shift).unwrap_or(0),
                _ => return Ok(None),
            };
            op = "shift_register_extended";
        } else if h == 0xeedc {
            let addr = cpu.r[s].wrapping_add(cpu.r[c]);
            mem = Some((d, s, addr, 1, x & 1 != 0, x & 2 != 0, Some(addr)));
            op = "byte_preincrement";
        } else if h == 0xe1c0 && (x >> 10) & 3 != 1 {
            let shift = ((x >> 8) & 3) * 16 + (x & 15);
            let mode = (x >> 10) & 3;
            cpu.r[d] = match mode {
                0 => cpu.r[s].checked_shl(shift).unwrap_or(0),
                2 => cpu.r[s].checked_shr(shift).unwrap_or(0),
                _ => ((cpu.r[s] as i32) >> shift.min(31)) as u32,
            };
            op = "shift_extended";
        } else if h & 0xff80 == 0xff00
            && matches!(h & 15, 0 | 1 | 2 | 3 | 8 | 9 | 10 | 11 | 12 | 13)
        {
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
                next = next.wrapping_add((signed(cpu.read(pc + 4, 2)?, 16) * 2) as u32);
            }
            op = "branch_long";
        } else if h == 0xe190 && x & 15 <= 3 {
            cpu.r[d] = match x & 3 {
                0 => cpu.r[s] | cpu.r[c],
                1 => cpu.r[s] ^ cpu.r[c],
                2 => cpu.r[s] & cpu.r[c],
                _ => !cpu.r[c],
            };
            op = "logic_three";
        } else if h & 0xfff0 == 0xeb00 && x != 0 {
            let mut address = cpu.r[n];
            for register in (0..16).rev() {
                if x & (1 << register) != 0 {
                    cpu.r[register] = cpu.read(address, 4)?;
                    address = address.wrapping_add(4);
                }
            }
            op = "load_register_list";
        } else if matches!(h, 0xe9d8 | 0xe9d9 | 0xe9dc | 0xe9dd | 0xe9de) {
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
            op = "stack_subword";
        } else if h == 0xe9d4 {
            mem = Some((d, 0, cpu.sr[14] + (x & 4092), 4, x & 1 != 0, false, None));
            op = "stack_extended";
        } else if h & 0xfff8 == 0xec50 && x & 3 <= 1 {
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
            op = "memory_pair";
        } else if matches!(h & 0xfff0, 0xe1a0 | 0xe1b0) {
            let pos = (x >> 7) & 31;
            let len = (x >> 2) & 31;
            let mask = (1u32 << len) - 1;
            cpu.r[n] = if h & 0x10 == 0 {
                (cpu.r[n] & !(mask << pos)) | ((cpu.r[d] & mask) << pos)
            } else {
                (cpu.r[d] >> pos) & mask
            };
            op = "bit_field";
        } else if h & 0xfff0 == 0xe850 {
            let test = (cpu.r[n] & (1 << ((x >> 11) & 31)) != 0) == (x & 512 != 0);
            if test {
                next = next.wrapping_add((signed(x & 511, 9) * 2) as u32);
            }
            op = "branch_bit";
        } else if h & 0xffe0 == 0xef00 || h & 0xffe0 == 0xefc0 {
            let addr = cpu.r[d].wrapping_add((h & 31) * 4);
            let old = cpu.read(addr, 4)?;
            let value = packed(x);
            cpu.write(
                addr,
                if h & 0xc0 == 0 {
                    old | value
                } else {
                    old & !value
                },
            )?;
            op = "memory_mask";
        } else if matches!(
            (h >> 4) & 255,
            0x81 | 0x82
                | 0x83
                | 0x89
                | 0x8b
                | 0x91
                | 0x92
                | 0x93
                | 0x99
                | 0x9b
                | 0xa1
                | 0xa2
                | 0xa3
                | 0xc1
                | 0xc3
                | 0xc9
                | 0xcb
                | 0xd1
                | 0xd2
                | 0xd3
                | 0xd9
                | 0xda
                | 0xdb
                | 0xe9
                | 0xeb
        ) && h & 0xf000 == 0xe000
        {
            let kind = (h >> 4) & 255;
            let lhs = cpu.r[n];
            let rhs = if kind & 7 == 1 {
                cpu.r[c]
            } else if matches!(kind, 0x83 | 0x93 | 0x9b | 0xd3 | 0xdb | 0xeb) {
                signed(x & 4095, 12) as u32
            } else {
                packed(x)
            };
            let test = match kind {
                0x81..=0x83 => lhs == rhs,
                0x89 | 0x8b => lhs != rhs,
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
                0xc9 | 0xcb => lhs <= rhs,
                0xd1..=0xd3 => (lhs as i32) >= (rhs as i32),
                0xd9..=0xdb => (lhs as i32) < (rhs as i32),
                _ => (lhs as i32) <= (rhs as i32),
            };
            next = cpu.conditional(test, x)?;
            op = "conditional_block";
        } else if h == 0xe864 {
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
            op = "memory_logic";
        } else if matches!(h & 0xfff0, 0xe140 | 0xe150 | 0xe160 | 0xe170) {
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
            op = "logic_immediate";
        } else if h & 0xffe0 == 0xea40 {
            cpu.write(cpu.r[d].wrapping_add((h & 31) * 4), packed(x))?;
            op = "store_immediate";
        } else if matches!(h & 0xfff0, 0xe100 | 0xe110 | 0xe120 | 0xe130 | 0xe0e0) {
            let value = match (h >> 4) & 15 {
                0 => x & 4095,
                1 => (x & 4095) + 4096,
                2 => (x & 4095) | 0xffffe000,
                3 => (x & 4095) | 0xfffff000,
                _ => packed(x),
            };
            cpu.r[n] = cpu.r[d].wrapping_add(value);
            op = "add_immediate";
        } else if h == 0xe8f8 {
            cpu.r[d] = cpu.sr[14].wrapping_add(x & 4095);
            op = "add_stack_extended";
        } else if h & 0xff80 == 0xf800
            || h & 0xff80 == 0xf880
            || matches!(
                h & 0xff80,
                0xf900 | 0xf980 | 0xfc00 | 0xfc80 | 0xfd00 | 0xfd80 | 0xfe00 | 0xfe80
            )
        {
            let kind = (h >> 7) & 63;
            let immediate = signed((((h >> 4) & 7) << 7) | (x >> 9), 10) as u32;
            let v = cpu.r[n];
            let test = match kind {
                0x30 => v == immediate,
                0x31 => v != immediate,
                0x32 => v >= immediate,
                0x33 => v < immediate,
                0x38 => v > immediate,
                0x39 => v <= immediate,
                0x3a => (v as i32) >= signed(immediate, 10),
                0x3b => (v as i32) < signed(immediate, 10),
                0x3c => (v as i32) > signed(immediate, 10),
                _ => (v as i32) <= signed(immediate, 10),
            };
            if test {
                next = next.wrapping_add((signed(x & 511, 9) * 2) as u32);
            }
            op = "branch_compare_immediate";
        } else if matches!(
            h & 0xfff0,
            0xe800 | 0xe880 | 0xe900 | 0xe980 | 0xec00 | 0xec80 | 0xed00 | 0xed80 | 0xee00 | 0xee80
        ) && x & 0xe00 == 0
        {
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
            op = "branch_compare_register";
        } else if matches!(h, 0xee50 | 0xee52 | 0xee54 | 0xee58 | 0xee5a) {
            let off = ((x >> 8) & 15) * 16 + (x & 15);
            mem = Some((
                d,
                s,
                cpu.r[s].wrapping_add(off),
                1,
                h & 2 != 0,
                h & 15 == 4,
                if h & 8 != 0 {
                    Some(cpu.r[s].wrapping_add(off))
                } else {
                    None
                },
            ));
            op = "byte_extended";
        } else if h & 0xfff8 == 0xecd0 {
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
            op = "word_extended";
        } else if h == 0xecdc && x & 15 == 3 {
            let address = cpu.r[s].wrapping_add(cpu.r[c]);
            cpu.write(address, cpu.r[d])?;
            cpu.r[s] = address;
            op = "word_register_preincrement_store";
        } else if h == 0xecdc && x & 15 == 2 {
            // Vendor form: rD = [++rS=rC]. Commit the base before the
            // destination so a load into its own base retains the loaded word.
            let address = cpu.r[s].wrapping_add(cpu.r[c]);
            let value = cpu.read(address, 4)?;
            cpu.r[s] = address;
            cpu.r[d] = value;
            op = "word_register_preincrement";
        } else if h == 0xeddc && matches!(x & 15, 0 | 2) {
            let address = cpu.r[s].wrapping_add(cpu.r[c]);
            let value = cpu.read(address, 2)?;
            cpu.r[s] = address;
            cpu.r[d] = if x & 2 != 0 {
                signed(value, 16) as u32
            } else {
                value
            };
            op = "halfword_register_preincrement";
        } else if matches!(h, 0xecd8 | 0xedd8 | 0xeed8) {
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
            op = "memory_indexed";
        } else {
            return Ok(None);
        }
    } else {
        return Ok(None);
    }
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
    Ok(Some((next, op)))
}
