// SPDX-License-Identifier: GPL-3.0-only
//! Spin loops: a core that busy-waits in a short loop (polling memory for a
//! flag that another core or an interrupt handler sets) while nothing it
//! reads changes. Both stock FM-1 cores do this: CPU0 waits for the LCD DMA
//! flag (0x0200f11e..0x0200f12c), CPU1 polls its job mailbox (0x01c0247e..,
//! calling 0x01c01c22, which resets TIMER5's counter and reads it back), so
//! the halted-core skip (`Cpu::skip_idle_calls`) never applies to them.
//!
//! A loop is first recorded (`Recorder`): passes from one head state back
//! to that state, with the core's state before every instruction and the
//! data accesses each instruction made. `SpinLoop::analyze` accepts them as a
//! fixed point: the same PCs, the same state at the head, the same memory
//! addresses with the same values read and stored, and as the only device
//! accesses TIMER4/TIMER5 register reads and counter writes. A timer read
//! may return different values in different passes (it measures time): the
//! passes are told apart by those values (their "key"), and passes with the
//! same values up to an instruction must have the same state there. A pass
//! is then a function of its head state, memory and these values: while
//! memory holds the recorded values and every timer read returns a value a
//! recorded pass read, the core repeats recorded passes exactly.
//!
//! `Cpu::skip_idle_calls` then jumps over spans in which every issuing core
//! is halted or in such a loop (`Plan`): the stores write values memory
//! already holds, the timer accesses are replayed at the oscillator tick at
//! which each would run (`Plan::advance`, with the bus's fractional
//! instruction clock), and the core is left in the recorded state of the
//! instruction it reached. Anything else (an interrupt, a device event, a
//! timer value no pass read) ends the span before it happens, as for a
//! halted core.
use crate::cpu::Repeat;
use crate::devices::{Timer, TIMER4, TIMER5};

/// One data access by an instruction (not an instruction fetch).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Access {
    pub address: u32,
    pub size: u8,
    pub write: bool,
    pub value: u32,
}

/// The part of a core's context that decides what it executes next. Cores
/// halted in `idle`, holding the bus lock, inside nested interrupts or with
/// an exception to enter are never recorded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CoreState {
    pub pc: u32,
    pub r: [u32; 16],
    pub sr: [u32; 16],
    pub interrupts_enabled: bool,
    pub in_interrupt: bool,
    pub predicate_skip: Option<(u32, u32, u32)>,
    pub irq_predicate: Option<(u32, u32, u32)>,
    pub repeat: Option<Repeat>,
    pub irq_repeat: Option<Repeat>,
    pub idle_wake_delay: u8,
}

/// Passes recorded before a loop is accepted: `ITERATIONS`, or
/// `TIMED_ITERATIONS` when it accesses a timer. More passes see more of the
/// timer values the loop can meet (the instruction clock moves against the
/// timer by a fraction of an oscillator tick each pass).
pub(crate) const ITERATIONS: usize = 4;
pub(crate) const TIMED_ITERATIONS: usize = 16;
/// Longest pass recorded, in instructions.
pub(crate) const MAX_PERIOD: usize = 256;

/// One instruction of a recorded pass: the state before it and its accesses.
pub(crate) struct Phase {
    pub state: CoreState,
    pub accesses: Vec<Access>,
}

#[derive(Default)]
pub(crate) struct Recorder {
    passes: Vec<Vec<Phase>>,
    current: Vec<Phase>,
    restarted: bool,
}

pub(crate) enum Recording {
    Continue,
    /// Started again: the next instruction is the new head.
    Restart,
    Done(Vec<Vec<Phase>>),
    Abort(&'static str),
}

impl Recorder {
    /// Before an instruction: `state` is the core's state (None: not
    /// recordable). Closes a pass when the head PC comes round again.
    pub fn before(&mut self, state: Option<&CoreState>) -> Recording {
        let Some(state) = state else {
            return Recording::Abort("halted, bus locked, nested or excepting");
        };
        let head = self
            .passes
            .first()
            .or(Some(&self.current))
            .and_then(|p| p.first());
        if let Some(head) = head {
            // The head state, not just its PC: a pass may run an inner loop
            // (a delay) through the head PC with other register values.
            if *state == head.state && !self.current.is_empty() {
                self.passes.push(std::mem::take(&mut self.current));
                let devices = self.passes[0]
                    .iter()
                    .flat_map(|phase| &phase.accesses)
                    .any(|access| !is_memory(access.address));
                let wanted = if devices {
                    TIMED_ITERATIONS
                } else {
                    ITERATIONS
                };
                if self.passes.len() == wanted {
                    return Recording::Done(std::mem::take(&mut self.passes));
                }
            }
        }
        if self.current.len() >= MAX_PERIOD {
            // Started outside the loop (say, in a handler that returned into
            // it): once, start again with this instruction as the head.
            if self.passes.is_empty() && !self.restarted {
                self.restarted = true;
                self.current.clear();
                return Recording::Restart;
            }
            return Recording::Abort("no pass back to the head");
        }
        Recording::Continue
    }

