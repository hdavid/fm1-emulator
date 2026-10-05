// SPDX-License-Identifier: GPL-3.0-only
// The USB host model as a USB audio (UAC1) host, against a small scripted
// device that drives the USB0 registers as a firmware does (EP0 requests,
// isochronous IN on EP2 and EP3, isochronous OUT on EP2). The real firmware
// side is tests/usb_audio_firmware.rs.
use fm1_emu::{usb::Usb, usb_audio::NOMINAL_44K1, RAM, RAM_SIZE};

const EP0: u32 = RAM + 0x1000;
const TX2: u32 = RAM + 0x2000;
const RX2: u32 = RAM + 0x3000;
const FB3: u32 = RAM + 0x4000;
const TICKS_PER_MS: u64 = 24_000;

/// The SLOOP USB audio configuration (tests/usb_audio.rs has it annotated).
const CONFIG: [u8; 411] = [
    9, 2, 0x9B, 0x01, 6, 1, 0, 0x80, 50, 8, 0x0B, 0, 2, 1, 1, 0, 0, 9, 4, 0, 0, 0, 1, 1, 0, 0, 9,
    0x24, 1, 0x00, 0x01, 9, 0, 1, 1, 9, 4, 1, 0, 2, 1, 3, 0, 0, 7, 0x24, 1, 0x00, 0x01, 0x41, 0x00,
    6, 0x24, 2, 1, 1, 0, 6, 0x24, 2, 2, 2, 0, 9, 0x24, 3, 1, 3, 1, 2, 1, 0, 9, 0x24, 3, 2, 4, 1, 1,
    1, 0, 9, 5, 0x01, 2, 64, 0, 0, 0, 0, 5, 0x25, 1, 1, 1, 9, 5, 0x81, 2, 64, 0, 0, 0, 0, 5, 0x25,
    1, 1, 3, 8, 0x0B, 2, 2, 1, 1, 0, 3, 9, 4, 2, 0, 0, 1, 1, 0, 3, 9, 0x24, 1, 0x00, 0x01, 30, 0,
    1, 3, 12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 3, 0, 0, 0, 9, 0x24, 3, 2, 0x01, 0x03, 0, 1, 0, 9, 4,
    3, 0, 0, 1, 2, 0, 0, 9, 4, 3, 1, 2, 1, 2, 0, 0, 7, 0x24, 1, 1, 1, 1, 0, 11, 0x24, 2, 1, 2, 2,
    16, 1, 0x44, 0xAC, 0, 9, 5, 0x02, 0x05, 180, 0, 1, 0, 131, 7, 0x25, 1, 1, 0, 0, 0, 9, 5, 0x83,
    0x11, 3, 0, 1, 4, 0, 9, 4, 3, 2, 2, 1, 2, 0, 0, 7, 0x24, 1, 1, 1, 1, 0, 11, 0x24, 2, 1, 2, 3,
    24, 1, 0x44, 0xAC, 0, 9, 5, 0x02, 0x05, 14, 1, 1, 0, 131, 7, 0x25, 1, 1, 0, 0, 0, 9, 5, 0x83,
    0x11, 3, 0, 1, 4, 0, 8, 0x0B, 4, 2, 1, 1, 0, 4, 9, 4, 4, 0, 0, 1, 1, 0, 4, 9, 0x24, 1, 0x00,
    0x01, 30, 0, 1, 5, 12, 0x24, 2, 3, 0x13, 0x07, 0, 4, 0, 0, 0, 0, 9, 0x24, 3, 4, 0x01, 0x01, 0,
    3, 0, 9, 4, 5, 0, 0, 1, 2, 0, 0, 9, 4, 5, 1, 1, 1, 2, 0, 0, 7, 0x24, 1, 4, 1, 1, 0, 11, 0x24,
    2, 1, 4, 2, 16, 1, 0x44, 0xAC, 0, 9, 5, 0x82, 0x05, 104, 1, 1, 0, 0, 7, 0x25, 1, 1, 0, 0, 0, 9,
    4, 5, 2, 1, 1, 2, 0, 0, 7, 0x24, 1, 4, 1, 1, 0, 11, 0x24, 2, 1, 4, 3, 24, 1, 0x44, 0xAC, 0, 9,
    5, 0x82, 0x05, 28, 2, 1, 0, 0, 7, 0x25, 1, 1, 0, 0, 0,
];
const DEVICE: [u8; 18] = [
    18, 1, 0, 2, 0xEF, 2, 1, 64, 9, 0x12, 1, 0, 0x10, 3, 1, 2, 0, 1,
];
const PRODUCT: [u8; 12] = [12, 3, b'S', 0, b'L', 0, b'O', 0, b'O', 0, b'P', 0];

/// A device that answers as usb.c does, polled every 250 us like TIMER5.
struct Device {
    usb: Usb,
    ram: Vec<u8>,
    in_data: Option<(Vec<u8>, usize)>,
    rate_pending: bool,
    rate_data: Vec<Vec<u8>>,
    play_alt: u8,
    cap_alt: u8,
    /// Capture: arm a packet when the endpoint is free.
    arm_capture: bool,
    next_sample: i32,
    armed: u64,
    /// Playback: release the OUT buffer after reading it.
    release_out: bool,
    feedback: u32,
    received: Vec<i32>,
}

