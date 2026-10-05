// SPDX-License-Identifier: GPL-3.0-only
// USB-MIDI 1.0 event packets (class spec section 4) and raw MIDI streams.
use fm1_emu::usb_midi::{encode, Decoder, Parser};

fn decode_all(packets: &[[u8; 4]]) -> Vec<Vec<u8>> {
    let mut decoder = Decoder::default();
    packets.iter().filter_map(|p| decoder.push(*p)).collect()
}

#[test]
fn channel_messages_take_the_cin_of_their_status_nibble() {
    assert_eq!(encode(&[0x90, 60, 100], 0), vec![[0x09, 0x90, 60, 100]]);
    assert_eq!(encode(&[0x83, 60, 0], 0), vec![[0x08, 0x83, 60, 0]]);
    assert_eq!(encode(&[0xB0, 7, 99], 0), vec![[0x0B, 0xB0, 7, 99]]);
    assert_eq!(encode(&[0xC5, 12], 0), vec![[0x0C, 0xC5, 12, 0]]);
    assert_eq!(encode(&[0xD0, 40], 0), vec![[0x0D, 0xD0, 40, 0]]);
    assert_eq!(encode(&[0xE0, 0, 64], 1), vec![[0x1E, 0xE0, 0, 64]]);
}

#[test]
fn system_messages_use_the_common_and_single_byte_cins() {
    assert_eq!(encode(&[0xF8], 0), vec![[0x0F, 0xF8, 0, 0]]);
    assert_eq!(encode(&[0xF1, 5], 0), vec![[0x02, 0xF1, 5, 0]]);
    assert_eq!(encode(&[0xF2, 1, 2], 0), vec![[0x03, 0xF2, 1, 2]]);
    assert_eq!(encode(&[0xF6], 0), vec![[0x05, 0xF6, 0, 0]]);
}

#[test]
fn sysex_is_split_in_threes_and_ends_with_cin_5_6_or_7() {
    // 6 bytes: start + end with 3
    assert_eq!(
        encode(&[0xF0, 0x7D, 0x46, 0x4C, 0x01, 0xF7], 0),
        vec![[0x04, 0xF0, 0x7D, 0x46], [0x07, 0x4C, 0x01, 0xF7]]
    );
    // 4 bytes: end with 1
    assert_eq!(
        encode(&[0xF0, 1, 2, 0xF7], 0),
        vec![[0x04, 0xF0, 1, 2], [0x05, 0xF7, 0, 0]]
    );
    // 5 bytes: end with 2
    assert_eq!(
        encode(&[0xF0, 1, 2, 3, 0xF7], 0),
        vec![[0x04, 0xF0, 1, 2], [0x06, 3, 0xF7, 0]]
    );
    // the shortest: F0 F7
    assert_eq!(encode(&[0xF0, 0xF7], 0), vec![[0x06, 0xF0, 0xF7, 0]]);
    assert_eq!(encode(&[0xF0, 1, 0xF7], 0), vec![[0x07, 0xF0, 1, 0xF7]]);
}

#[test]
fn malformed_messages_encode_to_nothing() {
    assert!(encode(&[], 0).is_empty());
    assert!(encode(&[0x40, 1], 0).is_empty());
    assert!(encode(&[0x90, 60], 0).is_empty());
    assert!(encode(&[0xF0, 1, 2], 0).is_empty(), "unterminated SysEx");
    assert!(
        encode(&[0xF0, 0x80, 0xF7], 0).is_empty(),
        "SysEx data is 7 bit"
    );
}

#[test]
fn decoder_reassembles_long_sysex_across_packets_and_round_trips() {
    for n in 0..200usize {
        let mut message = vec![0xF0];
        message.extend((0..n).map(|i| (i * 7 % 128) as u8));
        message.push(0xF7);
        let packets = encode(&message, 0);
        assert_eq!(packets.len(), message.len().div_ceil(3));
        assert_eq!(decode_all(&packets), vec![message]);
    }
}

#[test]
fn decoder_passes_realtime_bytes_through_a_sysex_and_drops_padding_packets() {
    let mut packets = encode(&[0xF0, 1, 2, 3, 4, 5, 0xF7], 0);
    packets.insert(1, [0x0F, 0xF8, 0, 0]);
    packets.insert(0, [0, 0, 0, 0]);
    assert_eq!(
        decode_all(&packets),
        vec![vec![0xF8], vec![0xF0, 1, 2, 3, 4, 5, 0xF7]]
    );
}

#[test]
fn decoder_drops_a_sysex_cut_by_a_channel_message_and_a_lone_end() {
    let packets = [
        [0x04, 0xF0, 1, 2],
        [0x09, 0x90, 60, 1],
        [0x05, 0xF7, 0, 0],
        [0x04, 0xF0, 3, 4],
        [0x05, 0xF7, 0, 0],
    ];
    assert_eq!(
        decode_all(&packets),
        vec![vec![0x90, 60, 1], vec![0xF0, 3, 4, 0xF7]]
    );
}

#[test]
fn parser_splits_a_raw_stream_into_messages_with_running_status() {
    let mut parser = Parser::default();
    let stream = [
        0x90, 60, 100, 62, 90,   // running status
        0xF8, // realtime
        0xF0, 1, 0xF8, 2, 0xF7, // SysEx with realtime inside
        0xC0, 5, 6,    // program change, running status
        0x40, // still running status C0
        0xF6,
    ];
    let messages: Vec<Vec<u8>> = stream.iter().filter_map(|b| parser.push(*b)).collect();
    assert_eq!(
        messages,
        vec![
            vec![0x90, 60, 100],
            vec![0x90, 62, 90],
            vec![0xF8],
            vec![0xF8],
            vec![0xF0, 1, 2, 0xF7],
            vec![0xC0, 5],
            vec![0xC0, 6],
            vec![0xC0, 0x40],
            vec![0xF6],
        ]
    );
}

#[test]
fn parser_drops_data_without_status_and_an_interrupted_sysex() {
    let mut parser = Parser::default();
    let stream = [1, 2, 0xF0, 1, 2, 0x90, 60, 1, 0xF7, 3];
    let messages: Vec<Vec<u8>> = stream.iter().filter_map(|b| parser.push(*b)).collect();
    assert_eq!(messages, vec![vec![0x90, 60, 1]]);
}

#[test]
fn parser_caps_sysex_length() {
    let mut parser = Parser::default();
    assert!(parser.push(0xF0).is_none());
    for _ in 0..fm1_emu::usb_midi::MAX_SYSEX + 10 {
        assert!(parser.push(1).is_none());
    }
    assert!(parser.push(0xF7).is_none(), "an oversized SysEx is dropped");
    assert_eq!(parser.push(0xF8), Some(vec![0xF8]));
}
