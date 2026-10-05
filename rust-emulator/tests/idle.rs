// SPDX-License-Identifier: GPL-3.0-only
// `idle` (0x0001) as wait-for-interrupt: the core stops issuing instructions
// until an interrupt is taken, each halted slot counting guest time exactly
// as one executed instruction; `run_steps` then skips the halted span up to
// the next device event in one jump, with the same guest-visible state.
use fm1_emu::{
    bus::Bus,
    cpu::Cpu,
    devices::{IRQ_CONFIG, TIMER5},
    RAM, SYSTEM_STACK, USER_STACK, XIP,
};

const STI: u16 = 0x0061;
const IDLE: u16 = 0x0001;
const NOP: u16 = 0x0000;
/// goto pc - 2 (back to the instruction before it).
const GOTO_BACK_ONE: u16 = 0x9ef7;
/// r0 += 1
const INC_R0: u16 = 0x21c0;
/// TIMER5 handler in RAM: acknowledge (CON = 0x4019), r2 += 1, rti.
const HANDLER: [u16; 8] = [
    0xffc0, 0x0900, 0x0001, // r0 = 0x00010900 (TIMER5)
    0xffc1, 0x4019, 0x0000, // r1 = 0x4019
    0x6081, // [r0] = r1
    0x0081, // rti (r2 += 1 is inserted before it)
];
const HANDLER_AT: u32 = RAM + 0x200;
/// TIMER5 prescaled by 4: an interrupt every 4 * PERIOD oscillator ticks.
const PERIOD: u32 = 150;

fn image(code: &[u16]) -> Vec<u8> {
    code.iter().flat_map(|h| h.to_le_bytes()).collect()
}

