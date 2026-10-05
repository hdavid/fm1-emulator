// SPDX-License-Identifier: GPL-3.0-only
// Host playback of the guest's ALNK0 DMA stream (audio.rs): 44.1 kHz stereo,
// 24-bit samples in the low bits of each i32 (Felucca audio.c: Q15 << 7, a
// "24-bit, -6 dBFS ceiling"), through the default output device. The worker
// fills a queue; the device callback drains it, resampling linearly when the
// device does not run at 44.1 kHz, and plays silence (counted) when empty.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const GUEST_RATE: f64 = 44_100.0;
/// Frames kept queued ahead of the device: enough to ride out a slow batch,
/// short enough that a key press is heard promptly (about 70 ms).
pub const TARGET_FRAMES: usize = 3_072;
/// A stall must not build up seconds of lag: older frames are dropped.
const MAX_FRAMES: usize = 16_384;

/// The playback queue, shared with the worker thread.
#[derive(Clone, Default)]
pub struct AudioQueue {
    frames: Arc<Mutex<VecDeque<[f32; 2]>>>,
}

impl AudioQueue {
    /// Guest frames (24-bit samples in i32) into the playback queue.
    pub fn push(&self, frames: impl Iterator<Item = [i32; 2]>) {
        const SCALE: f32 = 1.0 / 8_388_608.0; // 2^23: 24-bit full scale
        if let Ok(mut queue) = self.frames.lock() {
            queue.extend(frames.map(|[l, r]| [l as f32 * SCALE, r as f32 * SCALE]));
            let excess = queue.len().saturating_sub(MAX_FRAMES);
            queue.drain(..excess);
        }
    }

    pub fn queued(&self) -> usize {
        self.frames.lock().map_or(0, |queue| queue.len())
    }

    pub fn clear(&self) {
        if let Ok(mut queue) = self.frames.lock() {
            queue.clear();
        }
    }
}

/// The output stream; it stays on the thread that opened it.
pub struct HostAudio {
    pub queue: AudioQueue,
    /// Device callbacks that found the queue empty (audible as dropouts).
    underruns: Arc<AtomicU64>,
    _stream: cpal::Stream,
    pub device_rate: u32,
}

impl HostAudio {
    pub fn open() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output device")?;
        let config = Self::pick_config(&device)?;
        let device_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!(
                "audio output format {:?} is not supported (f32 only)",
                config.sample_format()
            ));
        }
        let queue = AudioQueue::default();
        let underruns = Arc::new(AtomicU64::new(0));
        let (frames, callback_underruns) = (queue.frames.clone(), underruns.clone());
        let step = GUEST_RATE / device_rate as f64;
        let mut position = 0.0f64; // fraction between queue[0] and queue[1]
        let stream = device
            .build_output_stream(
                &config.into(),
                move |out: &mut [f32], _| {
                    let Ok(mut queue) = frames.lock() else {
                        out.fill(0.0);
                        return;
                    };
                    let mut starved = false;
                    for frame in out.chunks_mut(channels) {
                        while position >= 1.0 && !queue.is_empty() {
                            queue.pop_front();
                            position -= 1.0;
                        }
                        let (left, right) = match (queue.front(), queue.get(1)) {
                            (Some(a), Some(b)) => {
                                let t = position as f32;
                                (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)
                            }
                            (Some(a), None) => (a[0], a[1]),
                            _ => {
                                starved = true;
                                (0.0, 0.0)
                            }
                        };
                        if !queue.is_empty() {
                            position += step;
                        }
                        for (channel, sample) in frame.iter_mut().enumerate() {
                            *sample = if channel % 2 == 0 { left } else { right };
                        }
                    }
                    if starved {
                        callback_underruns.fetch_add(1, Ordering::Relaxed);
                    }
                },
                |error| eprintln!("audio output: {error}"),
                None,
            )
            .map_err(|error| format!("audio output: {error}"))?;
        stream
            .play()
            .map_err(|error| format!("audio output: {error}"))?;
        Ok(Self {
            queue,
            underruns,
            _stream: stream,
            device_rate,
        })
    }

    /// 44.1 kHz f32 stereo when the device offers it, else its default.
    fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
        let wanted = cpal::SampleRate(GUEST_RATE as u32);
        if let Ok(configs) = device.supported_output_configs() {
            for range in configs {
                if range.sample_format() == cpal::SampleFormat::F32
                    && range.channels() >= 2
                    && range.min_sample_rate() <= wanted
                    && wanted <= range.max_sample_rate()
                {
                    return Ok(range.with_sample_rate(wanted));
                }
            }
        }
        device
            .default_output_config()
            .map_err(|error| format!("audio output: {error}"))
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_samples_are_24_bit_and_a_stall_drops_the_oldest_frames() {
        let queue = AudioQueue::default();
        queue.push([[1 << 23, -(1 << 22)]].into_iter());
        let first = queue.frames.lock().unwrap()[0];
        assert_eq!(first, [1.0, -0.5]);
        queue.push(std::iter::repeat_n([0, 0], MAX_FRAMES + 10));
        assert_eq!(queue.queued(), MAX_FRAMES);
        assert_eq!(queue.frames.lock().unwrap()[0], [0.0, 0.0]);
    }
}
