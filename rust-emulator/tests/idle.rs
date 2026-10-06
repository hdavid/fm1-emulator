// SPDX-License-Identifier: GPL-3.0-only
// Skipping halted time: a core halted by `idle` issues nothing until an
// interrupt enters, but each halted slot still takes guest time. `run_steps`
// jumps over a span in which every issuing core is halted, up to the next
// device event, and must leave exactly the state single steps leave.
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    devices::{IRQ_CONFIG, TIMER5},
    RAM, SYSTEM_STACK, USER_STACK, XIP,
};

const STI: u16 = 0x0061;
const IDLE: u16 = 0x0001;
/// goto pc - 2 (back to the instruction before it).
const GOTO_BACK_ONE: u16 = 0x9ef7;
/// The idle loop of a firmware main loop: idle, then (the measured
/// four-slot wake latency) four more instructions before going back.
const IDLE_LOOP: [u16; 7] = [STI, IDLE, NOP, NOP, NOP, NOP, 0x9af7];
const NOP: u16 = 0x0000;
/// r0 += 1
const INC_R0: u16 = 0x21c0;
/// TIMER5 handler in RAM: acknowledge (CON = 0x4019), r2 += 1, rti.
const HANDLER: [u16; 9] = [
    0xffc0, 0x0900, 0x0001, // r0 = 0x00010900 (TIMER5)
    0xffc1, 0x4019, 0x0000, // r1 = 0x4019
    0x6081, // [r0] = r1
    0x21c2, // r2 += 1: wakes taken
    0x0081, // rti
];
const HANDLER_AT: u32 = RAM + 0x200;
/// TIMER5 on the oscillator, prescaled by 4: an interrupt every
/// 4 * PERIOD oscillator ticks.
const PERIOD: u32 = 150;

fn image(code: &[u16]) -> Vec<u8> {
    code.iter().flat_map(|h| h.to_le_bytes()).collect()
}

/// Primary core at XIP running `code`, a TIMER5 source wired to HANDLER.
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
    cpu.sr[11] = 0x100;
    cpu.sr[13] = SYSTEM_STACK;
    cpu.sr[14] = USER_STACK;
    cpu
}

/// Everything a guest or a device can observe.
fn state(cpu: &mut Cpu) -> String {
    let ram: Vec<u32> = (0..64)
        .map(|i| cpu.bus.read(USER_STACK - 128 + 4 * i, 4).unwrap())
        .collect();
    format!(
        "pc {:x} r {:x?} sr {:x?} steps {} cores {:?} idle {} ticks {} irqs {} halted {} timer {} {} ram {:x?} second {:x?}",
        cpu.pc,
        cpu.r,
        cpu.sr,
        cpu.steps,
        cpu.core_steps,
        cpu.idle_slots,
        cpu.bus.oscillator_ticks(),
        cpu.irq_entries,
        cpu.halted(),
        cpu.bus.read(TIMER5 + 4, 4).unwrap(),
        cpu.bus.devices.timer5.pending,
        ram,
        cpu.secondary_pc(),
    )
}

#[test]
fn run_steps_wakes_on_the_timer_and_returns_to_idle() {
    let mut cpu = machine(&IDLE_LOOP, 24);
    cpu.step().unwrap();
    assert_eq!(cpu.step().unwrap(), "idle");
    assert!(cpu.halted());
    for _ in 0..100 {
        assert_eq!(cpu.step().unwrap(), "idle_wait");
    }
    assert_eq!((cpu.pc, cpu.r[2], cpu.steps), (XIP + 4, 0, 102));
    cpu.run_steps(4 * PERIOD as u64).unwrap();
    assert_eq!(cpu.r[2], 1);
    assert_eq!(cpu.irq_entries, 1);
    assert!(cpu.halted());
    assert_eq!(cpu.pc, XIP + 4);
}

#[test]
fn run_steps_jumps_to_the_next_event_with_the_state_of_single_steps() {
    for mhz in [24, 96, 312, 7] {
        let code = IDLE_LOOP;
        let mut single = machine(&code, mhz);
        single.idle_skip = false;
        let mut skipping = machine(&code, mhz);
        // Several wakes, and budgets that end inside a halted span, on a
        // wake, and in the middle of a handler.
        for count in [1, 7, 599, 600, 601, 5000, 12_345, 100_000] {
            for _ in 0..count {
                single.step().unwrap();
            }
            skipping.run_steps(count).unwrap();
            assert_eq!(
                state(&mut single),
                state(&mut skipping),
                "{mhz} MHz, {count} steps"
            );
        }
        assert!(skipping.r[2] > 10, "{mhz} MHz: {} wakes", skipping.r[2]);
        assert_eq!(single.idle_skipped, 0);
        assert!(
            skipping.idle_skipped > skipping.steps / 2,
            "{mhz} MHz: {} of {} skipped",
            skipping.idle_skipped,
            skipping.steps
        );
    }
}

#[test]
fn skipping_is_bounded_and_stops_before_the_event() {
    let mut cpu = machine(&IDLE_LOOP, 312);
    // Through the first wake: the handler's acknowledge (a register write)
    // makes the following tick exact, then the core halts again.
    while cpu.r[2] == 0 || !cpu.halted() {
        cpu.step().unwrap();
    }
    // Past the exact tick that write scheduled (13 calls per tick).
    cpu.run_steps(26).unwrap();
    assert_eq!(cpu.skip_idle_calls(10), 10);
    assert!(cpu.skip_idle_calls(u64::MAX) > 1000);
    // Stopped just before the call that runs the timer event.
    assert_eq!(cpu.skip_idle_calls(u64::MAX), 0);
    assert_eq!(cpu.r[2], 1);
    cpu.step().unwrap();
    cpu.run_steps(40).unwrap();
    assert_eq!(cpu.r[2], 2);
}

