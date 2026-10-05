// SPDX-License-Identifier: GPL-3.0-only
// The CPU exception (vector 1) raised by a divide by zero when EMU_CON bit 2
// is set. Register map: AC79 SDK csfr.h (q32DSP(n) at 0x1eef000 + 0x200 n:
// EMU_CON +0xd0, EMU_MSG +0xd4; ICFG00 +0x100) and cpu/wl82/debug.c
// (request_irq(IRQ_EXCEPTION_IDX = 1, 7, ...), EMU_CON |= BIT(2) | BIT(8),
// emu_msg[] bit 2 "div0_err"). X0X ea3c665 reports that on a real FM-1 the
// trap fires on float divides too (a crash in its audio interrupt).
// Encodings from the JieLi objdump: e1f4 0120 = r0 = r2 / r1 (u),
// e1f4 0121 = (s), e1f6 0040 = r1_r0 = r5_r4 / r0 (u), e53f 0103 =
// r0 = r0 / r1 (f).
use fm1_emu::{
    bus::Bus,
    cpu::{Cpu, Fault},
    XIP,
};

const EMU_CON: u32 = 0x01ee_f0d0;
const EMU_MSG: u32 = 0x01ee_f0d4;
const ICFG00: u32 = 0x01ee_f100;
const VECTORS: u32 = 0x01c7_fe00;
const DIV0: u32 = 1 << 2;

/// A CPU running `words` at XIP, with a handler (`nop`s) 64 bytes in and
/// vector 1 pointing at it.
fn cpu(words: &[u16]) -> Cpu {
    let mut image: Vec<u16> = words.to_vec();
    image.resize(48, 0x0000);
    let mut c = Cpu::new(
        Bus::new(image.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    );
    c.bus.write(VECTORS + 4, handler(), 4).unwrap();
    c
}

fn handler() -> u32 {
    XIP + 64
}

/// Exception source 1 enabled at priority 7 (fm1_irq_init), the interrupt
/// controller on (icfg bit 8) and the divide-by-zero trap armed.
fn arm(c: &mut Cpu) {
    c.bus.write(ICFG00, 0xf0, 4).unwrap();
    c.sr[11] |= 0x100;
    c.bus.write(EMU_CON, DIV0 | 1 << 8, 4).unwrap();
}

fn f(v: f32) -> u32 {
    v.to_bits()
}

fn assert_trapped(c: &Cpu, at: u32) {
    assert_eq!(c.pc, handler(), "vector 1 handler");
    assert_eq!(c.sr[0], at, "reti holds the dividing instruction");
    assert_eq!(c.sr[11] & 0xff, 1, "icfg names source 1");
    assert_eq!(c.bus.read(EMU_MSG, 4).unwrap() & DIV0, DIV0);
    assert!(!c.interrupts_enabled);
    assert_eq!(c.exception_entries, 1);
}

#[test]
fn float_divide_by_zero_traps_when_emu_con_bit_2_is_set() {
    let mut c = cpu(&[0xe53f, 0x0103]);
    arm(&mut c);
    c.interrupts_enabled = true;
    c.r[0] = f(1.0);
    c.r[1] = f(0.0);
    c.step().unwrap();
    assert_trapped(&c, XIP);
    assert_eq!(c.r[0], f(1.0), "the destination is not written");
}

#[test]
fn negative_zero_and_zero_over_zero_also_trap() {
    for (dividend, divisor) in [(1.0f32, -0.0f32), (0.0, 0.0), (-3.5, 0.0)] {
        let mut c = cpu(&[0xe53f, 0x0103]);
        arm(&mut c);
        c.r[0] = f(dividend);
        c.r[1] = f(divisor);
        c.step().unwrap();
        assert_trapped(&c, XIP);
    }
}

#[test]
fn float_divide_by_zero_gives_infinity_with_the_trap_off() {
    // X0X clears EMU_CON bit 2: the compiled code expects IEEE results.
    let mut c = cpu(&[0xe53f, 0x0103]);
    c.bus.write(ICFG00, 0xf0, 4).unwrap();
    c.sr[11] |= 0x100;
    c.r[0] = f(-2.0);
    c.r[1] = f(0.0);
    c.step().unwrap();
    assert_eq!(c.r[0], f(f32::NEG_INFINITY));
    assert_eq!(c.pc, XIP + 4);
    assert_eq!(c.bus.read(EMU_MSG, 4).unwrap(), 0);
    assert_eq!(c.exception_entries, 0);
}

#[test]
fn a_nonzero_float_divisor_does_not_trap() {
    let mut c = cpu(&[0xe53f, 0x0103]);
    arm(&mut c);
    c.r[0] = f(1.0);
    c.r[1] = f(1e-40); // subnormal, not zero
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 4);
    assert_eq!(c.exception_entries, 0);
}

