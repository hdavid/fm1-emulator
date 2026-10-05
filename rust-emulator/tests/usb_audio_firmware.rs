// SPDX-License-Identifier: GPL-3.0-only
// A SLOOP USB audio build (sloop feat/usb-audio, FELUCCA_USB_AUDIO=1) against
// the USB audio host: enumeration and descriptor checks, the four capture
// stems against the firmware's own mix taps, host playback against the DAC,
// and USB-MIDI alongside. Needs the ELF for the tap symbols:
//   USB_AUDIO_ELF=~/GitHub/sloop-usbaudio/build/felucca.elf \
//     cargo test --release --test usb_audio_firmware -- --ignored --nocapture
// Optional: USB_AUDIO_CPU_MHZ (default 96; at 48 the render misses deadlines, see render_deadlines), USB_AUDIO_WAV_DIR (WAV dumps).
use fm1_emu::{
    cpu::Cpu,
    firmware::Firmware,
    usb_audio::wav_bytes,
    usb_midi::{encode, Decoder},
};
use std::{collections::BTreeMap, env, path::Path};

/// CTL: the frames of one mix block (ua_audio's n).
const BLOCK: usize = 32;
const TRACKS: usize = 4;
/// `ua` (usb_audio_stream.c): byte offsets of the fields read here.
const UA_PLAY_READY: u32 = 2;
const UA_PW: u32 = 4;
const UA_PR: u32 = 8;
const UA_CW: u32 = 12;
const UA_COUNTERS: u32 = 32; // play/cap under/overruns, bad packets
/// felucca_dbg.late: an audio half rendered after its DMA deadline.
const DBG_LATE: u32 = 20;

fn word(cpu: &Cpu, a: u32) -> u32 {
    cpu.bus.read(a, 4).unwrap()
}

fn clip(x: i32) -> i32 {
    x.clamp(-32768, 32767)
}

struct Run {
    cpu: Cpu,
    sym: BTreeMap<String, u32>,
    /// From each ua_audio entry: the clipped stems (4 per frame).
    stems: Vec<i32>,
    /// ...and the DAC frames ua_audio should leave (mix + playback).
    expected_out: Vec<[i32; 2]>,
    blocks_with_playback: usize,
    /// Every DAC frame, drained from the audio model (it keeps one second).
    dac: Vec<[i32; 2]>,
    /// Bytes per sample of the streams (2 or 3).
    width: usize,
}

