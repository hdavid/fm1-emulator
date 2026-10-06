// SPDX-License-Identifier: GPL-3.0-only
//! The CPU side of spin-loop skipping (see `crate::spin`): looking for
//! loops, recording them, and the spinning cores of `skip_idle_calls`.
use super::{Cpu, Step};
use crate::spin::{Access, Clock, CoreState, Outcome, Plan, Recorder, Recording, SpinLoop};

/// Calls of `step` between looks at where the cores are.
pub(super) const POLL_STEPS: u32 = 64;
/// Loops kept per core.
const MAX_LOOPS: usize = 64;
/// Most polls to wait after failed recordings (doubling from 1).
const MAX_BACKOFF: u32 = 4096;

#[derive(Default)]
pub(super) struct SpinCore {
    /// The core's instructions are being recorded (`step_core_recording`).
    recording: bool,
    recorder: Recorder,
    loops: Vec<SpinLoop>,
    /// The core's PC was inside a known loop at the last poll.
    hint: bool,
    /// The last plan found the PC in a loop but not its state: record it.
    relearn: bool,
    /// No recording before `steps` reaches this; the current backoff in
    /// polls.
    resume_at: u64,
    backoff: u32,
}

impl Cpu {
    /// Whether `skip_idle_calls` may find something to jump over: the
    /// primary core is halted, or a core seemed to be in a known spin loop.
    /// A cheap test for callers that step one instruction at a time.
    pub fn may_skip(&self) -> bool {
        self.idle || self.spin_hint(0) || self.spin_hint(1)
    }

    pub(super) fn spin_hint(&self, core: usize) -> bool {
        self.spin_skip && self.spin[core].hint
    }

    pub(super) fn spin_recording(&self, core: usize) -> bool {
        self.spin[core].recording
    }

    /// Record a data access of the instruction executing (`logging`).
    #[cold]
    #[inline(never)]
    pub(super) fn log_access(&self, address: u32, size: usize, write: bool, value: u32) {
        // Stores keep only the bytes they write.
        let value = if size == 4 {
            value
        } else {
            value & ((1 << (8 * size)) - 1)
        };
        self.access_log.borrow_mut().push(Access {
            address,
            size: size as u8,
            write,
            value,
        });
    }

    /// The executing context as a spin-loop state, if it can be recorded.
    fn capture(&self) -> Option<CoreState> {
        (!self.idle && !self.bus_locked && self.irq_nest.is_empty() && self.exception.is_none())
            .then_some(CoreState {
                pc: self.pc,
                r: self.r,
                sr: self.sr,
                interrupts_enabled: self.interrupts_enabled,
                in_interrupt: self.in_interrupt,
                predicate_skip: self.predicate_skip,
                irq_predicate: self.irq_predicate,
                repeat: self.repeat,
                irq_repeat: self.irq_repeat,
                idle_wake_delay: self.idle_wake_delay,
            })
    }

    fn restore(&mut self, state: &CoreState) {
        self.pc = state.pc;
        self.r = state.r;
        self.sr = state.sr;
        self.interrupts_enabled = state.interrupts_enabled;
        self.in_interrupt = state.in_interrupt;
        self.predicate_skip = state.predicate_skip;
        self.irq_predicate = state.irq_predicate;
        self.repeat = state.repeat;
        self.irq_repeat = state.irq_repeat;
        self.idle_wake_delay = state.idle_wake_delay;
    }

    /// Run `f` with core `core`'s context in the executing registers.
    fn with_core<T>(&mut self, core: usize, f: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if core == 0 {
            return Some(f(self));
        }
        let mut secondary = self.secondary.take()?;
        secondary.swap(self);
        let result = f(self);
        secondary.swap(self);
        self.secondary = Some(secondary);
        Some(result)
    }