impl Device {
    fn new(usb: Usb) -> Self {
        let mut d = Device {
            usb,
            ram: vec![0; RAM_SIZE],
            in_data: None,
            rate_pending: false,
            rate_data: Vec::new(),
            play_alt: 0,
            cap_alt: 0,
            arm_capture: true,
            next_sample: 0,
            armed: 0,
            release_out: true,
            feedback: NOMINAL_44K1,
            received: Vec::new(),
        };
        d.reg(0x51000, 0x40);
        d.reg(0x11818, EP0);
        d.reg(0x11800, 4);
        d
    }
    fn reg(&mut self, a: u32, v: u32) {
        self.usb.write(a, v, &mut self.ram).unwrap().unwrap();
    }
    fn wr(&mut self, r: u32, v: u32) {
        self.reg(0x11804, (r << 8) | v);
    }
    fn rd(&mut self, r: u32) -> u32 {
        self.reg(0x11804, (r << 8) | 0x4000);
        self.usb.read(0x11804).unwrap() & 0xFF
    }
    fn mem(&mut self, a: u32, n: usize) -> &mut [u8] {
        let at = (a - RAM) as usize;
        &mut self.ram[at..at + n]
    }
    fn run_ms(&mut self, ms: u64) {
        for _ in 0..ms * 4 {
            self.usb
                .advance((TICKS_PER_MS / 4) as u32, &mut self.ram)
                .unwrap();
            self.poll();
        }
    }
    fn chunk(&mut self) {
        let (data, at) = self.in_data.take().unwrap();
        let n = (data.len() - at).min(64);
        self.mem(EP0, n).copy_from_slice(&data[at..at + n]);
        self.reg(0x11808, n as u32);
        let last = at + n == data.len();
        self.wr(17, if last { 0x0A } else { 0x02 });
        if !last {
            self.in_data = Some((data, at + n));
        }
    }
    fn send(&mut self, data: &[u8], wlength: usize) {
        self.in_data = Some((data[..data.len().min(wlength)].to_vec(), 0));
        self.wr(17, 0x40);
        self.chunk();
    }
    fn ep0(&mut self) {
        self.wr(14, 0);
        let csr = self.rd(17);
        if self.in_data.is_some() {
            if csr & 2 == 0 {
                self.chunk();
            }
            return;
        }
        if csr & 1 == 0 {
            return;
        }
        if self.rate_pending {
            let n = self.rd(22) as usize;
            let data = self.mem(EP0, n).to_vec();
            self.rate_data.push(data);
            self.rate_pending = false;
            self.wr(17, 0x48);
            return;
        }
        let s: [u8; 8] = self.mem(EP0, 8).try_into().unwrap();
        let wlength = u16::from_le_bytes([s[6], s[7]]) as usize;
        match (s[0], s[1]) {
            (0x80, 6) => match s[3] {
                1 => self.send(&DEVICE, wlength),
                2 => self.send(&CONFIG, wlength),
                3 if s[2] == 2 => self.send(&PRODUCT, wlength),
                _ => self.wr(17, 0x60),
            },
            (0x01, 0x0B) => {
                match s[4] {
                    3 => self.play_alt = s[2],
                    5 => self.cap_alt = s[2],
                    _ => {}
                }
                self.configure_streams();
                self.wr(14, 0);
                self.wr(17, 0x48);
            }
            (0x22, 1) => {
                self.rate_pending = true;
                self.wr(17, 0x40);
            }
            (0xA2, 0x81) => self.send(&[0x44, 0xAC, 0], wlength),
            _ => self.wr(17, 0x48),
        }
    }
    fn configure_streams(&mut self) {
        self.reg(0x11824, TX2); // EP2 TX buffer
        self.reg(0x11828, RX2); // EP2 RX buffer
        self.reg(0x1182C, FB3); // EP3 TX buffer
        self.wr(14, 2);
        self.wr(18, 0x40);
        self.wr(21, 0x40);
        self.wr(14, 3);
        self.wr(18, 0x40);
    }
    fn width(alt: u8) -> usize {
        if alt == 2 {
            3
        } else {
            2
        }
    }
    fn poll(&mut self) {
        let it = self.rd(2);
        if it & 1 != 0 {
            self.ep0();
        }
        if self.cap_alt != 0 && self.arm_capture {
            self.wr(14, 2);
            if self.rd(17) & 1 == 0 {
                let width = Self::width(self.cap_alt);
                let samples: Vec<i32> = (0..44 * 4)
                    .map(|_| {
                        self.next_sample += 1;
                        if width == 3 {
                            self.next_sample << 8
                        } else {
                            self.next_sample
                        }
                    })
                    .collect();
                let bytes = fm1_emu::usb_audio::encode_pcm(&samples, width);
                self.mem(TX2, bytes.len()).copy_from_slice(&bytes);
                self.reg(0x11810, bytes.len() as u32);
                self.wr(17, 1);
                self.armed += 1;
            }
        }
        if self.play_alt != 0 {
            self.wr(14, 3);
            if self.rd(17) & 1 == 0 {
                let fb = self.feedback.to_le_bytes();
                self.mem(FB3, 3).copy_from_slice(&fb[..3]);
                self.reg(0x11814, 3);
                self.wr(17, 1);
            }
            self.wr(14, 2);
            if self.release_out && self.rd(20) & 1 != 0 {
                let n = (self.rd(22) | self.rd(23) << 8) as usize;
                let bytes = self.mem(RX2, n).to_vec();
                let width = Self::width(self.play_alt);
                self.received
                    .extend(fm1_emu::usb_audio::decode_pcm(&bytes, width));
                self.wr(20, 0x10);
            }
        }
    }
}