impl Run {
    fn boot(alt: u8) -> Run {
        let path = env::var("USB_AUDIO_ELF")
            .expect("set USB_AUDIO_ELF to a USB audio build's felucca.elf");
        let firmware = Firmware::load(Path::new(&path)).unwrap();
        let mut bus = firmware.bus().unwrap();
        bus.usb.enable_audio_host(alt);
        let mut cpu = Cpu::new(bus, firmware.entry);
        cpu.r[0] = 0x01c7fe08;
        // TIMER5 (priority 4) must preempt the audio render (ALNK0, 3) to
        // serve the isochronous endpoints every millisecond.
        cpu.nested_irqs = true;
        let mhz = env::var("USB_AUDIO_CPU_MHZ").map_or(96, |v| v.parse().unwrap());
        cpu.set_cpu_mhz(mhz).unwrap();
        let mut run = Run {
            cpu,
            sym: firmware.symbols.clone(),
            stems: Vec::new(),
            expected_out: Vec::new(),
            blocks_with_playback: 0,
            dac: Vec::new(),
            width: alt as usize + 1,
        };
        for _ in 0..400 {
            if run.cpu.bus.usb.audio().streaming() {
                break;
            }
            run.cpu.run_steps(1_000_000).unwrap();
        }
        assert!(
            run.cpu.bus.usb.audio().streaming(),
            "the host opened the streams (config error: {:?})",
            run.cpu.bus.usb.audio().config_error
        );
        run
    }
    fn sym(&self, name: &str) -> u32 {
        *self
            .sym
            .get(name)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    }
    /// Run `steps` instructions one by one, tapping each ua_audio call.
    fn run(&mut self, steps: u64) {
        let (entry, capture, ua) = (
            self.sym("ua_audio"),
            self.sym("track_capture"),
            self.sym("ua"),
        );
        for _ in 0..steps {
            if self.cpu.pc == entry {
                self.tap(capture, ua);
                self.dac.extend(self.cpu.bus.audio.samples.drain(..));
            }
            self.cpu.step().unwrap();
        }
    }
    fn tap(&mut self, capture: u32, ua: u32) {
        let (out, master) = (self.cpu.r[0], self.cpu.r[1] as i32);
        let cpu = &self.cpu;
        let byte = |a: u32| cpu.bus.read(a, 1).unwrap();
        if byte(ua + 1) != 0 {
            if word(cpu, ua + UA_CW) == 0 {
                self.stems.clear(); // a (re)started capture ring
            }
            for i in 0..BLOCK * TRACKS {
                self.stems
                    .push(clip(word(cpu, capture + 4 * i as u32) as i32));
            }
        }
        let (pw, pr) = (word(cpu, ua + UA_PW), word(cpu, ua + UA_PR));
        let ready = byte(ua + UA_PLAY_READY) != 0 || pw.wrapping_sub(pr) >= 512;
        let playing = byte(ua) != 0 && ready && pw.wrapping_sub(pr) >= BLOCK as u32;
        let played = &self.cpu.bus.usb.audio().played;
        for i in 0..BLOCK {
            let mut f = [0; 2];
            for (c, v) in f.iter_mut().enumerate() {
                let mix = word(cpu, out + 4 * (2 * i + c) as u32) as i32;
                *v = if playing {
                    let p = played[(pr as usize) + i][c] >> (8 * (self.width - 2));
                    clip(mix + ((p * master) >> 12))
                } else {
                    mix
                };
            }
            self.expected_out.push(f);
        }
        self.blocks_with_playback += usize::from(playing);
    }
    fn counters(&self) -> Vec<u32> {
        let ua = self.sym("ua");
        (0..5)
            .map(|k| word(&self.cpu, ua + UA_COUNTERS + 4 * k))
            .collect()
    }
    fn late(&self) -> u32 {
        word(&self.cpu, self.sym("felucca_dbg") + DBG_LATE)
    }
    fn midi(&mut self, message: &[u8]) {
        self.cpu.bus.usb_midi_send(&encode(message, 0));
    }
}

/// The offset at which `long` contains all of `short` (exactly).
fn find(long: &[i32], short: &[i32], step: usize) -> Option<usize> {
    (0..=long.len().saturating_sub(short.len()))
        .step_by(step)
        .find(|&o| long[o..o + short.len()] == *short)
}

fn dump(name: &str, channels: u16, bits: u16, samples: &[i32]) {
    if let Ok(dir) = env::var("USB_AUDIO_WAV_DIR") {
        let path = Path::new(&dir).join(name);
        std::fs::write(&path, wav_bytes(44100, channels, bits, samples)).unwrap();
        println!("wrote {}", path.display());
    }
}

