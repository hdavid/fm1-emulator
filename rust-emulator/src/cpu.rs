// SPDX-License-Identifier: GPL-3.0-only
// Probe decodings mirror emu.py, checked against vendor objdump and the FM-1.
// Startup-only additions use the pinned Quarkslab pi32v2 reference; see README.
use crate::code_cache::Operands;
use crate::decode::{decode, decode_wide, is_parallel, primary, skip_length, Op};
use crate::devices::OSC_TICKS_PER_INSTRUCTION;
use crate::{
    bus::{AccessFault, Bus},
    PROBE_RETURN, RESULT, USER_STACK,
};
use std::{fmt, io::Write};

#[derive(Debug, PartialEq, Eq)]
pub enum Fault {
    Access { pc: u32, fault: AccessFault },
    Unsupported { pc: u32, word: u16 },
    Limit { pc: u32, limit: u64 },
    Preservation,
    Trace(String),
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    #[test]
    fn lock_instructions_change_ownership_without_changing_registers() {
        let mut c = Cpu::new(Bus::new(vec![0x41, 0, 0x40, 0]).unwrap(), crate::XIP);
        let before = c.r;
        assert_eq!(c.step().unwrap(), "lockset");
        assert!(c.bus_locked);
        assert_eq!(c.step().unwrap(), "lockclr");
        assert!(!c.bus_locked);
        assert_eq!(c.r, before);
    }

    #[test]
    fn secondary_uses_the_guest_handoff_and_serializes_bus_locks() {
        let mut c = Cpu::new(Bus::new(vec![0; 32]).unwrap(), crate::XIP);
        let entry = crate::RAM + 512;
        c.bus.write(entry, 0x00400041, 4).unwrap(); // lockset; lockclr
        c.bus.write(0x01c7fff8, entry, 4).unwrap();
        c.bus.write(0x1eee004, 8, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.pc, crate::XIP + 2);
        let secondary = c.secondary.as_ref().unwrap();
        assert_eq!(secondary.pc, entry + 2);
        assert_eq!(secondary.sr[6], 1);
        assert!(secondary.bus_locked);
        c.step().unwrap(); // Secondary owns the bus until LOCKCLR.
        assert_eq!(c.pc, crate::XIP + 2);
        assert!(!c.secondary.as_ref().unwrap().bus_locked);
        c.step().unwrap();
        assert_eq!(c.pc, crate::XIP + 4);
        c.bus.write(0x1eee004, 2, 4).unwrap();
        c.step().unwrap();
        assert!(c.secondary.is_none());
    }

    #[test]
    fn software_interrupt_latches_are_shared_and_masks_are_per_core() {
        use crate::devices::IRQ_CONFIG;
        let mut c = Cpu::new(Bus::new(vec![0; 32]).unwrap(), crate::XIP);
        c.bus
            .write(IRQ_CONFIG + 0x200 + 15 * 4, 3 << 28, 4)
            .unwrap();
        c.bus.write(0x1eef3a0, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 0), None);
        assert_eq!(c.bus.pending_irq_for(0x100, 1), Some(127));
        assert_eq!(c.bus.read(0x1eef38c, 4).unwrap(), 0x80000000);
        c.bus.write(0x1eef3a4, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 1), None);
        c.bus.write(0x1eef1a0, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 1), Some(127));
        assert_eq!(c.bus.read(0x1eef18c, 4).unwrap(), 0);
        c.bus.write(IRQ_CONFIG + 15 * 4, 3 << 28, 4).unwrap();
        assert_eq!(c.bus.read(0x1eef18c, 4).unwrap(), 0x80000000);
        c.bus.write(0x1eef3a4, 128, 4).unwrap();
        assert_eq!(c.bus.pending_irq_for(0x100, 0), None);
    }

    #[test]
    fn run_steps_matches_single_steps_and_stops_at_a_fault() {
        // r0 = 5; loop: r0 += -1; if r0 != 0 goto loop; then run off the
        // end of the image (a fetch fault).
        let program = vec![0x40, 0x25, 0xf8, 0x3f, 0xf0, 0x5e];
        let mut single = Cpu::new(Bus::new(program.clone()).unwrap(), crate::XIP);
        let mut batched = Cpu::new(Bus::new(program).unwrap(), crate::XIP);
        let mut single_fault = None;
        for _ in 0..40 {
            if let Err(fault) = single.step() {
                single_fault = Some(fault);
                break;
            }
        }
        assert_eq!(batched.run_steps(40).err(), single_fault);
        assert!(single_fault.is_some());
        assert_eq!(single.steps, 11);
        assert_eq!(
            (batched.pc, batched.r, batched.steps),
            (single.pc, single.r, single.steps)
        );
    }

    #[test]
    fn add_immediate_sets_the_carry_for_a_following_addc() {
        // r12 = r10 + 32; r0 = 0; r13 = r11 + r0 + c (compiler 64-bit add,
        // SLOOP's dx7_tables_init); the carry must not leak from before.
        let program = vec![0x0c, 0xe1, 0x20, 0xa0, 0x40, 0x20, 0xb8, 0xe0, 0xb0, 0xd0];
        for (low, high) in [(0u32, 0u32), (0xffff_ffe0, 1)] {
            let mut c = Cpu::new(Bus::new(program.clone()).unwrap(), crate::XIP);
            c.set_carry(true);
            c.r[10] = low;
            c.r[11] = 0;
            for _ in 0..3 {
                c.step().unwrap();
            }
            assert_eq!(c.r[12], low.wrapping_add(32));
            assert_eq!(c.r[13], high);
        }
    }

    #[test]
    fn both_cores_share_one_step_of_guest_time() {
        let mut c = Cpu::new(Bus::new(vec![0; 64]).unwrap(), crate::XIP);
        let entry = crate::RAM + 512;
        c.bus.write(0x01c7fff8, entry, 4).unwrap();
        c.bus.write(0x1eee004, 8, 4).unwrap();
        for _ in 0..10 {
            c.step().unwrap();
        }
        // Ten pairs of instructions (the secondary starts in the first).
        assert_eq!(c.steps, 10);
        assert_eq!(c.core_steps, [10, 10]);
        // CPU0 stopped: the secondary alone carries guest time.
        c.bus.write(0x1eee000, 4, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.steps, 11);
        assert_eq!(c.core_steps, [10, 11]);
    }

    #[test]
    fn paused_secondary_retains_context_until_resume() {
        let mut c = Cpu::new(Bus::new(vec![0; 32]).unwrap(), crate::XIP);
        let entry = crate::RAM + 512;
        c.bus.write(0x01c7fff8, entry, 4).unwrap();
        c.bus.write(0x1eee004, 8, 4).unwrap();
        c.step().unwrap();
        let paused_pc = c.secondary.as_ref().unwrap().pc;
        c.bus.write(0x1eee004, 12, 4).unwrap();
        assert_eq!(c.bus.read(0x1eee004, 4).unwrap() & 0x1c, 16);
        c.step().unwrap();
        c.step().unwrap();
        assert_eq!(c.secondary.as_ref().unwrap().pc, paused_pc);
        c.bus.write(0x1eee004, 24, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.secondary.as_ref().unwrap().pc, paused_pc + 2);
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access { pc, fault } => write!(f, "at PC 0x{pc:08x}: {fault}"),
            Self::Unsupported { pc, word } => {
                write!(f, "unsupported instruction 0x{word:04x} at PC 0x{pc:08x}")
            }
            Self::Limit { pc, limit } => write!(f, "instruction limit {limit} at PC 0x{pc:08x}"),
            Self::Preservation => write!(f, "probe did not preserve registers or stack"),
            Self::Trace(message) => write!(f, "trace: {message}"),
        }
    }
}