#[test]
fn a_halted_core_with_a_deliverable_interrupt_is_not_skipped() {
    let mut cpu = machine(&IDLE_LOOP, 24);
    cpu.run_steps(2).unwrap();
    cpu.bus.devices.timer5.pending = true;
    assert_eq!(cpu.skip_idle_calls(u64::MAX), 0);
    cpu.run_steps(20).unwrap();
    assert_eq!(cpu.r[2], 1);
}

#[test]
fn step_next_delivers_interrupts_like_step() {
    // A busy loop (r0 += 1) that the timer interrupts, at several clocks.
    for mhz in [24, 96, 7] {
        let code = [STI, INC_R0, GOTO_BACK_ONE];
        let mut single = machine(&code, mhz);
        let mut next = machine(&code, mhz);
        for count in [1, 3, 599, 600, 601, 20_000] {
            for _ in 0..count {
                single.step().unwrap();
                next.step_next().unwrap();
            }
            assert_eq!(
                state(&mut single),
                state(&mut next),
                "{mhz} MHz, {count} steps"
            );
        }
        assert!(next.r[2] > 5, "{mhz} MHz: {} interrupts", next.r[2]);
    }
    // A source made pending through the public fields: `step` sees it.
    let mut cpu = machine(&[STI, INC_R0, GOTO_BACK_ONE], 24);
    for _ in 0..3 {
        cpu.step_next().unwrap();
    }
    cpu.bus.devices.timer5.pending = true;
    cpu.step().unwrap();
    assert_eq!(cpu.irq_entries, 1);
}

const SECONDARY_AT: u32 = RAM + 0x400;
/// The secondary starts in interrupt context (the ROM handoff): it returns
/// from it with interrupts on (rti to SECONDARY_AT + 12), then idles. Its
/// interrupt controller stays off, so it never wakes.
const SECONDARY_IDLE: [u16; 8] = [
    0xffc0,
    0x040c,
    0x01c0, // r0 = SECONDARY_AT + 12
    0xe064,
    0x0080, // reti = r0
    0x0081, // rti
    IDLE,
    GOTO_BACK_ONE,
];

/// The secondary core runs `code` from RAM; the primary runs `primary`.
fn dual(primary: &[u16], code: &[u16], mhz: u32) -> Cpu {
    let mut cpu = machine(primary, mhz);
    for (i, h) in code.iter().enumerate() {
        cpu.bus
            .write(SECONDARY_AT + 2 * i as u32, *h as u32, 2)
            .unwrap();
    }
    cpu.bus.write(0x01c7_fff8, SECONDARY_AT, 4).unwrap();
    cpu.bus.write(0x1eee004, 8, 4).unwrap();
    cpu
}

#[test]
fn each_core_idles_independently() {
    // The secondary halts (interrupts on, no source): the primary keeps
    // counting, so nothing is skipped and both match single steps.
    let mut single = dual(&[INC_R0, GOTO_BACK_ONE], &SECONDARY_IDLE, 96);
    single.idle_skip = false;
    let mut skipping = dual(&[INC_R0, GOTO_BACK_ONE], &SECONDARY_IDLE, 96);
    for _ in 0..5000 {
        single.step().unwrap();
    }
    skipping.run_steps(5000).unwrap();
    assert_eq!(state(&mut single), state(&mut skipping));
    assert_eq!(skipping.secondary_pc(), Some(SECONDARY_AT + 14));
    assert_eq!(skipping.secondary_halted(), Some(true));
    assert!(skipping.r[0] > 1000);
    assert_eq!(skipping.idle_skipped, 0);
}

#[test]
fn both_cores_halted_skip_together_and_the_timer_wakes_the_primary() {
    let code = IDLE_LOOP;
    let mut single = dual(&code, &SECONDARY_IDLE, 312);
    single.idle_skip = false;
    let mut skipping = dual(&code, &SECONDARY_IDLE, 312);
    for count in [3, 1000, 4321, 60_000] {
        for _ in 0..count {
            single.step().unwrap();
        }
        skipping.run_steps(count).unwrap();
        assert_eq!(state(&mut single), state(&mut skipping), "{count} steps");
    }
    assert!(skipping.r[2] > 5);
    assert_eq!(skipping.secondary_pc(), Some(SECONDARY_AT + 14));
    assert!(skipping.idle_skipped > skipping.steps / 2);
}

#[test]
fn a_skipped_call_with_both_cores_halted_issues_one_slot_per_core() {
    // The cores share hardware time: a call of `step` with both running is
    // one issue slot per core and one instruction's worth of guest time.
    let code = IDLE_LOOP;
    let mut cpu = dual(&code, &SECONDARY_IDLE, 312);
    cpu.run_steps(20).unwrap();
    assert!(cpu.halted());
    assert_eq!(cpu.secondary_halted(), Some(true));
    while cpu.skip_idle_calls(1) == 0 {
        cpu.step().unwrap();
    }
    let (steps, cores, idle) = (cpu.steps, cpu.core_steps, cpu.idle_slots);
    let ticks = cpu.bus.oscillator_ticks();
    let skipped = cpu.skip_idle_calls(1000);
    assert_eq!(skipped, 1000);
    assert_eq!(cpu.steps, steps + 2000);
    assert_eq!(cpu.core_steps, [cores[0] + 1000, cores[1] + 1000]);
    assert_eq!(cpu.idle_slots, idle + 2000);
    // 312 MHz: 13 instructions per 24 MHz oscillator tick.
    assert!((cpu.bus.oscillator_ticks() - ticks).abs_diff(1000 / 13) <= 1);
}