fn session(alt: u8) {
    let mut run = Run::boot(alt);
    let mhz = env::var("USB_AUDIO_CPU_MHZ").map_or(96u64, |v| v.parse().unwrap());
    let ms = mhz * 1000; // instructions a millisecond, at most
                         // Host playback: silence, then a ramp pattern that never repeats within a block.
    let audio = run.cpu.bus.usb.audio_mut();
    // At 24 bit the low byte carries noise the device must discard.
    audio.play_queue.extend((0..44100).map(|k: i32| {
        let v = ((k * 97) % 4001 - 2000) * 4;
        let f = [v, -v / 2];
        if alt == 2 {
            f.map(|x| x << 8 | (k & 0xFF))
        } else {
            f
        }
    }));
    let late0 = run.late();
    run.cpu.bus.audio.samples.clear();
    run.run(200 * ms);
    for msg in [
        [0x90u8, 60, 100],
        [0x91, 64, 100],
        [0x92, 67, 100],
        [0x99, 36, 120],
        [0x99, 38, 100],
    ] {
        run.midi(&msg);
        run.run(150 * ms);
    }
    for msg in [[0x80u8, 60, 0], [0x81, 64, 0], [0x82, 67, 0]] {
        run.midi(&msg);
    }
    // USB-MIDI SysEx while streaming: the web editor's INFO request.
    let mut decoder = Decoder::default();
    run.cpu.bus.usb.midi_received.clear();
    run.midi(&[0xF0, 0x7D, 0x46, 0x4C, 1, 0xF7]);
    let mut replies: Vec<Vec<u8>> = Vec::new();
    for _ in 0..15 {
        run.run(100 * ms);
        let packets: Vec<_> = run.cpu.bus.usb.midi_received.drain(..).collect();
        replies.extend(packets.into_iter().filter_map(|p| decoder.push(p)));
    }
    assert!(
        replies
            .iter()
            .any(|m| m.starts_with(&[0xF0, 0x7D, 0x46, 0x4C, 1])),
        "the editor's INFO reply comes back over USB-MIDI while audio streams"
    );

    let host = run.cpu.bus.usb.audio();
    let captured = host.capture.clone();
    let shift = if alt == 2 { 8 } else { 0 };
    if alt == 2 {
        assert!(
            captured.iter().all(|s| s & 0xFF == 0),
            "24-bit capture pads the low byte"
        );
    }
    let captured16: Vec<i32> = captured.iter().map(|s| s >> shift).collect();
    println!(
        "alt {alt}: {} capture packets ({}..{} frames), missed {}, {} playback packets, lost {}, feedback {:.4} frames/ms",
        host.capture_packets,
        host.capture_frames.0,
        host.capture_frames.1,
        host.capture_missed,
        host.play_packets,
        host.play_lost,
        host.feedback as f64 / 16384.0
    );
    assert_eq!(host.play_lost, 0, "the device took every playback packet");
    assert!(host.capture_frames.0 >= 43 && host.capture_frames.1 <= 45);
    // 1. Capture: after the priming silence, exactly the stems ua_audio was given.
    let first = captured16
        .chunks(TRACKS)
        .position(|f| f.iter().any(|s| *s != 0))
        .expect("the captured streams carry sound");
    let body = &captured16[first * TRACKS..];
    let at =
        find(&run.stems, &body[..TRACKS * 64], TRACKS).expect("captured frames are tap frames");
    let n = body.len().min(run.stems.len() - at);
    assert!(n > TRACKS * 44100 / 2, "half a second compared");
    assert_eq!(
        body[..n],
        run.stems[at..at + n],
        "every capture frame equals its mix tap"
    );
    // Isolation: each track's channel sounds only when it plays (taps are per track).
    for ch in 0..TRACKS {
        let peak = body[..n]
            .iter()
            .skip(ch)
            .step_by(TRACKS)
            .map(|s| s.abs())
            .max()
            .unwrap();
        println!("stem {}: peak {peak}", ch + 1);
        assert!(peak > 100, "track {} reached its USB input", ch + 1);
    }
    // 2. Playback: every DAC frame is the mix plus the host's audio, x MASTER.
    let dac: Vec<i32> = run
        .dac
        .iter()
        .flat_map(|f| f.iter().map(|s| s >> 7))
        .collect();
    assert!(
        run.dac.iter().flatten().all(|s| s & 127 == 0),
        "Q15 << 7 on the I2S"
    );
    let expected: Vec<i32> = run
        .expected_out
        .iter()
        .flat_map(|f| f.iter().copied())
        .collect();
    assert!(
        run.blocks_with_playback > 100,
        "the host's audio reached the mix"
    );
    let probe = &expected[expected.len() / 2..expected.len() / 2 + 2 * 256];
    let o = find(&dac, probe, 2).expect("DAC frames are the ua_audio output");
    let lag = o as isize - (expected.len() / 2) as isize; // DAC index - expected index
    let from = lag.max(0) as usize;
    let to = dac.len().min((expected.len() as isize + lag) as usize);
    let (e_from, e_to) = ((from as isize - lag) as usize, (to as isize - lag) as usize);
    println!(
        "DAC: {} frames compared, {} blocks with host playback",
        (to - from) / 2,
        run.blocks_with_playback
    );
    assert!(to - from > expected.len() / 2);
    assert_eq!(
        dac[from..to],
        expected[e_from..e_to],
        "DAC = mix + playback x MASTER, every frame"
    );
    // 3. No glitches: no ring under/overruns, no late audio half.
    assert_eq!(
        run.counters(),
        vec![0; 5],
        "play under/over, capture under/over, bad packets"
    );
    assert_eq!(run.late() - late0, 0, "the DAC path met every deadline");
    dump(
        &format!("usb-capture-alt{alt}.wav"),
        4,
        8 * (alt as u16 + 1),
        &captured,
    );
    dump(&format!("dac-alt{alt}.wav"), 2, 16, &dac);
}