    /// Every `POLL_STEPS` calls: note cores whose PC lies in a known loop,
    /// and start recording a core that is in none (with backoff). While a
    /// core records, every call comes here.
    #[cold]
    #[inline(never)]
    pub(super) fn spin_poll(&mut self) {
        self.spin_countdown = POLL_STEPS;
        if !self.spin_skip || !self.idle_skip || self.bus.mmio_counting {
            for spin in &mut self.spin {
                spin.recording = false;
                spin.hint = false;
            }
            return;
        }
        for core in 0..2 {
            let (pc, halted) = match (core, self.secondary.as_ref()) {
                (0, _) => (self.pc, self.idle),
                (_, Some(secondary)) => (secondary.pc, secondary.idle),
                _ => {
                    self.spin[core].recording = false;
                    self.spin[core].hint = false;
                    continue;
                }
            };
            let spin = &mut self.spin[core];
            if spin.recording {
                continue;
            }
            spin.hint = !halted && spin.loops.iter().any(|l| l.contains(pc));
            if halted || (spin.hint && !spin.relearn) {
                continue;
            }
            if self.steps < spin.resume_at {
                continue;
            }
            spin.recording = true;
            spin.relearn = false;
            spin.recorder = Recorder::default();
        }
        if self.spin.iter().any(|spin| spin.recording) {
            self.spin_countdown = 1;
        }
    }

