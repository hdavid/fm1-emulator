// SPDX-License-Identifier: GPL-3.0-only
// The corex2 performance counters (AC79 SDK csfr.h, corex2(0) at 0x1eee000:
// C0_IF/RD/WR_UACNT L/H 0x200-0x214, C0_TL_CKCNT L/H 0x218/0x21c, the C1
// set at 0x220-0x23c, DBG_CON 0x344). The SDK's cpu_effic_init sets DBG_CON
// bits 0-2 (core 0) or 8-10 (core 1) and then reads TL_CKCNT; X0X measured
// on a real FM-1 that TL_CKCNT does not count with DBG_CON 0. TL_CKCNT
// counts CPU clock cycles; the emulator has no cache or bus stalls, so the
// three stall counters only hold what is written.
use fm1_emu::{bus::Bus, cpu::Cpu, XIP};

const C0_IF_UACNTL: u32 = 0x01ee_e200;
const C0_TL_CKCNTL: u32 = 0x01ee_e218;
const C0_TL_CKCNTH: u32 = 0x01ee_e21c;
const C1_TL_CKCNTL: u32 = 0x01ee_e238;
const DBG_CON: u32 = 0x01ee_e344;

/// A CPU running through 16 K `nop`s.
fn cpu() -> Cpu {
    let words = vec![0x0000u16; 16384];
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}

fn run(c: &mut Cpu, steps: u64) {
    for _ in 0..steps {
        c.step().unwrap();
    }
}

#[test]
fn the_cycle_counter_is_frozen_while_dbg_con_is_zero() {
    let mut c = cpu();
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 0);
    run(&mut c, 1000);
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 0);
}

#[test]
fn the_cycle_counter_counts_cpu_clock_cycles_once_enabled() {
    let mut c = cpu();
    run(&mut c, 100);
    c.bus.write(DBG_CON, 7, 4).unwrap();
    assert_eq!(c.bus.read(DBG_CON, 4).unwrap(), 7);
    run(&mut c, 1000);
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 1000);
    // At 96 MHz four cycles pass per oscillator tick.
    let mut c = cpu();
    c.set_cpu_mhz(96).unwrap();
    c.bus.write(DBG_CON, 1, 4).unwrap();
    run(&mut c, 4000);
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 4000);
    // Disabling freezes the count.
    c.bus.write(DBG_CON, 0, 4).unwrap();
    run(&mut c, 4000);
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 4000);
}

#[test]
fn the_counter_is_64_bits_and_writable() {
    let mut c = cpu();
    c.bus.write(C0_TL_CKCNTL, 0xffff_fff0, 4).unwrap();
    c.bus.write(C0_TL_CKCNTH, 2, 4).unwrap();
    c.bus.write(DBG_CON, 7, 4).unwrap();
    run(&mut c, 32);
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 0x10);
    assert_eq!(c.bus.read(C0_TL_CKCNTH, 4).unwrap(), 3);
}

#[test]
fn core_1_has_its_own_counter_and_enable_bits() {
    let mut c = cpu();
    c.bus.write(DBG_CON, 7 << 8, 4).unwrap();
    run(&mut c, 50);
    assert_eq!(c.bus.read(C0_TL_CKCNTL, 4).unwrap(), 0);
    assert_eq!(c.bus.read(C1_TL_CKCNTL, 4).unwrap(), 50);
}

#[test]
fn stall_counters_hold_what_is_written() {
    let mut c = cpu();
    c.bus.write(DBG_CON, 7, 4).unwrap();
    for (i, a) in (C0_IF_UACNTL..C0_TL_CKCNTL).step_by(4).enumerate() {
        c.bus.write(a, 0x100 + i as u32, 4).unwrap();
    }
    run(&mut c, 10);
    for (i, a) in (C0_IF_UACNTL..C0_TL_CKCNTL).step_by(4).enumerate() {
        assert_eq!(c.bus.read(a, 4).unwrap(), 0x100 + i as u32);
    }
}
