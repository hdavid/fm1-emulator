// SPDX-License-Identifier: GPL-3.0-only
// Packed 16-bit ("SIMD") forms of pi32v2 that JieLi's clang assembles for
// -mcpu=r3 (the AC79 SDK's CPU) without any feature flag. The compiler never
// emits them (vector types are scalarised), so firmware uses them through
// inline asm (SLOOP hal/fm1_simd.h).
//
// Encodings: the vendor objdump, decoding every extension halfword of each
// first halfword below. Fields of the extension x: rD 15:12, rB 11:8, rA
// 7:4, then four mode bits.
//
//   e500       rD.p = rA.q +/- rB.r     x bit 0 subtract, bit 1 rB.h, bit 2 rA.h, bit 3 rD.h
//   e541       rD.p = rA.q * rB.r (ssat)       same half bits (h bit 1: x2 -> e543)
//   e551/e553  rD = rA.q * rB.r (ssat[,x2])    x bit 1 rB.h, bit 2 rA.h
//   e404       rD = pack(rA.q, rB.r)           x bit 1 rB.h, bit 2 rA.h
//   e511+4k    rD = rA.a,rA.b +|+ rB.c,rB.d (ssat)      (+|- -|+ -|- : x bits 2, 3)
//   e561+4k    rD = rA.a,rA.b *|* rB.c,rB.d (ssat[,x2]) (e563+4k: x2)
//              rA's halves: x bit 1 (first), bit 0 (second); rB's: h bit 3
//              (first), bit 2 (second).
//
// Semantics are INFERRED (no JieLi documentation found; the toolchain only
// names the records, e.g. SIMD_QMUL16X2S_hhh, SIMD_QADDS16X2_hhh): they follow
// the Blackfin conventions the syntax copies (pack and the dual forms put
// their first element in the high half; ssat saturates to signed 16 or 32
// bits; x2 is the Q15 fractional product, (a * b) << 1, of which a 16-bit
// destination takes the high half, truncated). Only the real FM-1 can confirm
// them: SLOOP's SIMD PROBE (FELUCCA_SIMD_PROBE) executes each form on inputs
// that tell these choices apart.

/// The half of `r` selected by `high`, sign-extended.
pub(crate) fn half(r: u32, high: bool) -> i32 {
    (if high { r >> 16 } else { r } as u16) as i16 as i32
}

/// `r` with the half selected by `high` replaced by the low 16 bits of `v`.
pub(crate) fn set_half(r: u32, high: bool, v: i32) -> u32 {
    let v = v as u32 & 0xffff;
    if high {
        (r & 0xffff) | (v << 16)
    } else {
        (r & 0xffff_0000) | v
    }
}

pub(crate) fn sat16(v: i64) -> i32 {
    v.clamp(-32768, 32767) as i32
}

pub(crate) fn sat32(v: i64) -> u32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32 as u32
}

/// 16 x 16 signed product into 16 bits: integer (saturated) or, with x2, the
/// Q15 product (a * b) >> 15 truncated and saturated.
pub(crate) fn mul16(a: i32, b: i32, x2: bool) -> i32 {
    let p = a as i64 * b as i64;
    sat16(if x2 { p >> 15 } else { p })
}

/// 16 x 16 signed product into 32 bits; with x2 doubled and saturated.
pub(crate) fn mul32(a: i32, b: i32, x2: bool) -> u32 {
    let p = a as i64 * b as i64;
    sat32(if x2 { p << 1 } else { p })
}

/// The two lanes of a dual form: (first element, second element), each the
/// selected half of its register.
pub(crate) fn lanes(r: u32, first_high: bool, second_high: bool) -> (i32, i32) {
    (half(r, first_high), half(r, second_high))
}

/// A dual result: the first lane into the high half.
pub(crate) fn join(first: i32, second: i32) -> u32 {
    ((first as u32 & 0xffff) << 16) | (second as u32 & 0xffff)
}
