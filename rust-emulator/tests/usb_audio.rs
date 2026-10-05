// SPDX-License-Identifier: GPL-3.0-only
// USB Audio Class 1.0 (UAC1) host side: configuration parsing and the checks
// a class driver makes before it opens a stream, the 10.14 explicit-feedback
// frame clock, PCM packing and WAV dumps.
use fm1_emu::usb_audio::{
    decode_pcm, encode_pcm, parse_uac1, wav_bytes, FeedbackClock, NOMINAL_44K1,
};

/// The configuration of the SLOOP USB audio build (sloop feat/usb-audio,
/// firmware/src/usb.c + usb_audio_desc.h, the layout Melodee uses): USB-MIDI
/// (IF 0-1), a playback function (IF 2-3) and a capture function (IF 4-5).
const SLOOP: [u8; 411] = [
    9, 2, 0x9B, 0x01, 6, 1, 0, 0x80, 50, //
    8, 0x0B, 0, 2, 1, 1, 0, 0, //
    9, 4, 0, 0, 0, 1, 1, 0, 0, //
    9, 0x24, 1, 0x00, 0x01, 9, 0, 1, 1, //
    9, 4, 1, 0, 2, 1, 3, 0, 0, //
    7, 0x24, 1, 0x00, 0x01, 0x41, 0x00, //
    6, 0x24, 2, 1, 1, 0, //
    6, 0x24, 2, 2, 2, 0, //
    9, 0x24, 3, 1, 3, 1, 2, 1, 0, //
    9, 0x24, 3, 2, 4, 1, 1, 1, 0, //
    9, 5, 0x01, 2, 64, 0, 0, 0, 0, //
    5, 0x25, 1, 1, 1, //
    9, 5, 0x81, 2, 64, 0, 0, 0, 0, //
    5, 0x25, 1, 1, 3, //
    8, 0x0B, 2, 2, 1, 1, 0, 3, //
    9, 4, 2, 0, 0, 1, 1, 0, 3, //
    9, 0x24, 1, 0x00, 0x01, 30, 0, 1, 3, //
    12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 3, 0, 0, 0, //
    9, 0x24, 3, 2, 0x01, 0x03, 0, 1, 0, //
    9, 4, 3, 0, 0, 1, 2, 0, 0, //
    9, 4, 3, 1, 2, 1, 2, 0, 0, //
    7, 0x24, 1, 1, 1, 1, 0, //
    11, 0x24, 2, 1, 2, 2, 16, 1, 0x44, 0xAC, 0, //
    9, 5, 0x02, 0x05, 180, 0, 1, 0, 131, //
    7, 0x25, 1, 1, 0, 0, 0, //
    9, 5, 0x83, 0x11, 3, 0, 1, 4, 0, //
    9, 4, 3, 2, 2, 1, 2, 0, 0, //
    7, 0x24, 1, 1, 1, 1, 0, //
    11, 0x24, 2, 1, 2, 3, 24, 1, 0x44, 0xAC, 0, //
    9, 5, 0x02, 0x05, 14, 1, 1, 0, 131, //
    7, 0x25, 1, 1, 0, 0, 0, //
    9, 5, 0x83, 0x11, 3, 0, 1, 4, 0, //
    8, 0x0B, 4, 2, 1, 1, 0, 4, //
    9, 4, 4, 0, 0, 1, 1, 0, 4, //
    9, 0x24, 1, 0x00, 0x01, 30, 0, 1, 5, //
    12, 0x24, 2, 3, 0x13, 0x07, 0, 4, 0, 0, 0, 0, //
    9, 0x24, 3, 4, 0x01, 0x01, 0, 3, 0, //
    9, 4, 5, 0, 0, 1, 2, 0, 0, //
    9, 4, 5, 1, 1, 1, 2, 0, 0, //
    7, 0x24, 1, 4, 1, 1, 0, //
    11, 0x24, 2, 1, 4, 2, 16, 1, 0x44, 0xAC, 0, //
    9, 5, 0x82, 0x05, 104, 1, 1, 0, 0, //
    7, 0x25, 1, 1, 0, 0, 0, //
    9, 4, 5, 2, 1, 1, 2, 0, 0, //
    7, 0x24, 1, 4, 1, 1, 0, //
    11, 0x24, 2, 1, 4, 3, 24, 1, 0x44, 0xAC, 0, //
    9, 5, 0x82, 0x05, 28, 2, 1, 0, 0, //
    7, 0x25, 1, 1, 0, 0, 0, //
];

fn find(config: &[u8], pattern: &[u8]) -> usize {
    config
        .windows(pattern.len())
        .position(|w| w == pattern)
        .expect("pattern in the configuration")
}

#[test]
fn the_sloop_configuration_has_a_stereo_playback_and_a_four_channel_capture_stream() {
    let functions = parse_uac1(&SLOOP).unwrap();
    let play = functions.playback.expect("a playback stream");
    let cap = functions.capture.expect("a capture stream");
    assert_eq!((play.interface, cap.interface), (3, 5));
    assert_eq!(play.function_name, 3, "iInterface of its audio control");
    assert_eq!(cap.function_name, 4);
    for (alt, width) in [(1u8, 2u8), (2, 3)] {
        let p = play.alt(alt).unwrap();
        assert_eq!(
            (p.channels, p.width, p.bits, p.rate, p.endpoint),
            (2, width, width * 8, 44100, 0x02)
        );
        assert_eq!(p.max_packet as usize, 45 * 2 * width as usize);
        assert_eq!(p.feedback, Some(0x83));
        let c = cap.alt(alt).unwrap();
        assert_eq!(
            (c.channels, c.width, c.bits, c.rate, c.endpoint),
            (4, width, width * 8, 44100, 0x82)
        );
        assert_eq!(c.max_packet as usize, 45 * 4 * width as usize);
        assert_eq!(
            c.feedback, None,
            "capture is asynchronous, sized by the device"
        );
    }
}

