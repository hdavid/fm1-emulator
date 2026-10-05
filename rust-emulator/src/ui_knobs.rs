// SPDX-License-Identifier: GPL-3.0-only
// The panel's seven rotary encoders as matrix contacts. Each encoder's A and B
// contacts sit in the key matrix (gpio.rs) at the positions below, from
// Felucca's fm1_input.h FM1_ENC (physical wiring, so the same for every
// firmware). A queued detent plays one full quadrature cycle. Each phase is
// held until the guest has read both contacts' columns SCANS_PER_PHASE times:
// a firmware's scan rate in emulated time depends on its code and on the
// interpreter's speed, so pacing by its own reads works for any of them
// (Felucca's decoder needs two equal samples in a row).
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments (wiring).

/// Encoder i: A at (column, row), B at (column, row); row 0 is PA0, row 5 PB7.
pub const CONTACTS: [[usize; 4]; 7] = [
    [0, 0, 1, 0],
    [2, 0, 3, 0],
    [8, 1, 9, 1],
    [8, 0, 9, 0],
    [6, 0, 7, 0],
    [4, 0, 5, 0],
    [0, 5, 1, 5],
];
/// Matrix encoder behind each printed knob (Felucca panel.c defaults; the
/// physical wiring): SELECT 0, ALGORITHM 1, KNOB1-4 2-5, PRESETS 6.
pub mod knob {
    pub const SELECT: usize = 0;
    pub const ALGORITHM: usize = 1;
    pub const KNOB1: usize = 2;
    pub const PRESETS: usize = 6;
}
/// Guest reads of a contact's column per quadrature phase.
pub const SCANS_PER_PHASE: u32 = 3;
/// Detents that may queue up per encoder (a fast spin is not replayed for seconds).
const MAX_PENDING: i32 = 24;
/// Contacts (A closed, B closed) per phase; phase 0 is the detent (both open).
/// Clockwise is the order Felucca's decoder counts as + (00, 01, 11, 10).
const CLOCKWISE: [(bool, bool); 4] = [(false, false), (false, true), (true, true), (true, false)];
const COUNTER: [(bool, bool); 4] = [(false, false), (true, false), (true, true), (false, true)];

#[derive(Default, Clone)]
pub struct Encoders {
    pending: [i32; 7],
    phase: [u8; 7],
    direction: [i8; 7],
    /// Scans of the encoder's columns when its current phase began.
    since: [u32; 7],
}

impl Encoders {
    /// Queue `detents` clicks (+ clockwise) on encoder `e`.
    pub fn turn(&mut self, e: usize, detents: i32) {
        self.pending[e] = (self.pending[e] + detents).clamp(-MAX_PENDING, MAX_PENDING);
    }

    /// Advance with `scans(column)`, the guest's read count per matrix
    /// column so far; returns each encoder's (A, B) contacts.
    pub fn update(&mut self, scans: impl Fn(usize) -> u32) -> [(bool, bool); 7] {
        let mut contacts = [(false, false); 7];
        for e in 0..7 {
            let [a_column, _, b_column, _] = CONTACTS[e];
            let seen = scans(a_column).min(scans(b_column));
            let held = seen.wrapping_sub(self.since[e]) >= SCANS_PER_PHASE;
            if self.phase[e] == 0 {
                if self.pending[e] != 0 && held {
                    self.direction[e] = self.pending[e].signum() as i8;
                    self.phase[e] = 1;
                    self.since[e] = seen;
                }
            } else if held {
                if self.phase[e] == 3 {
                    // Back on the detent: the click is done; rest one phase.
                    self.phase[e] = 0;
                    self.pending[e] -= self.direction[e] as i32;
                } else {
                    self.phase[e] += 1;
                }
                self.since[e] = seen;
            }
            let table = if self.direction[e] >= 0 { &CLOCKWISE } else { &COUNTER };
            contacts[e] = table[self.phase[e] as usize];
        }
        contacts
    }
}

#[cfg(test)]
mod tests {
    use super::{knob::*, *};

    impl Encoders {
        fn busy(&self) -> bool {
            self.pending.iter().any(|&p| p != 0) || self.phase.iter().any(|&p| p != 0)
        }
    }

    /// Felucca's quadrature decoder (fm1_enc.h) without detent learning: rest
    /// at 00, a step on returning to it after >= 2 net transitions, and its
    /// two-sample filter.
    fn decode(states: &[(bool, bool)]) -> i32 {
        let (mut prev, mut last, mut sub, mut steps) = (0u32, 0u32, 0i32, 0i32);
        for &(a, b) in states {
            let cur = (a as u32) << 1 | b as u32;
            if cur != last {
                last = cur; // must be seen twice in a row
                continue;
            }
            if cur == prev {
                continue;
            }
            let idx = prev << 2 | cur;
            if (0x4182u32 >> idx) & 1 != 0 {
                sub += 1;
            } else if (0x2814u32 >> idx) & 1 != 0 {
                sub -= 1;
            }
            prev = cur;
            if cur == 0 {
                if sub >= 2 {
                    steps += 1;
                } else if sub <= -2 {
                    steps -= 1;
                }
                sub = 0;
            }
        }
        steps
    }

    /// A firmware scanning the whole matrix `scans` times, sampling encoder
    /// `e` once per scan, with the emulator updating contacts between reads.
    fn run(encoders: &mut Encoders, e: usize, scans: u32) -> Vec<(bool, bool)> {
        (0..scans).map(|n| encoders.update(|_| n)[e]).collect()
    }

    #[test]
    fn clockwise_and_counter_clockwise_detents_decode_as_felucca_counts_them() {
        let mut encoders = Encoders::default();
        encoders.turn(KNOB1, 3);
        assert_eq!(decode(&run(&mut encoders, KNOB1, 3 * 4 * SCANS_PER_PHASE + 4)), 3);
        assert!(!encoders.busy());
        let mut encoders = Encoders::default();
        encoders.turn(PRESETS, -2);
        assert_eq!(decode(&run(&mut encoders, PRESETS, 2 * 4 * SCANS_PER_PHASE + 4)), -2);
    }

    #[test]
    fn phases_wait_for_the_guest_to_scan_and_other_encoders_rest() {
        let mut encoders = Encoders::default();
        encoders.turn(SELECT, 1);
        // No scans: the contacts never leave the first phase.
        for _ in 0..100 {
            assert_eq!(encoders.update(|_| 0)[SELECT], (false, false));
        }
        let states = run(&mut encoders, SELECT, 4 * SCANS_PER_PHASE);
        let mut runs: Vec<((bool, bool), u32)> = vec![];
        for state in &states {
            match runs.last_mut() {
                Some((s, n)) if s == state => *n += 1,
                _ => runs.push((*state, 1)),
            }
        }
        assert!(runs.iter().all(|&(_, n)| n >= SCANS_PER_PHASE), "{runs:?}");
        assert_eq!(encoders.update(|_| 0)[ALGORITHM], (false, false));
    }

    #[test]
    fn a_fast_spin_is_capped() {
        let mut encoders = Encoders::default();
        encoders.turn(KNOB1, 1000);
        assert_eq!(encoders.pending[KNOB1], MAX_PENDING);
    }
}
