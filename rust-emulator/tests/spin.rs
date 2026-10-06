// SPDX-License-Identifier: GPL-3.0-only
// Spin loops (src/spin.rs): a core busy-waiting on a memory flag (stock
// FM-1: CPU0 on the LCD DMA flag, CPU1 on its job mailbox while resetting
// TIMER5) is jumped over like a halted one, with the guest state of single
// steps. Encodings checked with JieLi's objdump (scripts/pi32-objdump.sh).
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    devices::{IRQ_CONFIG, TIMER4, TIMER5},
    RAM, SYSTEM_STACK, USER_STACK, XIP,
};

const STI: u16 = 0x0061;
const IDLE: u16 = 0x0001;
const NOP: u16 = 0x0000;
const FLAG: u32 = RAM + 0x100;
/// TIMER5 handler in RAM: acknowledge (CON = 0x4019), r2 += 1,
/// [r9+0] = 0x0 (clear the flag), rti.
const HANDLER: [u16; 11] = [
    0xffc0, 0x0900, 0x0001, // r0 = 0x00010900 (TIMER5)
    0xffc1, 0x4019, 0x0000, // r1 = 0x4019
    0x6081, // [r0] = r1
    0x21c2, // r2 += 1
    0xea40, 0x9000, // [r9+0] = 0x0
    0x0081, // rti
];
const HANDLER_AT: u32 = RAM + 0x200;
/// TIMER5 prescaled by 4: an interrupt every 4 * PERIOD oscillator ticks.
const PERIOD: u32 = 150;

/// sti; loop: r0 = [r9+0]; if (r0 != 0) goto loop; r3 += 1;
/// [r9+0] = r3; goto loop. The handler ends each wait.
const POLL: [u16; 8] = [STI, 0xecd0, 0x0090, 0x5df0, 0x21c3, 0xecd0, 0x3091, 0x99f7];
/// As POLL, resetting TIMER4's counter and reading it on every pass (as
/// stock CPU1 does with TIMER5): sti; loop: [r8+0] = 0x0; r0 = [r8+0];
/// r1 = [r9+0]; if (r1 != 0) goto loop; r3 += 1; [r9+0] = r3; goto loop.
const TIMED_POLL: [u16; 12] = [
    STI, 0xea40, 0x8000, 0xecd0, 0x0080, 0xecd0, 0x1090, 0x59f1, 0x21c3, 0xecd0, 0x3091, 0x95f7,
];

fn image(code: &[u16]) -> Vec<u8> {
    code.iter().flat_map(|h| h.to_le_bytes()).collect()
}

/// Primary core at XIP running `code`, TIMER5 interrupts clearing FLAG,
/// TIMER4 running free (no interrupt), r8 = TIMER4's counter, r9 = FLAG.
fn machine(code: &[u16], mhz: u32) -> Cpu {
    let mut cpu = Cpu::new(Bus::new(image(code)).unwrap(), XIP);
    cpu.set_cpu_mhz(mhz).unwrap();
    for (i, h) in HANDLER.iter().enumerate() {
        cpu.bus
            .write(HANDLER_AT + 2 * i as u32, *h as u32, 2)
            .unwrap();
    }
    cpu.bus.write(0x01c7_fe00 + 63 * 4, HANDLER_AT, 4).unwrap();
    cpu.bus.write(IRQ_CONFIG + 7 * 4, 0x3000_0000, 4).unwrap();
    cpu.bus.write(TIMER5 + 8, PERIOD, 4).unwrap();
    cpu.bus.write(TIMER5, 0x4019, 4).unwrap();
    cpu.bus.write(TIMER4 + 8, u32::MAX, 4).unwrap();
    cpu.bus.write(TIMER4, 0x19, 4).unwrap();
    cpu.bus.write(FLAG, 1, 4).unwrap();
    cpu.r[8] = TIMER4 + 4;
    cpu.r[9] = FLAG;
    cpu.spin_log = std::env::var("FM1_SPIN_LOG").is_ok_and(|v| v == "1");
    cpu.sr[11] = 0x100;
    cpu.sr[13] = SYSTEM_STACK;
    cpu.sr[14] = USER_STACK;
    cpu
}

const SECONDARY_AT: u32 = RAM + 0x400;
/// The secondary core: r8 = TIMER4's counter, r9 = FLAG, leave the ROM
/// handoff's interrupt context (rti to the loop), then the timed poll:
/// loop: [r8+0] = 0x0; r0 = [r8+0]; r1 = [r9+0]; if (r1 != 0) goto loop;
/// r2 += 1; goto loop (counting while the flag is clear).
const SECONDARY_POLL: [u16; 21] = [
    0xffc8, 0x0804, 0x0001, // r8 = 0x10804
    0xffc9, 0x0100, 0x01c0, // r9 = FLAG
    0xffc0, 0x0418, 0x01c0, // r0 = SECONDARY_AT + 0x18
    0xe064, 0x0080, // reti = r0
    0x0081, // rti
    0xea40, 0x8000, 0xecd0, 0x0080, 0xecd0, 0x1090, 0x59f1, // the poll
    0x21c2, 0x97f7, // r2 += 1; goto loop
];
/// The primary idles; after each wake (the handler cleared FLAG) it sets
/// FLAG again: sti; idle; four nops (the measured wake latency: the
/// interrupt enters four slots after the wake); r3 += 1; [r9+0] = r3;
/// goto idle.
const PRIMARY_IDLE: [u16; 10] = [
    STI, IDLE, NOP, NOP, NOP, NOP, 0x21c3, 0xecd0, 0x3091, 0x97f7,
];

