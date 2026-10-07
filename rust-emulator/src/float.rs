// SPDX-License-Identifier: GPL-3.0-only
// r3 single-precision register operations, decoded with vendor objdump -mcpu=r3.
// Arithmetic/MAC rounding and conversions are checked with FM-1_986. Floating
// exceptions and nonfinite conversion results remain explicit limitations.
pub(crate) struct FloatResult {
    pub value: u32,
    pub flags: Option<u32>,
}

pub(crate) fn result(x: u32, registers: &[u32; 16]) -> Result<Option<FloatResult>, &'static str> {
    let d = (x >> 12) as usize;
    let s = ((x >> 4) & 15) as usize;
    let c = ((x >> 8) & 15) as usize;
    let a = f32::from_bits(registers[s]);
    let b = f32::from_bits(registers[c]);
    if x & 15 == 15 {
        // Unary operations on rC; s selects them (vendor objdump --mattr=+fprev1):
        // 0-3 ftoi and 4-7 ftou rounding to even, toward zero, up and down;
        // 8 itof, 9 utof; 10 rD.l = ftof(rC) to binary16, 11 rD = ftof(rC.l)
        // from binary16; 12-15 an integral float with the same four roundings.
        let round = |v: f32| match s & 3 {
            0 => v.round_ties_even(),
            1 => v.trunc(),
            2 => v.ceil(),
            _ => v.floor(),
        };
        let value = match s {
            8 => (registers[c] as i32 as f32).to_bits(),
            9 => (registers[c] as f32).to_bits(),
            0..=7 => {
                let v = round(b) as f64;
                let signed = s < 4;
                if !v.is_finite()
                    || if signed {
                        !(-2147483648.0..2147483648.0).contains(&v)
                    } else {
                        !(0.0..4294967296.0).contains(&v)
                    }
                {
                    return Err("exceptional floating-point conversion is not implemented");
                }
                if signed {
                    v as i32 as u32
                } else {
                    v as u32
                }
            }
            10..=15 if !b.is_finite() && s != 11 => {
                return Err("exceptional floating-point conversion is not implemented");
            }
            10 => (registers[d] & 0xffff_0000) | u32::from(to_half(b)),
            11 => from_half(registers[c] as u16)
                .ok_or("exceptional floating-point conversion is not implemented")?
                .to_bits(),
            _ => round(b).to_bits(),
        };
        return Ok(Some(FloatResult { value, flags: None }));
    }
    if matches!(x & 15, 5 | 6) {
        if !a.is_finite() || !b.is_finite() {
            return Err("exceptional floating-point comparison is not implemented");
        }
        let minimum = x & 15 == 5;
        let value = if a == 0.0 && b == 0.0 {
            // Hardware keeps -0 for MIN and +0 for MAX, in either order.
            if minimum {
                registers[s] | registers[c]
            } else {
                registers[s] & registers[c]
            }
        } else if minimum {
            a.min(b).to_bits()
        } else {
            a.max(b).to_bits()
        };
        let flags = ((a >= b) as u32) << 1 | ((a == b) as u32) << 2 | ((a < b) as u32) << 3;
        return Ok(Some(FloatResult {
            value,
            flags: Some(flags),
        }));
    }
    let value = match x & 15 {
        0 => a + b,
        1 => a - b,
        2 => a * b,
        3 => a / b,
        // Hardware rounds the product before accumulation; it is not fused.
        7 => f32::from_bits(registers[d]) + a * b,
        8 => f32::from_bits(registers[d]) - a * b,
        _ => return Ok(None),
    };
    // A finite value over zero with the divide-by-zero trap off (the trap is
    // taken before this). X0X runs this way on physical FM-1s: clang hoists
    // guarded divides, so its master limiter divides 1 by a silent sample's
    // magnitude every sample and discards the quotient. The quotient itself
    // is unmeasured; the IEEE value (infinity, or NaN for 0/0) is kept.
    let divide_by_zero = x & 15 == 3 && b == 0.0 && a.is_finite();
    if !value.is_finite() && !divide_by_zero {
        return Err("exceptional floating-point arithmetic is not implemented");
    }
    Ok(Some(FloatResult {
        value: value.to_bits(),
        flags: None,
    }))
}

/// IEEE binary32 to binary16, rounding to nearest even (finite inputs).
fn to_half(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent >= 31 {
        return sign | 0x7c00;
    }
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let full = mantissa | 0x80_0000;
        let shift = (14 - exponent) as u32;
        let half = full >> shift;
        let rest = full & ((1 << shift) - 1);
        let middle = 1 << (shift - 1);
        let up = rest > middle || (rest == middle && half & 1 != 0);
        return sign | (half + up as u32) as u16;
    }
    let half = ((exponent as u32) << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    let up = rest > 0x1000 || (rest == 0x1000 && half & 1 != 0);
    sign | (half + up as u32) as u16
}

/// IEEE binary16 to binary32, exact; `None` for infinities and NaNs.
fn from_half(h: u16) -> Option<f32> {
    let negative = h & 0x8000 != 0;
    let exponent = ((h >> 10) & 31) as i32;
    let mantissa = (h & 0x3ff) as f32;
    let magnitude = match exponent {
        0 => mantissa / 1024.0 / 16384.0,
        31 => return None,
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    };
    Some(if negative { -magnitude } else { magnitude })
}