#[test]
fn integer_divides_by_zero_trap_when_armed() {
    for words in [[0xe1f4u16, 0x0120], [0xe1f4, 0x0121], [0xe1f6, 0x0040]] {
        let mut c = cpu(&words);
        arm(&mut c);
        c.r[0] = 0; // the long form's divisor
        c.r[1] = 0;
        c.r[2] = 1234;
        c.r[4] = 99;
        c.step().unwrap();
        assert_trapped(&c, XIP);
        assert_eq!(c.r[0], 0);
    }
}

#[test]
fn integer_divide_by_zero_with_the_trap_off_stops_explicitly() {
    // The quotient the divider leaves is unmeasured: fault, do not guess.
    let mut c = cpu(&[0xe1f4, 0x0120]);
    c.r[2] = 7;
    c.r[1] = 0;
    match c.step() {
        Err(Fault::Trap { pc, reason }) => {
            assert_eq!(pc, XIP);
            assert!(reason.contains("divide by zero"), "{reason}");
        }
        other => panic!("expected a divide-by-zero fault, got {other:?}"),
    }
}

#[test]
fn the_trap_is_taken_inside_an_interrupt_handler() {
    // X0X 0.2's crash was in master_process, inside the audio interrupt.
    let mut c = cpu(&[0x0000, 0x0000, 0xe53f, 0x0103]);
    arm(&mut c);
    c.step().unwrap(); // nop
                       // Pretend an interrupt handler is running: interrupts off.
    c.interrupts_enabled = false;
    c.step().unwrap(); // nop
    c.r[1] = 0;
    c.step().unwrap();
    assert_trapped(&c, XIP + 4);
}

#[test]
fn a_trap_with_vector_1_disabled_stops_explicitly() {
    let mut c = cpu(&[0xe53f, 0x0103]);
    c.sr[11] |= 0x100;
    c.bus.write(EMU_CON, DIV0, 4).unwrap();
    c.r[1] = 0;
    match c.step() {
        Err(Fault::Trap { pc, reason }) => {
            assert_eq!(pc, XIP);
            assert!(reason.contains("vector 1"), "{reason}");
        }
        other => panic!("expected an undeliverable trap, got {other:?}"),
    }
    assert_eq!(c.bus.read(EMU_MSG, 4).unwrap() & DIV0, DIV0);
}

#[test]
fn emu_msg_is_write_one_to_clear() {
    let mut c = cpu(&[0xe53f, 0x0103]);
    arm(&mut c);
    c.r[1] = 0;
    c.step().unwrap();
    c.bus.write(EMU_MSG, 0xffff_ffff, 4).unwrap();
    assert_eq!(c.bus.read(EMU_MSG, 4).unwrap(), 0);
}

#[test]
fn the_trap_inside_a_repeat_block_does_not_loop_back() {
    // rep 3 { r0 = r0 / r1 (f) } (8210: three times a four-byte body). An
    // interrupt waits for the block to finish; the exception does not, and
    // the repeat neither loops back over the handler nor survives into it.
    let mut c = cpu(&[0x8210, 0xe53f, 0x0103, 0x0000]);
    arm(&mut c);
    c.r[0] = f(1.0);
    c.r[1] = 0;
    c.step().unwrap(); // rep 3
    c.step().unwrap();
    assert_trapped(&c, XIP + 2);
    for _ in 0..4 {
        c.step().unwrap(); // the handler's nops run straight on
    }
    assert_eq!(c.pc, handler() + 8);
}

#[test]
fn each_core_uses_its_own_emu_con_bank() {
    // Core 1's q32DSP bank is 0x200 above core 0's: with only core 0 armed,
    // a divide on core 1 (cnum = 1) does not trap; it faults explicitly
    // (integer) or gives infinity (float).
    let mut c = cpu(&[0xe53f, 0x0103]);
    arm(&mut c);
    c.sr[6] = 1;
    c.bus.write(ICFG00 + 0x200, 0xf0, 4).unwrap();
    c.r[0] = f(1.0);
    c.r[1] = 0;
    c.step().unwrap();
    assert_eq!(c.r[0], f(f32::INFINITY));
    assert_eq!(c.exception_entries, 0);
    // Arm core 1's bank: now it traps and flags core 1's EMU_MSG.
    let mut c = cpu(&[0xe53f, 0x0103]);
    c.sr[6] = 1;
    c.sr[11] |= 0x100;
    c.bus.write(ICFG00 + 0x200, 0xf0, 4).unwrap();
    c.bus.write(EMU_CON + 0x200, DIV0, 4).unwrap();
    c.r[1] = 0;
    c.step().unwrap();
    assert_eq!(c.pc, handler());
    assert_eq!(c.bus.read(EMU_MSG + 0x200, 4).unwrap() & DIV0, DIV0);
    assert_eq!(c.bus.read(EMU_MSG, 4).unwrap(), 0);
}
