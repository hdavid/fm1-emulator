// SPDX-License-Identifier: GPL-3.0-only
// Functional ALNK0 DMA subset from Felucca's fm1_audio.h. The supported clock
// setup is 44.1 kHz; codec analog behavior and host audio playback are absent.
use crate::RAM;
use std::collections::VecDeque;
pub const BASE: u32 = 0x12e00;
pub const IRQ: usize = 11;
pub const SAMPLE_RATE: u64 = 44_100;

#[derive(Default)]
pub struct Audio {
    control: u32,
    config: u32,
    pending: u8,
    format: u8,
    address: u32,
    half_words: u32,
    clock: u32,
    iomap: u32,
    frame: u32,
    phase: u64,
    pub frames: u64,
    pub halves: u64,
    pub samples: VecDeque<[i32; 2]>,
    /// Onset probe (timing measurements): off while `threshold` is 0.
    pub probe: OnsetProbe,
}

/// Records when the DMA transfers a frame that rises out of silence: the
/// first frame with |left| or |right| >= `threshold` after at least `quiet`
/// frames below `threshold / 8` (and the first one after arming). Each onset
/// is (frame index, oscillator tick of its transfer).
#[derive(Default)]
pub struct OnsetProbe {
    pub threshold: i32,
    pub quiet: u32,
    below: u32,
    armed: bool,
    pub onsets: Vec<(u64, u64)>,
    /// (frame index, tick) of every half-buffer switch while the probe is on.
    pub halves: Vec<(u64, u64)>,
}
impl OnsetProbe {
    pub fn arm(&mut self, threshold: i32, quiet: u32) {
        self.threshold = threshold;
        self.quiet = quiet;
        self.below = quiet;
        self.armed = true;
        self.onsets.clear();
        self.halves.clear();
    }
    fn frame(&mut self, frame: u64, tick: u64, [l, r]: [i32; 2]) {
        let level = l.unsigned_abs().max(r.unsigned_abs());
        if level >= self.threshold as u32 {
            if self.armed && self.below >= self.quiet {
                self.onsets.push((frame, tick));
            }
            self.armed = true;
            self.below = 0;
        } else if level < (self.threshold / 8).max(1) as u32 {
            self.below = self.below.saturating_add(1);
        }
    }
}
impl Audio {
    pub fn read(&self, address: u32) -> Option<u32> {
        Some(match address {
            BASE => self.control,
            0x12e04 => self.config,
            0x12e08 => self.pending as u32,
            0x12e0c => self.format as u32,
            0x12e1c => self.address,
            0x12e20 => self.half_words,
            0x10014 => self.clock,
            0x51030 => self.iomap,
            0x14300 => 0, // SRC CON0: sample-rate converter remains disabled.
            _ => return None,
        })
    }
    pub fn write(&mut self, address: u32, value: u32) -> Option<Result<(), &'static str>> {
        self.read(address)?;
        match address {
            BASE => {
                let start = self.control & 0x800 == 0 && value & 0x800 != 0;
                self.control = (value & 0x7fff) | (self.control & 0x8000);
                if start {
                    if self.half_words == 0 || self.half_words & 1 != 0 {
                        return Some(Err("audio DMA requires an even, nonzero stereo word count"));
                    }
                    self.frame = 0;
                    self.phase = 0;
                    self.control &= !0x8000;
                }
            }
            0x12e04 => self.config = value & 0xffff,
            0x12e08 => self.pending &= !(((value & 15) << 4) as u8),
            0x12e0c => self.format = value as u8,
            0x12e1c => self.address = value,
            0x12e20 => self.half_words = value & 0xffff,
            0x10014 => {
                if value & 0xf00 != 0 {
                    return Some(Err("unsupported audio sample clock divider"));
                }
                self.clock = value;
            }
            0x51030 => self.iomap = value,
            0x14300 => {
                if value != 0 {
                    return Some(Err("hardware sample-rate conversion is not implemented"));
                }
            }
            _ => unreachable!(),
        }
        Some(Ok(()))
    }
    pub fn pending_irq(&self) -> bool {
        self.control & 0x800 != 0 && self.config & (1 << 14) != 0 && self.pending & 0x80 != 0
    }
    /// Oscillator ticks until the running DMA transfers its next frame.
    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        (self.control & 0x800 != 0).then(|| {
            (24_000_000u64.saturating_sub(self.phase))
                .div_ceil(SAMPLE_RATE)
                .max(1)
        })
    }
    /// `now`: the oscillator tick at the end of this span (a frame
    /// transfers at its end: spans end at `ticks_to_event`).
    pub fn advance(&mut self, ticks: u32, ram: &[u8], now: u64) -> Result<(), &'static str> {
        if self.control & 0x800 == 0 {
            return Ok(());
        }
        let start = self
            .address
            .checked_sub(RAM)
            .ok_or("audio DMA source must be in SRAM")? as usize;
        let length = self.half_words as usize * 8; // Two halves, four bytes per word.
        if self.address & 3 != 0 || start.checked_add(length).is_none_or(|end| end > ram.len()) {
            return Err("invalid audio DMA buffer");
        }
        self.phase += ticks as u64 * SAMPLE_RATE;
        while self.phase >= 24_000_000 {
            self.phase -= 24_000_000;
            let offset = start + self.frame as usize * 8;
            let left = i32::from_le_bytes(ram[offset..offset + 4].try_into().unwrap());
            let right = i32::from_le_bytes(ram[offset + 4..offset + 8].try_into().unwrap());
            if self.samples.len() == SAMPLE_RATE as usize {
                self.samples.pop_front();
            }
            self.samples.push_back([left, right]);
            if self.probe.threshold > 0 {
                self.probe.frame(self.frames, now, [left, right]);
            }
            self.frames += 1;
            self.frame += 1;
            let half_frames = self.half_words / 2;
            if self.frame == half_frames || self.frame == half_frames * 2 {
                self.control ^= 0x8000;
                self.pending |= 0x80;
                self.halves += 1;
                if self.probe.threshold > 0 {
                    self.probe.halves.push((self.frames, now));
                }
            }
            if self.frame == half_frames * 2 {
                self.frame = 0;
            }
        }
        Ok(())
    }
}