fn streaming(alt: u8) -> Device {
    let mut usb = Usb::default();
    usb.enable_audio_host(alt);
    let mut dev = Device::new(usb);
    dev.run_ms(100);
    assert!(dev.usb.audio().streaming(), "the host opened both streams");
    dev
}

#[test]
fn a_cdc_less_composite_device_enumerates_with_the_plain_host_too() {
    let mut dev = Device::new(Usb::default());
    dev.run_ms(100);
    assert_eq!(
        dev.usb.setups, 4,
        "descriptor, address, configuration, configure"
    );
    assert!(!dev.usb.audio().streaming());
    assert_eq!((dev.play_alt, dev.cap_alt), (0, 0), "nobody opens a stream");
}

#[test]
fn the_audio_host_selects_both_streams_and_sets_and_reads_the_rate() {
    for alt in [1u8, 2] {
        let dev = streaming(alt);
        assert_eq!((dev.play_alt, dev.cap_alt), (alt, alt));
        assert_eq!(
            dev.rate_data,
            vec![vec![0x44, 0xAC, 0], vec![0x44, 0xAC, 0]],
            "SET_CUR sampling frequency data stages"
        );
        assert_eq!(dev.usb.audio().rate_replies, vec![44100, 44100]);
        assert!(dev.usb.midi_ready(), "USB-MIDI is opened as well");
        assert_eq!(dev.usb.product(), Some("SLOOP"));
    }
}

#[test]
fn capture_packets_leave_one_per_frame_in_order() {
    for alt in [1u8, 2] {
        let mut dev = streaming(alt);
        let (packets, armed) = (dev.usb.audio().capture_packets, dev.armed);
        dev.run_ms(50);
        let audio = dev.usb.audio();
        assert_eq!(audio.capture_packets - packets, 50, "one IN packet a frame");
        assert_eq!(dev.armed - armed, 50, "the device rearms after each");
        assert_eq!(audio.capture_missed, 0);
        assert_eq!(audio.capture_channels(), 4);
        let wide = if alt == 2 { 256 } else { 1 };
        let expected: Vec<i32> = (1..=audio.capture.len() as i32).map(|v| v * wide).collect();
        assert_eq!(audio.capture, expected, "samples in order, none twice");
    }
}

#[test]
fn a_frame_without_an_armed_capture_packet_is_counted_as_missed() {
    let mut dev = streaming(1);
    dev.run_ms(2);
    dev.arm_capture = false;
    dev.run_ms(6);
    let missed = dev.usb.audio().capture_missed;
    assert!((5..=6).contains(&missed), "missed {missed}");
}

#[test]
fn playback_packets_follow_the_device_feedback() {
    let mut dev = streaming(1);
    dev.usb
        .audio_mut()
        .play_queue
        .extend((0..10_000).map(|k| [k, -k]));
    let sent = dev.usb.audio().played.len();
    dev.feedback = NOMINAL_44K1 + 8192; // 44.6 frames a millisecond
    dev.run_ms(20); // the new rate reaches the host
    let start = dev.usb.audio().played.len();
    dev.run_ms(100);
    let audio = dev.usb.audio();
    let frames = audio.played.len() - start;
    assert!((4459..=4461).contains(&frames), "{frames} frames in 100 ms");
    assert_eq!(audio.play_lost, 0);
    let all: Vec<i32> = audio
        .played
        .iter()
        .flat_map(|f| f.iter().copied())
        .collect();
    assert!(
        all.len() - dev.received.len() <= 2 * 45,
        "the device read all but the last packet"
    );
    assert_eq!(
        dev.received,
        all[..dev.received.len()],
        "every frame once, in order"
    );
    assert_eq!(
        audio.played[sent..sent + 2],
        [[0, 0], [1, -1]],
        "the queue follows the silence"
    );
}

#[test]
fn an_out_packet_into_a_buffer_the_device_still_owns_is_lost() {
    let mut dev = streaming(1);
    dev.release_out = false;
    dev.run_ms(10);
    let lost = dev.usb.audio().play_lost;
    assert!((9..=10).contains(&lost), "lost {lost}");
}
