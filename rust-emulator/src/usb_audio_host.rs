// SPDX-License-Identifier: GPL-3.0-only
//! The USB audio side of the host model (usb.rs): after enumeration it
//! selects one alternate of each UAC1 stream, sets and reads the sampling
//! rate as macOS does, then every 1 ms frame takes the capture and feedback
//! IN packets and sends a playback OUT packet sized by the device's 10.14
//! feedback (an asynchronous stream). Off unless enabled.
use crate::usb_audio::{
    decode_pcm, encode_pcm, parse_uac1, AudioFunctions, FeedbackClock, NOMINAL_44K1,
};
use std::collections::VecDeque;

#[derive(Default)]
pub struct AudioHost {
    enabled: bool,
    alt: u8,
    functions: AudioFunctions,
    /// The configuration failed the class checks (the host opens nothing).
    pub config_error: Option<String>,
    /// Control requests after enumeration, with their OUT data stages.
    plan: Vec<([u8; 8], Option<Vec<u8>>)>,
    done: usize,
    stage: Option<Vec<u8>>,
    stage_wanted: bool,
    /// GET_CUR sampling frequency replies, in request order.
    pub rate_replies: Vec<u32>,
    /// Captured samples, interleaved (capture_channels() per frame), at
    /// full width: a 24-bit stream's samples are 24-bit values.
    pub capture: Vec<i32>,
    pub capture_packets: u64,
    /// Frames whose IN token found no armed capture packet.
    pub capture_missed: u64,
    /// Frames per capture packet: smallest and largest seen.
    pub capture_frames: (u32, u32),
    /// Frames to play (host to device); silence when empty.
    pub play_queue: VecDeque<[i32; 2]>,
    /// Frames the device was sent, in order (silence included).
    pub played: Vec<[i32; 2]>,
    pub play_packets: u64,
    /// OUT packets dropped because the device had not released its buffer.
    pub play_lost: u64,
    /// The last valid 10.14 feedback (nominal until the first).
    pub feedback: u32,
    pub feedback_packets: u64,
    pub feedback_missed: u64,
    clock: FeedbackClock,
    pending_out: Option<usize>,
}

