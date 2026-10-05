// SPDX-License-Identifier: GPL-3.0-only
//! USB-MIDI 1.0 event packets (USB Device Class Definition for MIDI Devices
//! 1.0, section 4: byte 0 = cable << 4 | CIN, then up to 3 MIDI bytes) and
//! raw MIDI byte streams (what Web MIDI and the bridge's WebSocket carry).

/// One USB-MIDI event packet as it sits in an endpoint buffer.
pub type Packet = [u8; 4];

/// Longest SysEx the decoder and parser assemble; longer ones are dropped.
/// The firmwares' longest frames are about 1 KiB (editor SMP_END).
pub const MAX_SYSEX: usize = 64 * 1024;

/// Bytes a channel or system-common message has, status included.
fn length_of(status: u8) -> Option<usize> {
    match status {
        0x80..=0xBF | 0xE0..=0xEF | 0xF2 => Some(3),
        0xC0..=0xDF | 0xF1 | 0xF3 => Some(2),
        0xF6 | 0xF8..=0xFF => Some(1),
        _ => None,
    }
}

fn packet(cable: u8, cin: u8, bytes: &[u8]) -> Packet {
    let mut p = [cable << 4 | cin, 0, 0, 0];
    p[1..1 + bytes.len()].copy_from_slice(bytes);
    p
}

/// Packets for one complete MIDI message (channel, system common,
/// realtime or a whole F0..F7 SysEx). Malformed input gives no packets.
pub fn encode(message: &[u8], cable: u8) -> Vec<Packet> {
    let cable = cable & 15;
    let Some(&status) = message.first() else {
        return Vec::new();
    };
    if status == 0xF0 {
        return encode_sysex(message, cable);
    }
    if length_of(status) != Some(message.len()) || message[1..].iter().any(|b| b & 0x80 != 0) {
        return Vec::new();
    }
    let cin = match status {
        0x80..=0xEF => status >> 4,
        0xF2 => 3,
        0xF1 | 0xF3 => 2,
        0xF6 => 5,
        _ => 0xF,
    };
    vec![packet(cable, cin, message)]
}

fn encode_sysex(message: &[u8], cable: u8) -> Vec<Packet> {
    let body = &message[1..];
    let well_formed = body.last() == Some(&0xF7)
        && body[..body.len() - 1].iter().all(|b| b & 0x80 == 0);
    if !well_formed {
        return Vec::new();
    }
    message
        .chunks(3)
        .enumerate()
        .map(|(i, chunk)| {
            let last = (i + 1) * 3 >= message.len();
            let cin = if last { 4 + chunk.len() as u8 } else { 4 };
            packet(cable, cin, chunk)
        })
        .collect()
}

/// Packets back to complete messages (one per call at most: a packet holds
/// at most one message end). Realtime bytes inside a SysEx come out at once.
#[derive(Default)]
pub struct Decoder {
    sysex: Option<Vec<u8>>,
}

impl Decoder {
    pub fn push(&mut self, p: Packet) -> Option<Vec<u8>> {
        let (cin, data) = (p[0] & 15, &p[1..]);
        match cin {
            4 => {
                self.sysex_bytes(&data[..3]);
                None
            }
            // CIN 5 is a SysEx end with one byte (F7) or a one-byte system common (F6)
            5 if data[0] != 0xF7 => self.single(&data[..1]),
            5..=7 => {
                self.sysex_bytes(&data[..(cin - 4) as usize]);
                let done = self.sysex.take()?;
                (done.last() == Some(&0xF7)).then_some(done)
            }
            0xF => {
                if data[0] >= 0xF8 {
                    return Some(vec![data[0]]);
                }
                self.single(&data[..1])
            }
            2 => self.single(&data[..2]),
            3 => self.single(&data[..3]),
            8..=0xE => {
                let n = length_of(data[0]).filter(|_| data[0] >> 4 == cin)?;
                self.sysex = None;
                Some(data[..n].to_vec())
            }
            _ => None,
        }
    }

    fn single(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        (length_of(bytes[0]) == Some(bytes.len())).then(|| {
            if bytes[0] < 0xF8 {
                self.sysex = None;
            }
            bytes.to_vec()
        })
    }

    fn sysex_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == 0xF0 {
                self.sysex = Some(vec![b]);
                continue;
            }
            let Some(buffer) = &mut self.sysex else {
                return;
            };
            if buffer.len() >= MAX_SYSEX {
                self.sysex = None;
                return;
            }
            buffer.push(b);
            if b == 0xF7 {
                return;
            }
        }
    }
}

/// A raw MIDI byte stream to complete messages, with running status.
#[derive(Default)]
pub struct Parser {
    status: Option<u8>,
    message: Vec<u8>,
    in_sysex: bool,
    oversized: bool,
}

impl Parser {
    pub fn push(&mut self, b: u8) -> Option<Vec<u8>> {
        if b >= 0xF8 {
            return Some(vec![b]);
        }
        if b & 0x80 != 0 {
            return self.status_byte(b);
        }
        if self.in_sysex {
            if self.message.len() >= MAX_SYSEX {
                self.oversized = true;
            } else {
                self.message.push(b);
            }
            return None;
        }
        let status = self.status?;
        if self.message.is_empty() {
            self.message.push(status);
        }
        self.message.push(b);
        if Some(self.message.len()) != length_of(status) {
            return None;
        }
        if status >= 0xF0 {
            self.status = None; // system common: no running status
        }
        Some(std::mem::take(&mut self.message))
    }

    fn status_byte(&mut self, b: u8) -> Option<Vec<u8>> {
        let sysex = std::mem::replace(&mut self.in_sysex, false);
        let oversized = std::mem::replace(&mut self.oversized, false);
        let mut pending = std::mem::take(&mut self.message);
        self.status = None;
        match b {
            0xF0 => {
                self.in_sysex = true;
                self.message.push(b);
                None
            }
            0xF7 if sysex && !oversized => {
                pending.push(b);
                Some(pending)
            }
            0xF6 => Some(vec![b]),
            _ => {
                if length_of(b).is_some() {
                    self.status = Some(b);
                }
                None
            }
        }
    }
}