    /// Whether the next instruction would be the head of the recording.
    pub fn at_head(&self) -> bool {
        self.passes.is_empty() && self.current.is_empty()
    }

    /// After the instruction that started in `state` made `accesses`.
    pub fn after(&mut self, state: CoreState, accesses: Vec<Access>) {
        self.current.push(Phase { state, accesses });
    }
}

/// A timer register access: (CON address of TIMER4/5, register offset).
fn timer_register(access: &Access) -> Option<(u32, u32)> {
    [TIMER4, TIMER5].into_iter().find_map(|base| {
        let offset = access.address.wrapping_sub(base);
        (offset < 12 && offset % 4 == 0 && access.size == 4).then_some((base, offset))
    })
}

/// A timer access of a loop pass, in program order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimerOp {
    pub phase: usize,
    pub base: u32,
    pub offset: u32,
    /// The counter value written; None for a read.
    pub write: Option<u32>,
}

pub(crate) struct SpinLoop {
    /// PC of each instruction of a pass (phase).
    pub pcs: Vec<u32>,
    /// The code halfwords at each phase's PC when recorded.
    pub code: Vec<[u16; 3]>,
    /// Per recorded pass: the state before each phase, and its key (the
    /// values of its timer reads, in order).
    pub passes: Vec<(Vec<CoreState>, Vec<u32>)>,
    /// Memory accesses (reads and stores, recorded values).
    pub memory: Vec<Access>,
    pub timer_ops: Vec<TimerOp>,
    /// The loop's PCs, sorted and unique (a cheap first test).
    pub pc_set: Vec<u32>,
}

/// Whether `address` is ordinary memory (SRAM or the XIP window): values a
/// pass reads there stay as recorded while nothing stores to them.
fn is_memory(address: u32) -> bool {
    let ram = address.wrapping_sub(crate::RAM) < crate::RAM_SIZE as u32;
    let xip = (crate::XIP..crate::XIP_END).contains(&address);
    ram || xip
}