/// Internal result: the fault is boxed so that results stay register-sized
/// on the per-instruction path; `step` unboxes it.
pub(crate) type Step<T> = Result<T, Box<Fault>>;

pub fn signed(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

pub struct Cpu {
    pub bus: Bus,
    pub r: [u32; 16],
    pub sr: [u32; 16],
    pub pc: u32,
    pub steps: u64,
    /// Instructions executed by each core. `steps` is the time base: with
    /// both cores running, a pair of instructions (one per core) takes one
    /// step of guest time, as both cores run on the same clock.
    pub core_steps: [u64; 2],
    /// False while the secondary executes alongside the primary: its
    /// instruction then shares the primary's step of guest time.
    counts_time: bool,
    /// Instructions per 24 MHz oscillator tick: the emulated CPU clock is
    /// 24 MHz times this (1 = 24 MHz, the real-time default; the FM-1's
    /// WL82 runs at 120-396 MHz, typically 320). Timers, audio DMA, USB and
    /// the watchdog advance once per tick.
    pub instructions_per_tick: u32,
    subtick: u32,
    pub interrupts_enabled: bool,
    pub irq_entries: u64,
    in_interrupt: bool,
    predicate_skip: Option<(u32, u32)>,
    irq_predicate: Option<(u32, u32)>,
    repeat: Option<(u32, u32, u32)>,
    irq_repeat: Option<(u32, u32, u32)>,
    bus_locked: bool,
    /// Stopped by `idle` until an interrupt is taken (per core).
    halted: bool,
    secondary: Option<Core>,
    irq_priority_mask: u32,
    /// Name of the last executed instruction form (what `step` returns).
    pub(crate) name: &'static str,
    /// `run_steps` and `skip_idle` jump over halted spans (true, the
    /// default); false steps every halted slot (the reference behaviour,
    /// identical guest-visible state, for comparisons).
    pub idle_skip: bool,
    /// Instruction slots in which a core was halted by `idle` (stepped or
    /// skipped), summed over both cores: the units of `core_steps`, in
    /// which they are counted too.
    pub idle_slots: u64,
    /// Of `idle_slots`, those jumped over by `skip_idle` without stepping.
    pub idle_skipped: u64,
}

// Per-core context. Memory and devices remain on the one shared bus.
struct Core {
    irq_priority_mask: u32,
    r: [u32; 16],
    sr: [u32; 16],
    pc: u32,
    interrupts_enabled: bool,
    in_interrupt: bool,
    predicate_skip: Option<(u32, u32)>,
    irq_predicate: Option<(u32, u32)>,
    repeat: Option<(u32, u32, u32)>,
    irq_repeat: Option<(u32, u32, u32)>,
    bus_locked: bool,
    halted: bool,
}
impl Core {
    fn reset(pc: u32) -> Self {
        let mut sr = [0; 16];
        sr[6] = 1;
        Self {
            irq_priority_mask: 0,
            r: [0; 16],
            sr,
            pc,
            interrupts_enabled: false,
            // ROM's secondary handoff is in supervisor/interrupt context.
            in_interrupt: true,
            predicate_skip: None,
            irq_predicate: None,
            repeat: None,
            irq_repeat: None,
            bus_locked: false,
            halted: false,
        }
    }
    fn swap(&mut self, cpu: &mut Cpu) {
        use std::mem::swap;
        swap(&mut self.r, &mut cpu.r);
        swap(&mut self.sr, &mut cpu.sr);
        swap(&mut self.pc, &mut cpu.pc);
        swap(&mut self.interrupts_enabled, &mut cpu.interrupts_enabled);
        swap(&mut self.in_interrupt, &mut cpu.in_interrupt);
        swap(&mut self.predicate_skip, &mut cpu.predicate_skip);
        swap(&mut self.irq_predicate, &mut cpu.irq_predicate);
        swap(&mut self.repeat, &mut cpu.repeat);
        swap(&mut self.irq_repeat, &mut cpu.irq_repeat);
        swap(&mut self.bus_locked, &mut cpu.bus_locked);
        swap(&mut self.irq_priority_mask, &mut cpu.irq_priority_mask);
        swap(&mut self.halted, &mut cpu.halted);
    }
}

impl Cpu {
    pub fn new(bus: Bus, entry: u32) -> Self {
        Self {
            bus,
            r: [0; 16],
            sr: [0; 16],
            pc: entry,
            steps: 0,
            core_steps: [0; 2],
            counts_time: true,
            instructions_per_tick: 1,
            subtick: 0,
            interrupts_enabled: false,
            irq_entries: 0,
            in_interrupt: false,
            predicate_skip: None,
            irq_predicate: None,
            repeat: None,
            irq_repeat: None,
            bus_locked: false,
            halted: false,
            secondary: None,
            irq_priority_mask: 0,
            name: "",
            idle_skip: true,
            idle_slots: 0,
            idle_skipped: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn read(&self, address: u32, size: usize) -> Step<u32> {
        self.bus
            .read(address, size)
            .map_err(|fault| Box::new(Fault::Access { pc: self.pc, fault }))
    }

    #[inline]
    pub(crate) fn write(&mut self, address: u32, value: u32) -> Step<()> {
        self.bus
            .write(address, value, 4)
            .map_err(|fault| Box::new(Fault::Access { pc: self.pc, fault }))
    }

    /// PSR bit 1 is the carry flag (Quarkslab pi32v2 PSR: V, C, Z, N).
    pub(crate) fn set_carry(&mut self, carry: bool) {
        self.sr[5] = (self.sr[5] & !2) | (u32::from(carry) << 1);
    }

    pub(crate) fn carry(&self) -> bool {
        self.sr[5] & 2 != 0
    }

    #[inline]
    pub(crate) fn push(&mut self, value: u32) -> Step<()> {
        let address = self.sr[14].wrapping_sub(4);
        self.write(address, value)?;
        self.sr[14] = address;
        Ok(())
    }

    #[inline]
    pub(crate) fn pop(&mut self) -> Step<u32> {
        let value = self.read(self.sr[14], 4)?;
        self.sr[14] = self.sr[14].wrapping_add(4);
        Ok(value)
    }

    /// The primary core is inside an interrupt handler (until its `rti`).
    pub fn in_interrupt(&self) -> bool {
        self.in_interrupt
    }

    pub fn step(&mut self) -> Result<&'static str, Fault> {
        self.step_cores().map_err(|fault| *fault)
    }

    /// Instructions `run_steps` runs between checks for halted cores.
    const HALT_CHECK_INTERVAL: u64 = 64;

    /// The primary core is halted by `idle`, waiting for an interrupt.
    pub fn halted(&self) -> bool {
        self.halted
    }

    /// `count` calls of `step`, stopping at the first fault, in one loop
    /// (no per-instruction call or result). Spans in which every running
    /// core is halted are jumped over (`skip_idle`) unless `idle_skip` is
    /// off; the guest-visible state is the same either way.
    pub fn run_steps(&mut self, count: u64) -> Result<(), Fault> {
        let mut done = 0;
        while done < count {
            if self.halted || self.secondary.as_ref().is_some_and(|core| core.halted) {
                let skipped = self.skip_calls(count - done, u64::MAX);
                if skipped > 0 {
                    done += skipped;
                    continue;
                }
            }
            // Checking for a halt only every few instructions keeps this
            // loop as tight as a plain one; the halted slots stepped
            // meanwhile are equivalent to skipped ones.
            let batch = (count - done).min(Self::HALT_CHECK_INTERVAL);
            for _ in 0..batch {
                self.step_cores().map_err(|fault| *fault)?;
            }
            done += batch;
        }
        Ok(())
    }

    /// While every core that would run is halted by `idle` and no interrupt
    /// can be taken, jump guest time forward as if the halted slots had
    /// been stepped one by one, to just before the slot that runs the next
    /// device event, by at most `max_steps` steps of guest time (`steps`).
    /// Returns the steps skipped (0: step normally). Each skipped step
    /// counts exactly like a stepped halted one (in `steps`, `core_steps`
    /// and `idle_slots`), so timers,
    /// audio DMA, the LCD, USB and the watchdog see the same ticks.
    pub fn skip_idle(&mut self, max_steps: u64) -> u64 {
        let before = self.steps;
        self.skip_calls(u64::MAX, max_steps);
        self.steps - before
    }

    /// `skip_idle` counted in calls of `step` (one step of guest time
    /// each), at most `max_calls`; returns the calls skipped.
    pub fn skip_idle_calls(&mut self, max_calls: u64) -> u64 {
        self.skip_calls(max_calls, u64::MAX)
    }

    /// `skip_idle` in calls of `step_cores` (at most `max_calls`, and at
    /// most `max_steps` steps of guest time); returns the calls skipped.
    #[cold]
    #[inline(never)]
    fn skip_calls(&mut self, max_calls: u64, max_steps: u64) -> u64 {
        if !self.idle_skip {
            return 0;
        }
        // Mirror `step_cores`: which cores step on the next call. A call
        // that starts or resets the secondary core is stepped normally.
        let control = self.bus.core_control(1);
        let secondary = self.secondary.as_ref();
        if (control & 2 != 0 && secondary.is_some()) || (secondary.is_none() && control & 10 == 8) {
            return 0;
        }
        let secondary_running = control & 0x18 == 8 && secondary.is_some();
        let secondary_alone = secondary_running
            && (self.bus.core_control(0) & 16 != 0 || secondary.is_some_and(|core| core.bus_locked));
        let primary_steps = !secondary_alone;
        let secondary_steps = secondary_alone || (secondary_running && !self.bus_locked);
        if (primary_steps && !self.halted)
            || (secondary_steps && !secondary.is_some_and(|core| core.halted))
        {
            return 0;
        }
        // A halted core takes a deliverable interrupt in its next slot.
        if self.bus.any_irq_pending() {
            if primary_steps && self.bus.pending_irq_for(self.sr[11], self.sr[6] as usize).is_some() {
                return 0;
            }
            if secondary_steps
                && secondary.is_some_and(|core| {
                    self.bus.pending_irq_for(core.sr[11], core.sr[6] as usize).is_some()
                })
            {
                return 0;
            }
        }
        let now = self.bus.now;
        let next_event = self.bus.next_event;
        if next_event <= now {
            return 0;
        }
        let per_tick = self.instructions_per_tick as u64;
        // Every call of `step_cores` is one step of guest time: the two
        // cores share a clock, so a pair of slots (one per core) takes one
        // step. Steps up to and including the one whose tick reaches
        // next_event; that one is left to `step_cores` (it runs the event).
        let to_event = (per_tick - self.subtick as u64) + (next_event - now - 1) * per_tick;
        let calls = (to_event - 1).min(max_calls).min(max_steps);
        if calls == 0 {
            return 0;
        }
        let subtick = self.subtick as u64 + calls;
        self.bus.now += subtick / per_tick;
        self.subtick = (subtick % per_tick) as u32;
        self.steps += calls;
        self.core_steps[0] += if primary_steps { calls } else { 0 };
        self.core_steps[1] += if secondary_steps { calls } else { 0 };
        let slots = calls * (u64::from(primary_steps) + u64::from(secondary_steps));
        self.idle_slots += slots;
        self.idle_skipped += slots;
        calls
    }

    #[inline(always)]
    fn step_cores(&mut self) -> Step<&'static str> {
        let control = self.bus.core_control(1);
        if control & 2 != 0 && self.secondary.is_some() {
            self.secondary = None;
        }
        if self.secondary.is_none() && control & 10 == 8 {
            // SPL's RAM handoff vector, written by the unchanged stock guest.
            let entry = self.read(0x01c7fff8, 4)?;
            self.bus
                .fetch(entry)
                .map_err(|fault| Fault::Access { pc: self.pc, fault })?;
            self.secondary = Some(Core::reset(entry));
        }
        let secondary_running = control & 0x18 == 8 && self.secondary.is_some();
        if secondary_running
            && (self.bus.core_control(0) & 16 != 0
                || self.secondary.as_ref().is_some_and(|core| core.bus_locked))
        {
            return self.step_secondary();
        }
        self.step_core()?;
        self.core_steps[0] += 1;
        let op = self.name;
        if !self.bus_locked && secondary_running {
            // Both cores run on one clock: the secondary's instruction shares
            // the primary's step of guest time instead of adding its own.
            self.counts_time = false;
            let result = self.step_secondary();
            self.counts_time = true;
            result?;
        }
        Ok(op)
    }

    fn step_secondary(&mut self) -> Step<&'static str> {
        let mut secondary = self.secondary.take().unwrap();
        secondary.swap(self);
        let result = self.step_core();
        secondary.swap(self);
        self.secondary = Some(secondary);
        self.core_steps[1] += 1;
        result.map(|()| self.name)
    }

    /// A bundle's following slot at `self.pc`: its raw word (executed without
    /// normalization), classification and known operands.
    fn following(&mut self) -> Step<(u32, Op, Operands)> {
        let pc = self.pc;
        Ok(match self.bus.decoded(pc) {
            Some(entry) if !is_parallel(entry.h as u32) => {
                (entry.h as u32, entry.op, entry.operands())
            }
            Some(entry) => (entry.h as u32, decode(entry.h as u32), entry.operands()),
            None => {
                let h = self.read(pc, 2)?;
                (h, decode(h), Operands::default())
            }
        })
    }

    #[inline(always)]
    fn step_core(&mut self) -> Step<()> {
        if self.halted {
            return self.halted_slot();
        }
        if let Some((at, end)) = self.predicate_skip {
            if self.pc == at {
                self.pc = end;
                self.predicate_skip = None;
            }
        }
        let pc = self.pc;
        self.bus.pc_hint.set(pc);
        let (h, op, code) = match self.bus.decoded(pc) {
            Some(entry) => (entry.h as u32, entry.op, entry.operands()),
            None => {
                let h = self
                    .bus
                    .fetch(pc)
                    .map_err(|fault| Fault::Access { pc, fault })? as u32;
                (h, decode(primary(h)), Operands::default())
            }
        };
        if is_parallel(h) {
            self.execute_bundle(pc, h, op, code)?;
        } else {
            self.execute(h, op, code)?;
        }
        if let Some((start, end, count)) = self.repeat {
            if self.pc == end {
                if count > 1 {
                    self.pc = start;
                    self.repeat = Some((start, end, count - 1));
                } else {
                    self.repeat = None;
                }
            }
        }
        if self.counts_time {
            self.count_slot(pc)?;
        }
        self.dispatch_interrupt()
    }

    /// A slot of a core halted in `idle`: no instruction issues, but it
    /// takes the slot (and, when it carries guest time, the step of guest
    /// time) of one, so guest time runs exactly as if it had.
    #[inline(never)]
    fn halted_slot(&mut self) -> Step<()> {
        self.name = "halted";
        self.idle_slots += 1;
        if self.counts_time {
            self.count_slot(self.pc)?;
        }
        self.dispatch_interrupt()
    }

    /// One step of guest time (`pc`: for a device fault).
    #[inline(always)]
    fn count_slot(&mut self, pc: u32) -> Step<()> {
        self.steps += 1;
        self.subtick += 1;
        if self.subtick >= self.instructions_per_tick {
            self.subtick = 0;
            self.bus.now += 1;
            if self.bus.now >= self.bus.next_event {
                self.advance_devices(pc)?;
            }
        }
        Ok(())
    }

    /// A parallel bundle at `pc`: the following slot, then the primary slot
    /// (word `h`, classified as `op`), both reading the incoming registers.
    #[inline(never)]
    fn execute_bundle(&mut self, pc: u32, h: u32, op: Op, code: Operands) -> Step<()> {
        let length = if h >> 13 == 6 { 2 } else { 4 };
        self.pc = pc + length;
        let (following, following_op, following_code) = self.following()?;
        let before = self.r;
        let specials_before = self.sr;
        self.execute(following, following_op, following_code)?;
        let following_registers = self.r;
        let following_specials = self.sr;
        let continuation = self.pc;
        self.r = before;
        self.sr = specials_before;
        self.pc = pc;
        // The primary slot reads its operands after the following slot
        // ran; if that slot stored over them, decode them afresh.
        let (op, code) = if self.bus.is_cached(pc) {
            (op, code)
        } else {
            (decode(primary(h)), Operands::default())
        };
        self.execute(primary(h), op, code)?;
        // Both slots read the incoming registers. Compiler bundles have
        // distinct destinations; retain writes from the following slot
        // where the primary slot did not change that register.
        for i in 0..16 {
            if self.r[i] == before[i] {
                self.r[i] = following_registers[i];
            }
            if self.sr[i] == specials_before[i] {
                self.sr[i] = following_specials[i];
            }
        }
        self.pc = continuation;
        Ok(())
    }

    /// Oscillator tick `bus.now`, at which a device may have an event: the
    /// ticks since the last one only counted, so they are applied at once,
    /// then this tick runs exactly as every tick once did.
    fn advance_devices(&mut self, pc: u32) -> Step<()> {
        let now = self.bus.now;
        self.bus
            .catch_up(now - 1)
            .map_err(|fault| Fault::Access { pc, fault })?;
        self.bus.synced = now;
        // Should this tick fault, the next one runs exactly too.
        self.bus.next_event = now + 1;
        self.advance_tick(pc)?;
        self.bus.next_event = now + self.bus.ticks_to_event();
        Ok(())
    }

    /// One oscillator tick of the clocked devices.
    fn advance_tick(&mut self, pc: u32) -> Step<()> {
        self.bus.devices.advance(OSC_TICKS_PER_INSTRUCTION);
        self.bus
            .system
            .advance(OSC_TICKS_PER_INSTRUCTION)
            .map_err(|reason| Fault::Access {
                pc,
                fault: AccessFault {
                    address: 0x13e08,
                    size: 4,
                    operation: "watchdog",
                    reason,
                },
            })?;
        self.bus
            .advance_usb(OSC_TICKS_PER_INSTRUCTION)
            .map_err(|fault| Fault::Access { pc, fault })?;
        self.bus
            .advance_audio(OSC_TICKS_PER_INSTRUCTION)
            .map_err(|fault| Fault::Access { pc, fault })?;
        self.bus.spi2.advance(OSC_TICKS_PER_INSTRUCTION);
        self.bus.lcd.advance(OSC_TICKS_PER_INSTRUCTION);
        Ok(())
    }

    /// Set the emulated CPU clock in MHz: a multiple of the 24 MHz oscillator.
    /// Diagnostics: the secondary core's PC, if it has been started.
    pub fn secondary_pc(&self) -> Option<u32> {
        self.secondary.as_ref().map(|core| core.pc)
    }

    /// Diagnostics: whether the secondary core, if started, is halted in
    /// `idle`.
    pub fn secondary_halted(&self) -> Option<bool> {
        self.secondary.as_ref().map(|core| core.halted)
    }

    pub fn set_cpu_mhz(&mut self, mhz: u32) -> Result<(), String> {
        if mhz == 0 || mhz % 24 != 0 {
            return Err(format!(
                "CPU clock {mhz} MHz: use a multiple of 24 (24, 96, 192, 312...)"
            ));
        }
        self.instructions_per_tick = mhz / 24;
        self.subtick = 0;
        Ok(())
    }

    /// Guest time in oscillator ticks (24 MHz) since reset.
    pub fn ticks(&self) -> u64 {
        self.steps / self.instructions_per_tick as u64
    }

    pub(crate) fn conditional(&mut self, test: bool, counts: u32) -> Step<u32> {
        let mut cursor = self.pc + 4;
        let mut then_end = cursor;
        let then_count = (counts >> 14) + 1;
        let else_count = (counts >> 12) & 3;
        for i in 0..then_count + else_count {
            let h = self.read(cursor, 2)?;
            cursor += skip_length(h);
            // A parallel pair counts as one conditional instruction bundle.
            if h >> 13 == 6 || h & 0xf800 == 0xf000 {
                let following = self.read(cursor, 2)?;
                cursor += skip_length(following);
            }
            if i + 1 == then_count {
                then_end = cursor;
            }
        }
        if test {
            self.predicate_skip = Some((then_end, cursor));
            Ok(self.pc + 4)
        } else {
            Ok(then_end)
        }
    }

    /// A code halfword after the instruction word: the decode cache's copy
    /// when it has one, otherwise a bus read (with the bus's faults).
    #[inline(always)]
    pub(crate) fn operand(&self, known: Option<u16>, address: u32) -> Step<u32> {
        match known {
            Some(value) => Ok(value as u32),
            None => self.operand_read(address),
        }
    }

    #[cold]
    #[inline(never)]
    fn operand_read(&self, address: u32) -> Step<u32> {
        self.read(address, 2)
    }

    /// Execute the instruction word `h` at `self.pc`, classified as `op`.
    fn execute(&mut self, h: u32, op: Op, code: Operands) -> Step<()> {
        let pc = self.pc;
        let a = (h & 7) as usize;
        let b = ((h >> 4) & 7) as usize;
        let mut next = pc.wrapping_add(2);
        let name;
        match op {
            Op::MovImm32 => {
                let value = self.operand(code.x, pc + 2)? | (self.operand(code.y, pc + 4)? << 16);
                let n = (h & 15) as usize;
                if h & 0xfff0 == 0xffc0 {
                    self.r[n] = value;
                    name = "mov_imm32";
                } else if matches!(n, 0 | 12 | 13 | 14) {
                    self.sr[n] = value;
                    name = "stack_imm32";
                } else {
                    return Err(Fault::Unsupported { pc, word: h as u16 }.into());
                }
                next = pc + 6;
            }
            Op::RepeatRegister => {
                // Register-count repeat: the block runs r times with the count
                // latched, and the register reads zero afterwards. Inferred from
                // the stock binaries: memcpy (0x02044596) reuses the count
                // register as its byte temporary with no branch after the
                // block, while startup's `rep r2 {..}; if (r2 != 0) goto rep`
                // must then fall through. Neither pattern is satisfied by
                // one block per dispatch.
                if self.repeat.is_some() {
                    return Err(Fault::Unsupported { pc, word: h as u16 }.into());
                }
                let register = (h & 15) as usize;
                let length = (((h >> 4) & 15) + 1) * 2;
                let count = self.r[register];
                if count == 0 {
                    next = pc + 2 + length;
                } else {
                    self.r[register] = 0;
                    self.repeat = Some((pc + 2, pc + 2 + length, count));
                }
                name = "repeat_register";
            }
            Op::RepeatImmediate => {
                if self.repeat.is_some() {
                    return Err(Fault::Unsupported { pc, word: h as u16 }.into());
                }
                let length = (((h >> 4) & 15) + 1) * 2;
                let count = ((h >> 8) & 31) + 1;
                self.repeat = Some((pc + 2, pc + 2 + length, count));
                name = "repeat_immediate";
            }
            Op::MovSpecial => {
                let extra = self.operand(code.x, pc + 2)?;
                let reg = ((extra >> 12) & 15) as usize;
                let special = ((extra >> 8) & 15) as usize;
                // Deliberately exclude PC writes and unrecognized reserved encodings.
                if special == 15 || !matches!(extra & 255, 0 | 128) {
                    return Err(Fault::Unsupported { pc, word: h as u16 }.into());
                }
                if extra & 255 == 128 {
                    self.sr[special] = self.r[reg];
                    if special == 11 {
                        self.interrupts_enabled = self.sr[11] & 0x200 != 0;
                    }
                } else {
                    self.r[reg] = self.sr[special];
                }
                next = pc + 4;
                name = "mov_special";
            }
            Op::MovMask => {
                let extra = self.operand(code.x, pc + 2)?;
                // The packed immediate of the other forms (JieLi objdump:
                // e060 3164 is r3 = 0x640064, 3264 is r3 = 0x64006400).
                let value = crate::extended::packed(extra);
                self.r[((extra >> 12) & 15) as usize] = value;
                next = pc + 4;
                name = "mov_mask";
            }
            Op::MovImm16 => {
                self.r[(h & 15) as usize] = signed(self.operand(code.x, pc + 2)?, 16) as u32;
                next = pc + 4;
                name = "mov_imm16";
            }
            Op::MovImm8 => {
                self.r[a] = (((h >> 3) & 7) << 5) | ((h >> 8) & 31);
                name = "mov_imm8";
            }
            Op::MovNegative => {
                self.r[a] = 0xffff_ffe0 | ((h >> 8) & 31);
                name = "mov_negative";
            }
            Op::MovReg => {
                self.r[(h & 15) as usize] = self.r[((h >> 4) & 15) as usize];
                name = "mov_reg";
            }
            Op::AddSub => {
                let c = (((h >> 7) & 3) * 2 + ((h >> 3) & 1)) as usize;
                // These forms produce the carry consumed by addc/subc in the
                // compiler's 64-bit arithmetic (C = carry out / no borrow).
                let (lhs, rhs) = (self.r[b], self.r[c]);
                if h & 0xfe00 == 0x1e00 {
                    self.r[a] = lhs.wrapping_sub(rhs);
                    self.set_carry(lhs >= rhs);
                    name = "sub";
                } else {
                    let (sum, carry) = lhs.overflowing_add(rhs);
                    self.r[a] = sum;
                    self.set_carry(carry);
                    name = "add";
                }
            }
            Op::AddImm8 => {
                let imm = signed((((h >> 3) & 7) << 5) | ((h >> 8) & 31), 8);
                self.r[a] = self.r[a].wrapping_add(imm as u32);
                name = "add_imm8";
            }
            Op::AddSp => {
                let imm = (signed((h >> 5) & 7, 3) << 7) | (((h >> 8) & 31) << 2) as i32;
                self.sr[14] = self.sr[14].wrapping_add(imm as u32);
                name = "add_sp";
            }
            Op::AddSmall => {
                self.r[a] = self.r[b].wrapping_add((h >> 8) & 31);
                name = "add_small";
            }
            Op::Logic => match h & 0xff88 {
                0x1900 => {
                    self.r[a] |= self.r[b];
                    name = "or";
                }
                0x1908 => {
                    self.r[a] ^= self.r[b];
                    name = "xor";
                }
                0x1980 => {
                    self.r[a] &= self.r[b];
                    name = "and";
                }
                _ => {
                    self.r[a] = !self.r[b];
                    name = "not";
                }
            },
            Op::Asr => {
                self.r[a] = ((self.r[b] as i32) >> ((h >> 8) & 31)) as u32;
                name = "asr";
            }
            Op::Shift => {
                let shift = (h >> 8) & 31;
                if h & 0x80 != 0 {
                    self.r[a] = self.r[b] >> shift;
                    name = "lsr";
                } else {
                    self.r[a] = self.r[b] << shift;
                    name = "lsl";
                }
            }
            Op::BytePostIncrementRegister => {
                // JieLi objdump: "rD = b[rB++=rI] (u)" / "b[rB++=rI] = rD" with
                // rD = bits 0-2, store = bit 3, rB = bits 4-6, rI = r8 + bits 7-9;
                // the access uses rB, then rB += rI (post-increment, as its
                // "h[r15++=2]" for edd0). Stock/Baud Girl 0x0200de98: 13c0.
                let data = (h & 7) as usize;
                let base = ((h >> 4) & 7) as usize;
                let step = 8 + ((h >> 7) & 7) as usize;
                let address = self.r[base];
                let next = address.wrapping_add(self.r[step]);
                if h & 8 != 0 {
                    self.bus
                        .write(address, self.r[data] & 0xff, 1)
                        .map_err(|fault| Box::new(Fault::Access { pc: self.pc, fault }))?;
                    self.r[base] = next;
                    name = "byte_store_postincrement_register";
                } else {
                    let value = self.read(address, 1)?;
                    self.r[base] = next;
                    self.r[data] = value;
                    name = "byte_load_postincrement_register";
                }
            }
            Op::LoadStore32 => {
                let address = self.r[b].wrapping_add((signed((h >> 8) & 31, 5) * 4) as u32);
                if h & 0x80 != 0 {
                    self.write(address, self.r[a])?;
                    name = "store32";
                } else {
                    self.r[a] = self.read(address, 4)?;
                    name = "load32";
                }
            }
            Op::PushPopMask => {
                let mask = self.operand(code.x, pc + 2)?;
                // h bit 0 adds rets to the push and pc to the pop (FM-1_093
                // pairs e8d9/e8d5 as function prologue/epilogue).
                if h & !1 == 0xe8d8 {
                    if h & 1 != 0 {
                        self.push(self.sr[3])?;
                    }
                    for n in (0..16).rev() {
                        if mask & (1 << n) != 0 {
                            self.push(self.r[n])?;
                        }
                    }
                    name = "push_mask";
                } else {
                    for n in 0..16 {
                        if mask & (1 << n) != 0 {
                            self.r[n] = self.pop()?;
                        }
                    }
                    name = "pop_mask";
                }
                next = pc + 4;
                if h == 0xe8d5 {
                    next = self.pop()?;
                }
            }
            Op::PushRegs => {
                let boundary = (h & 15) as usize;
                let range = if boundary < 4 {
                    boundary..=3
                } else {
                    4..=boundary
                };
                for n in range.rev() {
                    self.push(self.r[n])?;
                }
                name = "push_regs";
            }
            Op::PopPc => {
                next = self.pop()?;
                name = "pop_pc";
            }
            Op::PushRets => {
                self.push(self.sr[3])?;
                name = "push_rets";
            }
            Op::PopRegs => {
                let boundary = (h & 15) as usize;
                let range = if boundary < 4 {
                    boundary..=3
                } else {
                    4..=boundary
                };
                for n in range {
                    self.r[n] = self.pop()?;
                }
                name = "pop_regs";
            }
            Op::PushRetsRegs => {
                self.push(self.sr[3])?;
                for n in (4..=(h & 15) as usize).rev() {
                    self.push(self.r[n])?;
                }
                name = "push_rets_regs";
            }
            Op::PopRetsRegs => {
                for n in 4..=(h & 15) as usize {
                    self.r[n] = self.pop()?;
                }
                self.sr[3] = self.pop()?;
                name = "pop_rets_regs";
            }
            Op::PopPcRegs => {
                for n in 4..=(h & 15) as usize {
                    self.r[n] = self.pop()?;
                }
                next = self.pop()?;
                name = "pop_pc_regs";
            }
            Op::MoveStackPointer => {
                match h {
                    0x1440 => self.sr[14] = self.sr[12],
                    0x1441 => self.sr[14] = self.sr[13],
                    0x1442 => self.sr[12] = self.sr[14],
                    _ => self.sr[13] = self.sr[14],
                }
                name = "move_stack_pointer";
            }
            Op::PushSpecial => {
                // 0x04c0 | mask over {reti 0, rete 1, retx 2, rets 3, sr4 4,
                // psr 5}, highest first, so the lowest is at the lowest
                // address (04e9 = {psr, rets, reti}; fm1_vec.S stubs: 04c8 =
                // {rets}).
                for index in (0..6).rev() {
                    if h & (1 << index) != 0 {
                        self.push(self.sr[index])?;
                    }
                }
                name = "push_special";
            }
            Op::PopSpecial => {
                // 0x0480 | the same mask, lowest first (SLOOP's tail calls:
                // pop {rets}; goto f; irq exit: 04a9 = {psr, rets, reti}).
                for index in 0..6 {
                    if h & (1 << index) != 0 {
                        self.sr[index] = self.pop()?;
                    }
                }
                name = "pop_special";
            }
            Op::CallRel32 => {
                // Vendor startup uses a signed byte displacement after a 6-byte call.
                let displacement =
                    self.operand(code.x, pc + 2)? | (self.operand(code.y, pc + 4)? << 16);
                self.sr[3] = pc + 6;
                next = (pc + 6).wrapping_add(displacement);
                name = "call_rel32";
            }
            Op::Rel22 => {
                let displacement = signed(((h & 63) << 16) | self.operand(code.x, pc + 2)?, 22) * 2;
                if h & 0xffc0 == 0xea80 {
                    self.sr[3] = pc + 4;
                    name = "call_rel22";
                } else {
                    name = "goto_rel22";
                }
                next = (pc + 4).wrapping_add(displacement as u32);
            }
            Op::CallRel9 => {
                // Vendor assembler's short relative call (signed 9-bit byte offset).
                let displacement = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9);
                self.sr[3] = pc + 2;
                next = (pc + 2).wrapping_add(displacement as u32);
                name = "call_rel9";
            }
            Op::GotoRel12 => {
                let displacement = signed(
                    ((h & 3) << 10) | (((h >> 4) & 15) << 6) | (((h >> 8) & 31) << 1),
                    12,
                );
                next = (pc + 2).wrapping_add(displacement as u32);
                name = "goto_rel12";
            }
            Op::BranchRegister => {
                let displacement = signed((((h >> 4) & 7) << 6) | (((h >> 8) & 31) << 1), 9);
                let nonzero = h & 0x80 != 0;
                if (self.r[a] != 0) == nonzero {
                    next = (pc + 2).wrapping_add(displacement as u32);
                }
                name = if nonzero {
                    "branch_nonzero"
                } else {
                    "branch_zero"
                };
            }
            Op::TestsetByte => {
                let address = self.r[(h & 15) as usize];
                let old = self.read(address, 1)?;
                self.bus
                    .write(address, 0xff, 1)
                    .map_err(|fault| Fault::Access { pc, fault })?;
                // FM-1_982 physical probe: the old byte's low nibble is copied
                // into the four PSR condition bits, not a comparison result.
                self.sr[5] = (self.sr[5] & !15) | (old & 15);
                name = "testset_byte";
            }
            Op::Return => {
                next = self.sr[3];
                name = "return";
            }
            Op::CallReg => {
                self.sr[3] = pc + 2;
                next = self.r[(h & 15) as usize];
                name = "call_reg";
            }
            Op::Rti => {
                if !self.in_interrupt {
                    return Err(Fault::Unsupported { pc, word: h as u16 }.into());
                }
                next = self.sr[0];
                self.sr[13] = self.sr[14];
                self.sr[14] = self.sr[12];
                self.in_interrupt = false;
                self.predicate_skip = self.irq_predicate.take();
                self.repeat = self.irq_repeat.take();
                self.interrupts_enabled = true;
                self.sr[11] = (self.sr[11] & !255) | 0x200;
                self.write(0x1eef1a8 + self.sr[6] * 0x200, self.irq_priority_mask)?;
                name = "rti";
            }
            Op::Cli => {
                self.interrupts_enabled = false;
                self.sr[11] &= !0x200;
                name = "cli";
            }
            Op::Lock => {
                // CPU bus ownership latch. With one executing core acquisition
                // cannot contend; this does not replace the guest's memory locks.
                self.bus_locked = h == 0x0041;
                name = if self.bus_locked {
                    "lockset"
                } else {
                    "lockclr"
                };
            }
            Op::Sti => {
                self.interrupts_enabled = true;
                self.sr[11] |= 0x200;
                name = "sti";
            }
            Op::Nop => {
                // 0x0001 is the SDK's asm("idle") (wait for interrupt; FM-1_093
                // IDLE0 task at 0x0205b8da): with interrupts enabled the core
                // halts after it until it takes an interrupt, which returns
                // to the next instruction. With interrupts off (or inside a
                // handler) nothing could wake it: there it is a hint and
                // execution continues (what the hardware does then is
                // unverified).
                name = match h {
                    0x0020 => "csync",
                    0x0001 => {
                        self.halted = self.interrupts_enabled
                            && !self.in_interrupt
                            && !self.blocks_interrupts_at(pc + 2);
                        "idle"
                    }
                    _ => "nop",
                };
            }
            Op::Wide => {
                // The extension halfword selects the form.
                let x = self.operand(code.x, pc + 2)?;
                let operands = Operands {
                    x: Some(x as u16),
                    y: code.y,
                };
                return self.execute(h, decode_wide(h, x), operands);
            }
            Op::Unsupported => return Err(Fault::Unsupported { pc, word: h as u16 }.into()),
            _ => {
                // Sets the name itself.
                self.pc = crate::extended::execute(self, op, h, pc, code)?;
                return Ok(());
            }
        }
        self.pc = next;
        self.name = name;
        Ok(())
    }

    /// Whether the PC is inside an active repeat block or the then-part of
    /// a conditional block (stale state after a branch out does not count).
    fn inside_block(&self) -> bool {
        let pc = self.pc;
        matches!(self.repeat, Some((start, end, _)) if (start..end).contains(&pc))
            || matches!(self.predicate_skip, Some((at, _)) if pc < at && at - pc <= 32)
    }

    /// Whether the PC `next` after an `idle` is one at which no interrupt
    /// is taken (`inside_block`, including a repeat that loops back): an
    /// `idle` there could never be woken, so it stays a hint.
    fn blocks_interrupts_at(&self, next: u32) -> bool {
        matches!(self.repeat, Some((start, end, count))
            if (start..end).contains(&next) || (next == end && count > 1))
            || matches!(self.predicate_skip, Some((at, _)) if next < at && at - next <= 32)
    }

    #[inline(always)]
    fn dispatch_interrupt(&mut self) -> Step<()> {
        // No interrupt inside a repeat or conditional block. The stock RTOS
        // context switch saves only r0-r15 and {psr, rets, reti} (04e9 /
        // 04a9 + e8d8/e8d4), so no hidden block state can survive an
        // interrupt that switches tasks; restoring it on rti leaked one
        // task's repeat into another (FM-1_093: a nested rep at 0x01c05026).
        if !self.interrupts_enabled
            || self.in_interrupt
            || !self.bus.any_irq_pending()
            || self.inside_block()
        {
            return Ok(());
        }
        self.take_interrupt()
    }

    /// Enter the highest-priority deliverable interrupt, if any.
    #[inline(never)]
    fn take_interrupt(&mut self) -> Step<()> {
        if let Some(source) = self.bus.pending_irq_for(self.sr[11], self.sr[6] as usize) {
            let mask_register = 0x1eef1a8 + self.sr[6] * 0x200;
            let priority = self
                .bus
                .devices
                .irq_priority_for(source, self.sr[11], self.sr[6] as usize)
                .unwrap();
            self.irq_priority_mask = self.read(mask_register, 4)?;
            self.write(mask_register, priority)?;
            let handler = self.read(0x01c7_fe00 + source as u32 * 4, 4)?;
            self.bus
                .fetch(handler)
                .map_err(|fault| Fault::Access { pc: self.pc, fault })?;
            self.sr[0] = self.pc;
            self.sr[12] = self.sr[14];
            self.sr[14] = self.sr[13];
            self.pc = handler;
            self.sr[11] = (self.sr[11] & !0x2ff) | source as u32;
            self.in_interrupt = true;
            self.irq_predicate = self.predicate_skip.take();
            self.irq_repeat = self.repeat.take();
            self.interrupts_enabled = false;
            self.halted = false;
            self.irq_entries += 1;
        }
        Ok(())
    }

    pub fn run(
        &mut self,
        stop: Option<u32>,
        limit: u64,
        mut trace: Option<&mut dyn Write>,
    ) -> Result<(), Fault> {
        while stop != Some(self.pc) {
            if self.steps >= limit {
                return Err(Fault::Limit { pc: self.pc, limit });
            }
            let pc = self.pc;
            let op = self.step()?;
            if let Some(writer) = trace.as_mut() {
                writeln!(
                    writer,
                    "{{\"pc\":{pc},\"op\":\"{op}\",\"next_pc\":{},\"registers\":{:?},\"sp\":{}}}",
                    self.pc, self.r, self.sr[14]
                )
                .map_err(|error| Fault::Trace(error.to_string()))?;
            }
        }
        Ok(())
    }

    pub fn probe(&mut self, limit: u64, trace: Option<&mut dyn Write>) -> Result<[u32; 12], Fault> {
        self.r = std::array::from_fn(|n| 0x1020_3040 + n as u32 * 0x0101_0101);
        self.r[0] = RESULT;
        let before = self.r;
        self.sr[14] = USER_STACK;
        self.sr[3] = PROBE_RETURN;
        self.run(Some(PROBE_RETURN), limit, trace)?;
        if self.r[1..] != before[1..] || self.sr[14] != USER_STACK {
            return Err(Fault::Preservation);
        }
        let mut values = [0; 12];
        for (i, value) in values.iter_mut().enumerate() {
            *value = self
                .read(RESULT + i as u32 * 4, 4)
                .map_err(|fault| *fault)?;
        }
        Ok(values)
    }
}
