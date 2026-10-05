// SPDX-License-Identifier: GPL-3.0-only
// Host playback of the guest's ALNK0 DMA stream (audio.rs): 44.1 kHz stereo,
// 24-bit left-justified in i32, through the default output device. The guest
// fills a queue; the device callback drains it, resampling linearly when the
// device does not run at 44.1 kHz, and plays silence (counted) when it is empty.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const GUEST_RATE: f64 = 44_100.0;
/// Frames kept queued ahead of the device: enough to ride out a slow UI
/// frame, short enough that a key press is heard promptly (~70 ms).
pub const TARGET_FRAMES: usize = 3_072;
/// Above this the guest is paused for the rest of the UI frame (~190 ms).
pub const MAX_FRAMES: usize = 8_192;

#[derive(Default)]
struct Shared {
    queue: VecDeque<[f32; 2]>,
}

pub struct HostAudio {
    shared: Arc<Mutex<Shared>>,
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
        let shared = Arc::new(Mutex::new(Shared::default()));
        let underruns = Arc::new(AtomicU64::new(0));
        let (cb_shared, cb_underruns) = (shared.clone(), underruns.clone());
        let step = GUEST_RATE / device_rate as f64;
        let mut position = 0.0f64; // fraction between queue[0] and queue[1]
        let stream = device
            .build_output_stream(
                &config.into(),
                move |out: &mut [f32], _| {
                    let Ok(mut shared) = cb_shared.lock() else {
                        out.fill(0.0);
                        return;
                    };
                    let mut starved = false;
                    for frame in out.chunks_mut(channels) {
                        let queue = &mut shared.queue;
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
                        cb_underruns.fetch_add(1, Ordering::Relaxed);
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
            shared,
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

    /// Guest frames (24-bit left-justified i32) into the playback queue.
    pub fn push(&self, frames: impl Iterator<Item = [i32; 2]>) {
        const SCALE: f32 = 1.0 / 2_147_483_648.0;
        if let Ok(mut shared) = self.shared.lock() {
            shared
                .queue
                .extend(frames.map(|[l, r]| [l as f32 * SCALE, r as f32 * SCALE]));
            let excess = shared.queue.len().saturating_sub(MAX_FRAMES * 2);
            shared.queue.drain(..excess); // a stall must not build seconds of lag
        }
    }

    pub fn queued(&self) -> usize {
        self.shared.lock().map_or(0, |shared| shared.queue.len())
    }

    pub fn clear(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.queue.clear();
        }
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}