impl SpinLoop {
    /// Accept recorded passes as a loop, or say why not. `code(pc)` reads
    /// the instruction halfwords at `pc`.
    pub fn analyze(
        passes: Vec<Vec<Phase>>,
        code: impl Fn(u32) -> Option<[u16; 3]>,
    ) -> Result<Self, &'static str> {
        let first = passes.first().ok_or("no pass")?;
        let period = first.len();
        if period == 0 || passes.iter().any(|p| p.len() != period) {
            return Err("passes differ in length");
        }
        let pcs: Vec<u32> = first.iter().map(|phase| phase.state.pc).collect();
        if passes
            .iter()
            .any(|p| p.iter().map(|phase| phase.state.pc).ne(pcs.iter().copied()))
        {
            return Err("passes take different paths");
        }
        if passes.iter().any(|p| p[0].state != first[0].state) {
            return Err("state at the head differs between passes");
        }
        let mut memory = vec![];
        let mut timer_ops = vec![];
        // (phase, access index) of each timer read, in program order.
        let mut read_at = vec![];
        for (phase, recorded) in first.iter().enumerate() {
            let shape = |p: &Vec<Phase>| {
                p[phase]
                    .accesses
                    .iter()
                    .map(|a| (a.address, a.size, a.write))
                    .collect::<Vec<_>>()
            };
            if passes.iter().any(|p| shape(p) != shape(&passes[0])) {
                return Err("passes access different addresses");
            }
            for (i, access) in recorded.accesses.iter().enumerate() {
                let same_value = passes
                    .iter()
                    .all(|p| p[phase].accesses[i].value == access.value);
                if is_memory(access.address) {
                    if !same_value {
                        return Err("a memory value changes between passes");
                    }
                    memory.push(*access);
                } else if let Some((base, offset)) = timer_register(access) {
                    if access.write && (offset != 4 || !same_value) {
                        return Err("a timer write other than a fixed counter value");
                    }
                    if !access.write {
                        read_at.push((phase, i));
                    }
                    timer_ops.push(TimerOp {
                        phase,
                        base,
                        offset,
                        write: access.write.then_some(access.value),
                    });
                } else {
                    return Err("device access");
                }
            }
        }
        // A timer the pass only reads runs free: its values never repeat,
        // so no later pass would be a recorded one.
        let free_running = timer_ops.iter().any(|read| {
            read.write.is_none()
                && !timer_ops
                    .iter()
                    .any(|op| op.base == read.base && op.write.is_some())
        });
        if free_running {
            return Err("reads a timer it does not reset");
        }
        let key = |p: &Vec<Phase>| {
            read_at
                .iter()
                .map(|&(phase, i)| p[phase].accesses[i].value)
                .collect::<Vec<u32>>()
        };
        let passes: Vec<(Vec<CoreState>, Vec<u32>)> = passes
            .iter()
            .map(|p| (p.iter().map(|phase| phase.state.clone()).collect(), key(p)))
            .collect();
        let reads: Vec<usize> = read_at.iter().map(|&(phase, _)| phase).collect();
        // Determinism: equal read values so far, equal state.
        for phase in 0..period {
            let known = reads.iter().take_while(|&&p| p < phase).count();
            for a in &passes {
                for b in &passes {
                    if a.1[..known] == b.1[..known] && a.0[phase] != b.0[phase] {
                        return Err("same inputs, different state");
                    }
                }
            }
        }
        // Passes with the same read values are identical: keep one of each.
        let mut unique: Vec<(Vec<CoreState>, Vec<u32>)> = vec![];
        for pass in passes {
            if !unique.iter().any(|(_, key)| *key == pass.1) {
                unique.push(pass);
            }
        }
        let code = pcs
            .iter()
            .map(|&pc| code(pc).ok_or("code unreadable"))
            .collect::<Result<_, _>>()?;
        let mut pc_set = pcs.clone();
        pc_set.sort_unstable();
        pc_set.dedup();
        Ok(Self {
            pcs,
            code,
            passes: unique,
            memory,
            timer_ops,
            pc_set,
        })
    }

    /// Whether `pc` is one of the loop's PCs.
    pub fn contains(&self, pc: u32) -> bool {
        self.pc_set.binary_search(&pc).is_ok()
    }

    pub fn period(&self) -> usize {
        self.pcs.len()
    }

    /// How many timer reads a pass makes before `phase`.
    fn reads_before(&self, phase: usize) -> usize {
        self.timer_ops
            .iter()
            .filter(|op| op.write.is_none() && op.phase < phase)
            .count()
    }

    /// The phase whose recorded state is `state`, with the recorded passes
    /// that are in it there.
    pub fn locate(&self, state: &CoreState) -> Option<(usize, Vec<usize>)> {
        if !self.contains(state.pc) {
            return None;
        }
        (0..self.period())
            .filter(|&phase| self.pcs[phase] == state.pc)
            .find_map(|phase| {
                let passes: Vec<usize> = (0..self.passes.len())
                    .filter(|&i| self.passes[i].0[phase] == *state)
                    .collect();
                (!passes.is_empty()).then_some((phase, passes))
            })
    }

    /// The state after running from the head with timer read values `key`
    /// (those of the reads before `phase`) to `phase`, if recorded.
    fn state_at(&self, phase: usize, key: &[u32]) -> Option<&CoreState> {
        self.passes
            .iter()
            .find(|(_, k)| k.starts_with(key))
            .map(|(states, _)| &states[phase])
    }

    /// Whether a pass that starts at `phase` in one of `candidates` and then
    /// reads `values` (the reads at or after `phase`) is a recorded one.
    fn continues(&self, phase: usize, candidates: &[usize], values: &[u32]) -> bool {
        let before = self.reads_before(phase);
        candidates
            .iter()
            .any(|&i| self.passes[i].1[before..] == *values)
    }

    fn is_recorded_key(&self, key: &[u32]) -> bool {
        self.passes.iter().any(|(_, k)| k == key)
    }
}

/// A core of a span that is in a spin loop: where it is.
pub(crate) struct Plan {
    pub core: usize,
    pub spin: usize,
    pub phase: usize,
    pub candidates: Vec<usize>,
    /// The k-th instruction of the span (from 1) on this core runs after
    /// k - 1 + `offset` instruction issues of guest time: 0 when the core
    /// carries guest time, 1 for the secondary sharing the primary's call.
    pub offset: u64,
}