fn dual(mhz: u32) -> Cpu {
    let mut cpu = machine(&PRIMARY_IDLE, mhz);
    for (i, h) in SECONDARY_POLL.iter().enumerate() {
        cpu.bus
            .write(SECONDARY_AT + 2 * i as u32, *h as u32, 2)
            .unwrap();
    }
    cpu.bus.write(0x01c7_fff8, SECONDARY_AT, 4).unwrap();
    cpu.bus.write(0x1eee004, 8, 4).unwrap();
    cpu
}

/// Everything a guest or a device can observe.
fn state(cpu: &Cpu) -> String {
    let ram: Vec<u32> = (0..64)
        .map(|i| cpu.bus.read(USER_STACK - 128 + 4 * i, 4).unwrap())
        .collect();
    format!(
        "pc {:x} r {:x?} sr {:x?} steps {} cores {:?} ticks {} irqs {} halted {} timers {:x?} flag {} ram {:x?} second {:x?}",
        cpu.pc,
        cpu.r,
        cpu.sr,
        cpu.steps,
        cpu.core_steps,
        cpu.bus.oscillator_ticks(),
        cpu.irq_entries,
        cpu.halted(),
        [0, 4, 8].map(|offset| (
            cpu.bus.read(TIMER4 + offset, 4).unwrap(),
            cpu.bus.read(TIMER5 + offset, 4).unwrap()
        )),
        cpu.bus.read(FLAG, 4).unwrap(),
        ram,
        cpu.secondary_pc(),
    )
}

/// Run `make` single-stepped (no skipping at all) and with `run_steps`,
/// comparing the state after each count; returns the skipping machine.
fn compare(make: impl Fn() -> Cpu, counts: &[u64], label: &str) -> Cpu {
    let mut single = make();
    single.idle_skip = false;
    let mut skipping = make();
    for &count in counts {
        for _ in 0..count {
            single.step().unwrap();
        }
        skipping.run_steps(count).unwrap();
        assert_eq!(state(&single), state(&skipping), "{label}, {count} steps");
    }
    assert_eq!(single.spin_skipped, [0; 2]);
    skipping
}

const COUNTS: [u64; 10] = [1, 7, 99, 599, 600, 601, 5000, 12_345, 100_000, 250_000];

#[test]
fn a_memory_poll_is_jumped_over_with_the_state_of_single_steps() {
    // At 7 MHz an instruction takes several oscillator ticks: spans
    // between events are short, but the state must still be exact.
    for mhz in [24, 96, 192, 312, 100, 7] {
        let cpu = compare(|| machine(&POLL, mhz), &COUNTS, &format!("{mhz} MHz"));
        assert!(cpu.r[2] > 10, "{mhz} MHz: {} wakes", cpu.r[2]);
        assert!(
            mhz < 24 || cpu.spin_skipped[0] > cpu.steps / 2,
            "{mhz} MHz: {} of {} steps jumped",
            cpu.spin_skipped[0],
            cpu.steps
        );
    }
}

#[test]
fn a_poll_that_resets_and_reads_a_timer_replays_it() {
    // At 7 MHz an instruction takes several oscillator ticks: spans
    // between events are short, but the state must still be exact.
    for mhz in [24, 96, 192, 312, 100, 7] {
        let cpu = compare(|| machine(&TIMED_POLL, mhz), &COUNTS, &format!("{mhz} MHz"));
        assert!(cpu.r[2] > 10, "{mhz} MHz: {} wakes", cpu.r[2]);
        assert!(
            mhz < 24 || cpu.spin_skipped[0] > cpu.steps / 4,
            "{mhz} MHz: {} of {} steps jumped",
            cpu.spin_skipped[0],
            cpu.steps
        );
    }
}

#[test]
fn a_secondary_polling_while_the_primary_idles_is_jumped_over() {
    // Stock FM-1's shape: CPU1 polls a mailbox while CPU0 sleeps; the
    // primary's handler clears the flag, CPU1 counts, CPU0 sets it again.
    for mhz in [96, 192, 312, 100] {
        let cpu = compare(|| dual(mhz), &COUNTS, &format!("{mhz} MHz"));
        assert!(cpu.r[3] > 10, "{mhz} MHz: {} wakes", cpu.r[3]);
        assert!(
            cpu.spin_skipped[1] > cpu.steps / 4,
            "{mhz} MHz: {} of {} steps jumped",
            cpu.spin_skipped[1],
            cpu.steps
        );
    }
}

#[test]
fn a_counting_loop_is_not_a_spin_loop() {
    // r0 += 1; goto back: the state changes on every pass.
    let cpu = compare(|| machine(&[0x21c0, 0x9ef7], 96), &[100_000], "counter");
    assert_eq!(cpu.spin_skipped, [0; 2]);
}

#[test]
fn spin_skip_off_steps_every_instruction() {
    let mut cpu = machine(&POLL, 192);
    cpu.spin_skip = false;
    cpu.run_steps(100_000).unwrap();
    assert_eq!(cpu.spin_skipped, [0; 2]);
    assert!(cpu.r[2] > 10);
}