    /// `step_core` while recording core `core` (its context executing).
    #[cold]
    #[inline(never)]
    pub(super) fn step_core_recording(
        &mut self,
        core: usize,
        advance_time: bool,
    ) -> Step<&'static str> {
        let state = self.capture();
        let mut verdict = self.spin[core].recorder.before(state.as_ref());
        let head = match verdict {
            Recording::Restart => true,
            Recording::Continue => self.spin[core].recorder.at_head(),
            _ => false,
        };
        if head {
            // A head already in a known loop: nothing to record.
            let known = state.as_ref().is_some_and(|state| {
                self.spin[core]
                    .loops
                    .iter()
                    .any(|l| l.locate(state).is_some())
            });
            if known {
                let spin = &mut self.spin[core];
                spin.recording = false;
                spin.hint = true;
                return self.step_core(advance_time);
            }
            verdict = Recording::Continue;
        }
        match verdict {
            Recording::Continue | Recording::Restart => {}
            Recording::Abort(reason) => {
                self.spin_rejected(core, reason);
                return self.step_core(advance_time);
            }
            Recording::Done(passes) => {
                self.spin_accept(core, passes);
                return self.step_core(advance_time);
            }
        }
        let Some(state) = state else {
            unreachable!("the recorder continues only with a state");
        };
        let (entries, exceptions) = (self.irq_entries, self.exception_entries);
        self.logging = true;
        let result = self.step_core(advance_time);
        self.logging = false;
        let accesses = std::mem::take(&mut *self.access_log.borrow_mut());
        if result.is_err() {
            self.spin[core].recording = false;
            return result;
        }
        if self.irq_entries != entries || self.exception_entries != exceptions || self.idle {
            self.spin_rejected(core, "interrupted, excepted or halted");
        } else {
            self.spin[core].recorder.after(state, accesses);
        }
        result
    }

    fn spin_accept(&mut self, core: usize, passes: Vec<Vec<crate::spin::Phase>>) {
        self.spin[core].recording = false;
        let head = passes[0][0].state.pc;
        let bus = &self.bus;
        let code = |pc: u32| -> Option<[u16; 3]> {
            let mut words = [0; 3];
            for (i, word) in words.iter_mut().enumerate() {
                *word = bus.fetch(pc.wrapping_add(2 * i as u32)).ok()?;
            }
            Some(words)
        };
        match SpinLoop::analyze(passes, code) {
            Ok(spin) => {
                if self.spin_log {
                    eprintln!(
                        "spin: core {core} loop at 0x{head:08x}: {} instructions, {} memory accesses, {} timer accesses, {} read keys",
                        spin.period(),
                        spin.memory.len(),
                        spin.timer_ops.len(),
                        spin.passes.len()
                    );
                }
                let state = &mut self.spin[core];
                if state.loops.len() == MAX_LOOPS {
                    state.loops.remove(0);
                }
                state.loops.push(spin);
                state.hint = true;
                state.backoff /= 2;
                state.resume_at = 0;
            }
            Err(reason) => self.spin_rejected(core, reason),
        }
    }

    fn spin_rejected(&mut self, core: usize, reason: &str) {
        if self.spin_log {
            eprintln!("spin: core {core} at 0x{:08x}: no loop ({reason})", self.pc);
        }
        let state = &mut self.spin[core];
        state.recording = false;
        state.recorder = Recorder::default();
        state.backoff = (state.backoff * 2).clamp(1, MAX_BACKOFF);
        state.resume_at = self.steps + u64::from(state.backoff) * u64::from(POLL_STEPS);
    }

    /// Where core `core` is in one of its loops, if it is in one and the
    /// memory and code that loop depends on still hold the recorded values.
    fn spin_plan(&mut self, core: usize, offset: u64) -> Option<Plan> {
        let state = self.with_core(core, |cpu| cpu.capture())??;
        let located = self.spin[core]
            .loops
            .iter()
            .enumerate()
            .find_map(|(index, spin)| spin.locate(&state).map(|found| (index, found)));
        let Some((index, (phase, candidates))) = located else {
            // In a loop's PCs with another state: maybe another instance.
            self.spin[core].relearn = true;
            return None;
        };
        let spin = &self.spin[core].loops[index];
        let bus = &self.bus;
        let memory = spin
            .memory
            .iter()
            .all(|a| bus.read(a.address, a.size as usize).ok() == Some(a.value));
        let code = spin.pcs.iter().zip(&spin.code).all(|(&pc, words)| {
            (0..3).all(|i| bus.fetch(pc.wrapping_add(2 * i as u32)).ok() == Some(words[i]))
        });
        (memory && code).then_some(Plan {
            core,
            spin: index,
            phase,
            candidates,
            offset,
        })
    }

    /// For a span of `calls` calls: the outcome for each spinning core, or
    /// None if one of them is not in a loop that runs on exactly as
    /// recorded for the whole span.
    pub(super) fn spin_outcomes(
        &mut self,
        primary: bool,
        secondary: bool,
        secondary_alone: bool,
        calls: u64,
    ) -> Option<Vec<(usize, Outcome)>> {
        let mut plans = vec![];
        for (core, spins, offset) in [(0, primary, 0), (1, secondary, u64::from(!secondary_alone))]
        {
            if !spins {
                continue;
            }
            match self.spin_plan(core, offset) {
                Some(plan) => plans.push(plan),
                None => {
                    self.spin[core].hint = false;
                    return None;
                }
            }
        }
        // Two loops replaying the same timers would interleave: not handled.
        let with_timers = plans
            .iter()
            .filter(|p| !self.spin[p.core].loops[p.spin].timer_ops.is_empty())
            .count();
        if with_timers > 1 {
            return None;
        }
        let bus = &self.bus;
        let tick_after = |issues: u64| bus.tick_after_issues(issues);
        let clock = Clock {
            synced: bus.synced(),
            tick_after: &tick_after,
            peripheral_hz: bus.peripheral_hz(),
        };
        let mut outcomes = vec![];
        for plan in &plans {
            let spin = &self.spin[plan.core].loops[plan.spin];
            let outcome = plan.advance(spin, calls, &clock, |base| {
                bus.devices
                    .timer(base)
                    .expect("timer ops name TIMER4 or TIMER5")
                    .clone()
            });
            match outcome {
                Some(outcome) => outcomes.push((plan.core, outcome)),
                None => {
                    self.spin[plan.core].hint = false;
                    return None;
                }
            }
        }
        Some(outcomes)
    }

    /// Leave each spinning core where the span took it, and its timers as
    /// they were replayed. Guest time has already advanced over the span.
    pub(super) fn spin_apply(&mut self, outcomes: Vec<(usize, Outcome)>, calls: u64) {
        let latest = outcomes
            .iter()
            .flat_map(|(_, outcome)| outcome.timers.iter().map(|(_, _, at)| *at))
            .max();
        if let Some(latest) = latest {
            // As the counter writes would: devices brought up to the last
            // one (no device has an event inside the span), the timers in
            // their replayed state, and the next tick exact.
            self.bus.sync_to(latest);
            let hz = self.bus.peripheral_hz();
            for (_, outcome) in &outcomes {
                for (base, timer, at) in &outcome.timers {
                    let mut timer = timer.clone();
                    if latest > *at {
                        timer.advance_with_clock((latest - at) as u32, hz);
                    }
                    *self.bus.devices.timer_mut(*base).expect("TIMER4 or TIMER5") = timer;
                }
            }
            self.bus.registers_written();
        }
        for (core, outcome) in outcomes {
            self.spin_skipped[core] += calls;
            self.with_core(core, |cpu| cpu.restore(&outcome.state));
        }
    }
}