/// Guest time for `Plan::advance`: where the replayed timers start and how
/// the instruction clock advances.
pub(crate) struct Clock<'a> {
    /// The tick the bus's devices (and so `timer`) were advanced to.
    pub synced: u64,
    /// The oscillator tick after `n` more instruction issues.
    pub tick_after: &'a dyn Fn(u64) -> u64,
    /// The selected peripheral clock that TIMER4/5 count.
    pub peripheral_hz: u32,
}

/// What a span does to a planned core: the state it reaches, and each timer
/// it accessed with the tick that timer state is valid at.
pub(crate) struct Outcome {
    pub state: CoreState,
    pub timers: Vec<(u32, Timer, u64)>,
}

impl Plan {
    /// Replay the plan's timer accesses over `calls` calls as the bus would
    /// (`Bus::write` syncs a timer before a counter write, `Devices::read_at`
    /// reads it the elapsed ticks later) and find the state the core
    /// reaches. `timer(base)` is the device's state at `clock.synced`. None:
    /// some pass would read a value no recorded pass read, or a timer would
    /// reach its period (an event) within the span.
    pub fn advance(
        &self,
        spin: &SpinLoop,
        calls: u64,
        clock: &Clock,
        timer: impl Fn(u32) -> Timer,
    ) -> Option<Outcome> {
        let period = spin.period() as u64;
        let end_phase = ((self.phase as u64 + calls) % period) as usize;
        let hz = clock.peripheral_hz;
        let mut timers: Vec<(u32, Timer, u64)> = vec![];
        let mut values: Vec<u32> = vec![];
        // Instruction k (1..=calls) of the span runs phase
        // (phase + k - 1) % period. `first_k` is the k of the current
        // pass's `first_phase`; the first pass starts at the plan's phase.
        let mut first_k = 1u64;
        let mut first_phase = self.phase as u64;
        loop {
            for op in spin
                .timer_ops
                .iter()
                .filter(|op| op.phase as u64 >= first_phase)
            {
                let k = first_k + op.phase as u64 - first_phase;
                if k > calls {
                    break;
                }
                let at = (clock.tick_after)(k - 1 + self.offset);
                let index = match timers.iter().position(|(base, _, _)| *base == op.base) {
                    Some(index) => index,
                    None => {
                        timers.push((op.base, timer(op.base), clock.synced));
                        timers.len() - 1
                    }
                };
                let (_, state, valid_at) = &mut timers[index];
                match op.write {
                    Some(value) => {
                        if at > *valid_at {
                            let pending = state.pending;
                            // Spans end before the next device event, and
                            // events are at most 2^30 ticks apart.
                            state.advance_with_clock(u32::try_from(at - *valid_at).ok()?, hz);
                            if state.pending != pending {
                                return None;
                            }
                            *valid_at = at;
                        }
                        state.write(4, value)?.ok()?;
                    }
                    None => {
                        let value = if at == *valid_at {
                            state.read(op.offset)?
                        } else {
                            let mut later = state.clone();
                            later.advance_with_clock(u32::try_from(at - *valid_at).ok()?, hz);
                            if later.pending != state.pending {
                                return None;
                            }
                            later.read(op.offset)?
                        };
                        values.push(value);
                    }
                }
            }
            let last_k = first_k + period - first_phase - 1;
            if last_k > calls {
                break;
            }
            // A pass completed: it must be one that was recorded.
            let recorded = if first_k == 1 {
                spin.continues(self.phase, &self.candidates, &values)
            } else {
                spin.is_recorded_key(&values)
            };
            if !recorded {
                return None;
            }
            values.clear();
            first_k = last_k + 1;
            first_phase = 0;
        }
        // No replayed timer reaches its period before the span ends.
        let end = (clock.tick_after)(calls);
        for (_, state, valid_at) in &timers {
            if end > *valid_at {
                let mut later = state.clone();
                later.advance_with_clock(u32::try_from(end - *valid_at).ok()?, hz);
                if later.pending != state.pending {
                    return None;
                }
            }
        }
        let state = if first_k == 1 {
            // Still inside the pass the span started in.
            let before = spin.reads_before(self.phase);
            let upto = spin.reads_before(end_phase);
            self.candidates.iter().find_map(|&i| {
                let (states, key) = &spin.passes[i];
                (key[before..upto] == *values).then(|| states[end_phase].clone())
            })?
        } else {
            spin.state_at(end_phase, &values)?.clone()
        };
        Some(Outcome { state, timers })
    }
}
