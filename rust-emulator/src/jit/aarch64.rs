// SPDX-License-Identifier: GPL-3.0-only
use crate::blocks::{Op, Reg, Value};
fn word(out: &mut Vec<u8>, instruction: u32) {
    out.extend(instruction.to_le_bytes());
}
fn load(out: &mut Vec<u8>, v: Value, to: u32) {
    match v {
        Value::Register(Reg(r)) => word(
            out,
            0xb9400000 | (((r & 15) as u32) << 10) | (((r >= 16) as u32) << 5) | to,
        ),
        Value::Immediate(v) => {
            word(out, 0x52800000 | ((v & 65535) << 5) | to);
            if v >> 16 != 0 {
                word(out, 0x72a00000 | ((v >> 16) << 5) | to);
            }
        }
    }
}
fn store(out: &mut Vec<u8>, r: Reg, from: u32) {
    word(
        out,
        0xb9000000 | (((r.0 & 15) as u32) << 10) | (((r.0 >= 16) as u32) << 5) | from,
    );
}
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub(super) fn emit(op: Op, out: &mut Vec<u8>) -> bool {
    use Value::Register as R;
    match op {
        Op::Move(d, v) => {
            load(out, v, 2);
            store(out, d, 2);
        }
        Op::Arithmetic(d, l, r, sub) => {
            load(out, l, 2);
            load(out, r, 3);
            word(out, if sub { 0x6b030044 } else { 0x2b030044 }); // SUBS/ADDS w4,w2,w3.
                                                                  // Capture V, C, Z, N before any instruction can overwrite NZCV.
            for w in [
                0x1a9f77e5, 0x1a9f37e6, 0x1a9f17e7, 0x1a9f57e8, 0x2a0604a5, 0x2a0708a5, 0x2a080ca5,
            ] {
                word(out, w);
            }
            load(out, R(Reg(21)), 6);
            word(out, 0x121c6cc6); // Preserve PSR bits above the four condition flags.
            word(out, 0x2a0600a5);
            store(out, Reg(21), 5);
            store(out, d, 4);
        }
        Op::Logic(d, l, r, kind) => {
            load(out, l, 2);
            load(out, r, 3);
            word(
                out,
                [0x2a030044, 0x4a030044, 0x0a030044, 0x0a230044][kind as usize],
            );
            store(out, d, 4);
        }
        Op::Shift(d, s, count, kind) => {
            load(out, R(s), 2);
            if count >= 32 && kind < 2 {
                load(out, Value::Immediate(0), 4);
            } else {
                let c = if kind == 2 {
                    count.min(31) as u32
                } else {
                    (count & 31) as u32
                };
                let w = match kind {
                    0 => 0x53000044 | (((32 - c) & 31) << 16) | ((31 - c) << 10),
                    1 => 0x53007c44 | (c << 16),
                    2 => 0x13007c44 | (c << 16),
                    _ => 0x13820044 | (c << 10),
                };
                word(out, w);
            }
            store(out, d, 4);
        }
        Op::Multiply(d, s, v) => {
            load(out, R(s), 2);
            load(out, v, 3);
            word(out, 0x1b037c44);
            store(out, d, 4);
        }
        Op::Extend(d, s, bits, sign) => {
            load(out, R(s), 2);
            word(
                out,
                (if sign { 0x13000044 } else { 0x53000044 }) | ((bits as u32 - 1) << 10),
            );
            store(out, d, 4);
        }
        Op::Nop => {}
        _ => return false,
    }
    word(out, 0xd65f03c0);
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodings_match_the_native_assembler_for_register_and_immediate_moves() {
        let mut bytes = Vec::new();
        assert!(emit(
            Op::Move(Reg(15), Value::Register(Reg(21))),
            &mut bytes
        ));
        assert_eq!(
            bytes,
            [0xb9401422u32, 0xb9003c02, 0xd65f03c0]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        );
        bytes.clear();
        assert!(emit(
            Op::Move(Reg(0), Value::Immediate(0x12345678)),
            &mut bytes
        ));
        assert_eq!(
            bytes,
            [0x528acf02u32, 0x72a24682, 0xb9000002, 0xd65f03c0]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }
}