/// Primary core at XIP running `code`, a TIMER5 source wired to HANDLER.
fn machine(code: &[u16], mhz: u32) -> Cpu {
    let mut cpu = Cpu::new(Bus::new(image(code)).unwrap(), XIP);
    cpu.set_cpu_mhz(mhz).unwrap();
    let mut handler = HANDLER.to_vec();
    handler.insert(7, 0x21c2); // r2 += 1: wakes taken
    for (i, h) in handler.iter().enumerate() {
        cpu.bus.write(HANDLER_AT + 2 * i as u32, *h as u32, 2).unwrap();
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
fn state(cpu: &Cpu) -> String {
    let ram: Vec<u32> = (0..64)
        .map(|i| cpu.bus.read(USER_STACK - 128 + 4 * i, 4).unwrap())
        .collect();
    format!(
        "pc {:x} r {:x?} sr {:x?} steps {} ticks {} irqs {} halted {} timer {} {} ram {:x?} second {:x?}",
        cpu.pc,
        cpu.r,
        cpu.sr,
        cpu.steps,
        cpu.ticks(),
        cpu.irq_entries,
        cpu.halted(),
        cpu.bus.read(TIMER5 + 4, 4).unwrap(),
        cpu.bus.devices.timer5.pending,
        ram,
        cpu.secondary_pc(),
    )
}

#[test]
fn idle_halts_until_an_interrupt_and_resumes_after_it() {
    let mut cpu = machine(&[STI, IDLE, GOTO_BACK_ONE], 24);
    cpu.step().unwrap();
    assert_eq!(cpu.step().unwrap(), "idle");
    assert!(cpu.halted());
    assert_eq!(cpu.pc, XIP + 4);
    // Halted: the PC stays, time runs, no interrupt yet.
    for _ in 0..100 {
        assert_eq!(cpu.step().unwrap(), "halted");
    }
    assert_eq!((cpu.pc, cpu.r[2], cpu.steps), (XIP + 4, 0, 102));
    // The timer wakes it: the handler runs, returns after the idle, and
    // the loop goes back to idle.
    cpu.run_steps(4 * PERIOD as u64).unwrap();
    assert_eq!(cpu.r[2], 1);
    assert_eq!(cpu.irq_entries, 1);
    assert!(cpu.halted());
    assert_eq!(cpu.pc, XIP + 4);
}

#[test]
fn run_steps_jumps_to_the_next_event_with_the_state_of_single_steps() {
    for mhz in [24, 96, 312] {
        let code = [STI, IDLE, GOTO_BACK_ONE];
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
            assert_eq!(state(&single), state(&skipping), "{mhz} MHz, {count} steps");
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
fn skip_idle_is_bounded_by_its_budget_and_stops_before_the_event() {
    let mut cpu = machine(&[STI, IDLE, GOTO_BACK_ONE], 312);
    cpu.run_steps(2).unwrap();
    assert!(cpu.halted());
    assert_eq!(cpu.skip_idle(10), 10);
    assert_eq!(cpu.steps, 12);
    // Slot 13 runs the first tick, whose event the host's register writes
    // above scheduled.
    assert_eq!(cpu.skip_idle(u64::MAX), 0);
    cpu.step().unwrap();
    let skipped = cpu.skip_idle(u64::MAX);
    assert!(skipped > 0);
    // Stopped just before the slot that runs the timer event: nothing to
    // skip until a slot runs it.
    assert_eq!(cpu.skip_idle(u64::MAX), 0);
    assert_eq!(cpu.r[2], 0);
    cpu.step().unwrap();
    cpu.run_steps(40).unwrap();
    assert_eq!(cpu.r[2], 1);
}

#[test]
fn idle_with_interrupts_disabled_is_a_hint_and_does_not_hang() {
    let mut cpu = machine(&[IDLE, INC_R0, GOTO_BACK_ONE, NOP], 96);
    assert_eq!(cpu.step().unwrap(), "idle");
    assert!(!cpu.halted());
    assert_eq!(cpu.pc, XIP + 2);
    cpu.run_steps(10_000).unwrap();
    assert_eq!(cpu.steps, 10_001);
    assert!(cpu.r[0] > 1000);
    assert_eq!(cpu.irq_entries, 0);
}

#[test]
fn an_interrupt_already_pending_is_taken_at_once() {
    let mut cpu = machine(&[STI, IDLE, GOTO_BACK_ONE], 24);
    cpu.bus.devices.timer5.pending = true;
    cpu.step().unwrap(); // sti: the pending interrupt is taken here
    assert_eq!(cpu.pc, HANDLER_AT);
    cpu.run_steps(20).unwrap();
    assert_eq!(cpu.r[2], 1);
    assert!(cpu.halted());
}

const SECONDARY_AT: u32 = RAM + 0x400;
/// The secondary starts in interrupt context (the ROM handoff): it returns
/// from it with interrupts on (rti to SECONDARY_AT + 12), then idles. Its
/// interrupt controller stays off, so it never wakes.
const SECONDARY_IDLE: [u16; 8] = [
    0xffc0, 0x040c, 0x01c0, // r0 = SECONDARY_AT + 12
    0xe064, 0x0080, // reti = r0
    0x0081, // rti
    IDLE,
    GOTO_BACK_ONE,
];

/// The secondary core runs `code` from RAM; the primary runs `primary`.
fn dual(primary: &[u16], code: &[u16], mhz: u32) -> Cpu {
    let mut cpu = machine(primary, mhz);
    let entry = SECONDARY_AT;
    for (i, h) in code.iter().enumerate() {
        cpu.bus.write(entry + 2 * i as u32, *h as u32, 2).unwrap();
    }
    cpu.bus.write(0x01c7_fff8, entry, 4).unwrap();
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
    assert_eq!(state(&single), state(&skipping));
    assert_eq!(skipping.secondary_pc(), Some(SECONDARY_AT + 14));
    assert!(skipping.r[0] > 1000);
    assert_eq!(skipping.idle_skipped, 0);
}

#[test]
fn both_cores_halted_skip_together_and_the_timer_wakes_the_primary() {
    let code = [STI, IDLE, GOTO_BACK_ONE];
    let mut single = dual(&code, &SECONDARY_IDLE, 312);
    single.idle_skip = false;
    let mut skipping = dual(&code, &SECONDARY_IDLE, 312);
    for count in [3, 1000, 4321, 60_000] {
        for _ in 0..count {
            single.step().unwrap();
        }
        skipping.run_steps(count).unwrap();
        assert_eq!(state(&single), state(&skipping), "{count} steps");
    }
    assert!(skipping.r[2] > 5);
    assert_eq!(skipping.secondary_pc(), Some(SECONDARY_AT + 14));
    assert!(skipping.idle_skipped > skipping.steps / 2);
}
