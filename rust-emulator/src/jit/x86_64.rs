// SPDX-License-Identifier: GPL-3.0-only
use crate::blocks::{Op, Reg, Value};
fn load(out: &mut Vec<u8>, value: Value, to: u8) {
    match value {
        Value::Immediate(v) => {
            out.push(0xb8 + to);
            out.extend(v.to_le_bytes());
        }
        Value::Register(Reg(r)) => {
            out.extend([0x41, 0x8b, 0x40 | (to << 3) | (r >= 16) as u8, (r & 15) * 4])
        }
    }
}
fn store(out: &mut Vec<u8>, r: Reg, from: u8) {
    out.extend([
        0x41,
        0x89,
        0x40 | (from << 3) | (r.0 >= 16) as u8,
        (r.0 & 15) * 4,
    ]);
}
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(super) fn emit(op: Op, out: &mut Vec<u8>, windows: bool) -> bool {
    // Normalize C ABI argument pointers into volatile r8/r9. rdi/rsi are
    // callee-saved on Windows, so generated code never writes them.
    if windows {
        out.extend([0x49, 0x89, 0xc8, 0x49, 0x89, 0xd1]);
    } else {
        out.extend([0x49, 0x89, 0xf8, 0x49, 0x89, 0xf1]);
    }
    use Value::Register as R;
    match op {
        Op::Move(d, v) => {
            load(out, v, 0);
            store(out, d, 0);
        }
        Op::Arithmetic(d, l, r, sub) => {
            load(out, l, 0);
            load(out, r, 1);
            out.extend([if sub { 0x29 } else { 0x01 }, 0xc8]); // SUB/ADD eax,ecx.
                                                               // SETcc leaves flags unchanged. Guest subtraction carry is !CF.
            out.extend([
                0x41,
                0x0f,
                0x90,
                0xc2,
                0x41,
                0x0f,
                if sub { 0x93 } else { 0x92 },
                0xc3,
                0x0f,
                0x94,
                0xc1,
                0x0f,
                0x98,
                0xc2,
            ]);
            store(out, d, 0);
            out.extend([
                0x45, 0x0f, 0xb6, 0xd2, 0x45, 0x0f, 0xb6, 0xdb, 0x0f, 0xb6, 0xc9, 0x0f, 0xb6, 0xd2,
                0x41, 0xd1, 0xe3, 0xc1, 0xe1, 2, 0xc1, 0xe2, 3, 0x44, 0x09, 0xd1, 0x44, 0x09, 0xd9,
                0x09, 0xd1,
            ]);
            load(out, R(Reg(21)), 0);
            out.extend([0x83, 0xe0, 0xf0, 0x09, 0xc8]);
            store(out, Reg(21), 0);
        }
        Op::Logic(d, l, r, kind) => {
            load(out, l, 0);
            load(out, r, 1);
            if kind == 3 {
                out.extend([0xf7, 0xd1]);
            }
            out.extend([
                match kind {
                    0 => 0x09,
                    1 => 0x31,
                    _ => 0x21,
                },
                0xc8,
            ]);
            store(out, d, 0);
        }
        Op::Shift(d, s, count, kind) => {
            load(out, R(s), 0);
            if count >= 32 && kind < 2 {
                out.extend([0x31, 0xc0]);
            } else {
                out.extend([
                    0xc1,
                    [0xe0, 0xe8, 0xf8, 0xc8][kind as usize],
                    if kind == 2 { count.min(31) } else { count & 31 },
                ]);
            }
            store(out, d, 0);
        }
        Op::Multiply(d, s, v) => {
            load(out, R(s), 0);
            load(out, v, 1);
            out.extend([0x0f, 0xaf, 0xc1]);
            store(out, d, 0);
        }
        Op::Extend(d, s, bits, sign) => {
            load(out, R(s), 0);
            out.extend([
                0x0f,
                if sign {
                    if bits == 8 {
                        0xbe
                    } else {
                        0xbf
                    }
                } else if bits == 8 {
                    0xb6
                } else {
                    0xb7
                },
                0xc0,
            ]);
            store(out, d, 0);
        }
        Op::Nop => {}
        _ => return false,
    }
    out.push(0xc3);
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodings_use_the_correct_argument_registers_for_both_host_abis() {
        for windows in [false, true] {
            let mut bytes = Vec::new();
            assert!(emit(
                Op::Move(Reg(15), Value::Register(Reg(21))),
                &mut bytes,
                windows
            ));
            let mut expected = if windows {
                vec![0x49, 0x89, 0xc8, 0x49, 0x89, 0xd1]
            } else {
                vec![0x49, 0x89, 0xf8, 0x49, 0x89, 0xf1]
            };
            expected.extend([0x41, 0x8b, 0x41, 0x14, 0x41, 0x89, 0x40, 0x3c, 0xc3]);
            assert_eq!(bytes, expected);
        }
    }
}