impl AudioHost {
    pub(crate) fn enable(&mut self, alt: u8) {
        self.enabled = true;
        self.alt = alt;
        self.feedback = NOMINAL_44K1;
        self.capture_frames = (u32::MAX, 0);
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    /// The streams the configuration offers (after enumeration).
    pub fn functions(&self) -> &AudioFunctions {
        &self.functions
    }
    /// All requests answered: the streams run.
    pub fn streaming(&self) -> bool {
        self.enabled && !self.plan.is_empty() && self.done == self.plan.len()
    }
    pub fn capture_channels(&self) -> usize {
        self.capture_alt().map_or(0, |a| a.channels as usize)
    }
    fn capture_alt(&self) -> Option<&crate::usb_audio::AltSetting> {
        self.functions.capture.as_ref()?.alt(self.alt)
    }
    fn play_alt(&self) -> Option<&crate::usb_audio::AltSetting> {
        self.functions.playback.as_ref()?.alt(self.alt)
    }
    /// From the configuration descriptor: the class checks, then the plan of
    /// SET_INTERFACE, SET_CUR and GET_CUR (sampling frequency) per stream.
    pub(crate) fn configure(&mut self, config: &[u8]) {
        self.plan.clear();
        self.functions = match parse_uac1(config) {
            Ok(f) => f,
            Err(e) => {
                self.config_error = Some(e);
                return;
            }
        };
        let streams = [&self.functions.capture, &self.functions.playback];
        for stream in streams.into_iter().flatten() {
            let Some(alt) = stream.alt(self.alt) else {
                continue;
            };
            let rate = alt.rate.to_le_bytes();
            let ep = alt.endpoint;
            self.plan
                .push(([1, 0x0B, self.alt, 0, stream.interface, 0, 0, 0], None));
            self.plan
                .push(([0x22, 1, 0, 1, ep, 0, 3, 0], Some(rate[..3].to_vec())));
            self.plan.push(([0xA2, 0x81, 0, 1, ep, 0, 3, 0], None));
        }
    }
    pub(crate) fn requests(&self) -> usize {
        if self.enabled {
            self.plan.len()
        } else {
            0
        }
    }
    pub(crate) fn setup(&self, k: usize) -> Option<[u8; 8]> {
        self.plan.get(k).map(|(setup, _)| *setup)
    }
    /// Request `k` went out: its OUT data stage (if any) waits for the device.
    pub(crate) fn begin(&mut self, k: usize) {
        self.stage = self.plan.get(k).and_then(|(_, stage)| stage.clone());
        self.stage_wanted = false;
    }
    pub(crate) fn completed(&mut self, k: usize, response: &[u8]) {
        if self.plan.get(k).is_some_and(|(s, _)| s[0] == 0xA2) && response.len() == 3 {
            self.rate_replies.push(u32::from_le_bytes([
                response[0],
                response[1],
                response[2],
                0,
            ]));
        }
        self.done = self.done.max(k + 1);
    }
    /// The device serviced the SETUP of a request with an OUT data stage.
    pub(crate) fn data_stage_wanted(&mut self) {
        self.stage_wanted = self.stage.is_some();
    }
    pub(crate) fn data_stage_pending(&self) -> bool {
        self.stage_wanted
    }
    pub(crate) fn take_data_stage(&mut self) -> Vec<u8> {
        self.stage_wanted = false;
        self.stage.take().unwrap_or_default()
    }
    /// The IN endpoints polled every frame: capture data, then feedback.
    pub(crate) fn in_endpoints(&self) -> Vec<usize> {
        let mut eps = Vec::new();
        if let Some(c) = self.capture_alt() {
            eps.push((c.endpoint & 15) as usize);
        }
        if let Some(f) = self.play_alt().and_then(|p| p.feedback) {
            eps.push((f & 15) as usize);
        }
        eps
    }
    fn is_capture(&self, ep: usize) -> bool {
        self.capture_alt()
            .is_some_and(|c| (c.endpoint & 15) as usize == ep)
    }
    pub(crate) fn in_missed(&mut self, ep: usize) {
        if self.is_capture(ep) {
            self.capture_missed += 1;
        } else {
            self.feedback_missed += 1;
        }
    }
    pub(crate) fn in_packet(&mut self, ep: usize, bytes: &[u8]) {
        if self.is_capture(ep) {
            let (ch, width) = self
                .capture_alt()
                .map_or((1, 2), |c| (c.channels as usize, c.width as usize));
            let frames = (bytes.len() / (ch * width)) as u32;
            self.capture_frames = (
                self.capture_frames.0.min(frames),
                self.capture_frames.1.max(frames),
            );
            let usable = frames as usize * ch * width;
            self.capture.extend(decode_pcm(&bytes[..usable], width));
            self.capture_packets += 1;
        } else if bytes.len() >= 3 {
            let v = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
            // 10.14 within +-1 frame of nominal: anything else a host ignores.
            if v.abs_diff(NOMINAL_44K1) <= 16384 {
                self.feedback = v;
            }
            self.feedback_packets += 1;
        }
    }
    /// This frame's playback packet: (endpoint, bytes).
    pub(crate) fn out_packet(&mut self) -> Option<(usize, Vec<u8>)> {
        let (ep, width) = self
            .play_alt()
            .map(|p| ((p.endpoint & 15) as usize, p.width as usize))?;
        let n = self.clock.frames(self.feedback) as usize;
        let frames: Vec<[i32; 2]> = (0..n)
            .map(|_| self.play_queue.pop_front().unwrap_or([0, 0]))
            .collect();
        let samples: Vec<i32> = frames.iter().flat_map(|f| f.iter().copied()).collect();
        self.pending_out = Some(self.played.len());
        self.played.extend(frames);
        Some((ep, encode_pcm(&samples, width)))
    }
    pub(crate) fn out_delivered(&mut self) {
        self.pending_out = None;
        self.play_packets += 1;
    }
    /// The device still owned its buffer: these frames never arrive.
    pub(crate) fn out_lost(&mut self) {
        if let Some(at) = self.pending_out.take() {
            self.played.truncate(at);
        }
        self.play_lost += 1;
    }
}