#[test]
#[ignore = "requires USB_AUDIO_ELF; run in release mode with --ignored"]
fn sloop_usb_audio_16_bit() {
    session(1);
}

#[test]
#[ignore = "requires USB_AUDIO_ELF; run in release mode with --ignored"]
fn sloop_usb_audio_24_bit() {
    session(2);
}

/// Audio halves rendered late and the longest render (us), for the same
/// notes, with the audio host streaming (if the build has USB audio) or not.
fn deadlines(path: &str, mhz: u32) -> (u32, u32, u32) {
    let firmware = Firmware::load(Path::new(path)).unwrap();
    let mut bus = firmware.bus().unwrap();
    bus.usb.enable_audio_host(1);
    let mut cpu = Cpu::new(bus, firmware.entry);
    cpu.r[0] = 0x01c7fe08;
    cpu.nested_irqs = true;
    cpu.set_cpu_mhz(mhz).unwrap();
    let dbg = firmware.symbols["felucca_dbg"];
    let ms = mhz as u64 * 1000;
    cpu.run_steps(1500 * ms).unwrap();
    let late0 = word(&cpu, dbg + DBG_LATE);
    cpu.bus.write(dbg + 8, 0, 4).unwrap(); // max_us from here
    for msg in [
        [0x90u8, 60, 100],
        [0x91, 64, 100],
        [0x92, 67, 100],
        [0x99, 36, 120],
        [0x99, 38, 100],
    ] {
        cpu.bus.usb_midi_send(&encode(&msg, 0));
        cpu.run_steps(150 * ms).unwrap();
    }
    cpu.run_steps(500 * ms).unwrap();
    let streams = u32::from(cpu.bus.usb.audio().streaming());
    (
        word(&cpu, dbg + DBG_LATE) - late0,
        word(&cpu, dbg + 8),
        streams,
    )
}

#[test]
#[ignore = "requires USB_AUDIO_ELF and USB_PLAIN_ELF (a FELUCCA_USB_AUDIO=0 build); --ignored"]
fn render_deadlines_with_and_without_usb_audio() {
    let usb = env::var("USB_AUDIO_ELF").unwrap();
    let plain = env::var("USB_PLAIN_ELF").unwrap();
    for mhz in [48, 72, 96, 192, 312] {
        let (pl, pm, _) = deadlines(&plain, mhz);
        let (ul, um, streaming) = deadlines(&usb, mhz);
        assert_eq!(streaming, 1);
        println!("{mhz} MHz: plain late {pl}, max {pm} us | USB audio late {ul}, max {um} us");
        if mhz >= 96 {
            assert_eq!(
                ul, 0,
                "no late audio half with USB audio streaming at {mhz} MHz"
            );
        }
    }
}