#[test]
fn a_wrong_total_length_or_interface_count_is_rejected() {
    let mut bad = SLOOP;
    bad[2] = 0x9A;
    assert!(parse_uac1(&bad).unwrap_err().contains("wTotalLength"));
    let mut bad = SLOOP;
    bad[4] = 5;
    assert!(parse_uac1(&bad).unwrap_err().contains("bNumInterfaces"));
}

#[test]
fn an_ac_header_must_list_its_streaming_interface_and_cover_its_terminals() {
    let mut bad = SLOOP;
    let header = find(&SLOOP, &[9, 0x24, 1, 0x00, 0x01, 30, 0, 1, 5]);
    bad[header + 8] = 3; // the capture control claims the playback stream
    assert!(parse_uac1(&bad).unwrap_err().contains("baInterfaceNr"));
    let mut bad = SLOOP;
    bad[header + 5] = 29;
    assert!(parse_uac1(&bad).unwrap_err().contains("AC wTotalLength"));
}

#[test]
fn a_stream_must_link_to_a_usb_streaming_terminal_with_its_channel_count() {
    let mut bad = SLOOP;
    // capture AS_GENERAL bTerminalLink 4 (the USB streaming OT) -> 3 (the synth IT)
    let general = find(&SLOOP, &[7, 0x24, 1, 4, 1, 1, 0]);
    bad[general + 3] = 3;
    assert!(parse_uac1(&bad).unwrap_err().contains("bTerminalLink"));
    let mut bad = SLOOP;
    let format = find(&SLOOP, &[11, 0x24, 2, 1, 4, 2, 16]);
    bad[format + 4] = 2; // two channels from a four-channel terminal
    assert!(parse_uac1(&bad).unwrap_err().contains("channels"));
}

#[test]
fn an_asynchronous_out_endpoint_needs_its_feedback_endpoint() {
    let mut bad = SLOOP;
    let out = find(&SLOOP, &[9, 5, 0x02, 0x05, 180, 0, 1, 0, 131]);
    bad[out + 8] = 0x84;
    assert!(parse_uac1(&bad).unwrap_err().contains("feedback"));
    let mut bad = SLOOP;
    let fb = find(&SLOOP, &[9, 5, 0x83, 0x11, 3, 0, 1, 4, 0]);
    bad[fb + 4] = 2; // 10.14 needs three bytes
    assert!(parse_uac1(&bad).unwrap_err().contains("feedback"));
}

#[test]
fn a_max_packet_too_small_for_44_1_khz_is_rejected() {
    let mut bad = SLOOP;
    let cap = find(&SLOOP, &[9, 5, 0x82, 0x05, 104, 1, 1, 0, 0]);
    bad[cap + 4] = 103; // 359 bytes: 44 frames of 4 x 16 bit, less than 45
    assert!(parse_uac1(&bad).unwrap_err().contains("wMaxPacketSize"));
}

#[test]
fn a_midi_only_configuration_has_no_audio_streams() {
    let mut midi = SLOOP[..109].to_vec();
    midi.splice(9..17, []); // without the IAD
    midi[2] = midi.len() as u8;
    midi[3] = 0;
    midi[4] = 2;
    let functions = parse_uac1(&midi).unwrap();
    assert!(functions.playback.is_none() && functions.capture.is_none());
}

#[test]
fn the_feedback_clock_sends_44_or_45_frames_a_millisecond() {
    let mut clock = FeedbackClock::default();
    let frames: Vec<u32> = (0..1000).map(|_| clock.frames(NOMINAL_44K1)).collect();
    assert_eq!(
        frames.iter().sum::<u32>(),
        44099,
        "44.1 kHz in 10.14 rounds down"
    );
    assert!(frames.iter().all(|n| (44..=45).contains(n)));
    let mut fast = FeedbackClock::default();
    let more: u32 = (0..1000).map(|_| fast.frames(NOMINAL_44K1 + 8192)).sum();
    assert_eq!(more, 44599, "+0.5 frame a millisecond");
}

#[test]
fn pcm_round_trips_at_16_and_24_bit() {
    let samples = [0, 1, -1, 32767, -32768, 1234];
    for width in [2usize, 3] {
        let wide: Vec<i32> = samples
            .iter()
            .map(|s| if width == 3 { s << 8 } else { *s })
            .collect();
        let bytes = encode_pcm(&wide, width);
        assert_eq!(bytes.len(), samples.len() * width);
        assert_eq!(decode_pcm(&bytes, width), wide);
    }
    assert_eq!(encode_pcm(&[-2], 3), vec![0xFE, 0xFF, 0xFF]);
}

#[test]
fn wav_dumps_have_a_riff_header_and_the_samples() {
    let wav = wav_bytes(44100, 4, 16, &[1, 2, 3, 4, -1, -2, -3, -4]);
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 4);
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44100);
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 16);
    assert_eq!(wav.len(), 44 + 16);
    assert_eq!(&wav[44..46], &[1, 0]);
    let wav24 = wav_bytes(44100, 1, 24, &[-2]);
    assert_eq!(&wav24[44..], &[0xFE, 0xFF, 0xFF]);
}
