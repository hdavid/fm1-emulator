// SPDX-License-Identifier: GPL-3.0-only
// USB0 register bridge and a small USB host for the CDC console and, when
// enabled, USB-MIDI.
use crate::{usb_midi::Packet, RAM};
use std::collections::VecDeque;

/// Packets kept from the device when nobody drains them (oldest dropped).
const MIDI_RECEIVED_MAX: usize = 4096;
#[derive(Default)]
pub struct Usb {
    regs: [u32; 16],
    io: u32,
    clock: u32,
    sie: [u8; 16],
    endpoints: [[u8; 8]; 4],
    index: usize,
    ticks: u64,
    deadline: u64,
    attached: bool,
    phase: usize,
    waiting: bool,
    response: Vec<u8>,
    cdc_interface: Option<u8>,
    cdc_endpoint: Option<usize>,
    cdc_out: Option<(usize, usize)>,
    input: VecDeque<u8>,
    pub serial: VecDeque<u8>,
    pub setups: u64,
    pub packets: u64,
    midi: MidiHost,
    /// USB-MIDI packets the device sent on its MIDI IN endpoint.
    pub midi_received: VecDeque<Packet>,
}
/// The optional MIDI side of the host model (off unless enabled, so the CDC
/// enumeration every existing check was measured with stays byte for byte).
#[derive(Default)]
struct MidiHost {
    enabled: bool,
    /// Device endpoints: bulk OUT (host to device) and bulk IN.
    out_ep: Option<usize>,
    in_ep: Option<usize>,
    product_index: u8,
    product: Option<String>,
    to_device: VecDeque<Packet>,
}
/// A string descriptor (UTF-16LE after the 2-byte header) as text.
fn string_descriptor(d: &[u8]) -> Option<String> {
    let n = (*d.first()? as usize).min(d.len());
    if n < 2 || d[1] != 3 {
        return None;
    }
    let units: Vec<u16> = d[2..n]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> (Usb, Vec<u8>) {
        let mut usb = Usb::default();
        usb.io = 0x40;
        usb.regs[0] = 4;
        usb.attached = true;
        usb.phase = 2;
        // MIDI OUT 1 must be ignored. CDC OUT 2 and IN 3 are deliberately
        // distinct so neither direction can be guessed from the other.
        usb.response = vec![
            9, 4, 1, 0, 1, 1, 3, 0, 0, 7, 5, 1, 2, 64, 0, 0, 9, 4, 2, 0, 0, 2, 2, 1, 0, 9, 4, 3, 0,
            2, 10, 0, 0, 0, 7, 5, 2, 2, 32, 0, 0, 7, 5, 0x83, 2, 64, 0, 0,
        ];
        usb.complete();
        usb.phase = 5;
        usb.regs[10] = RAM + 64; // EP2 RX DMA.
        usb.endpoints[2][3] = 0xff;
        (usb, vec![0; 256])
    }
    fn sie_write(usb: &mut Usb, ram: &mut [u8], register: u32, value: u32) {
        usb.write(0x11804, register << 8 | value, ram)
            .unwrap()
            .unwrap();
    }
    fn sie_read(usb: &mut Usb, ram: &mut [u8], register: u32) -> u32 {
        usb.write(0x11804, register << 8 | 0x4000, ram)
            .unwrap()
            .unwrap();
        usb.read(0x11804).unwrap() & 255
    }

    #[test]
    fn a_configuration_without_a_cdc_console_skips_the_line_state() {
        // A firmware whose USB configuration has no CDC interface (e.g. USB
        // audio in its place) enumerates; only the line-state request goes.
        let usb = Usb::default();
        assert_eq!(usb.cdc_interface, None);
        assert_eq!(usb.setup(4), Ok(None));
        assert!(usb.setup(3).unwrap().is_some());
    }

    #[test]
    fn cdc_out_discovers_its_endpoint_and_preserves_packets_until_guest_ack() {
        let (mut usb, mut ram) = configured();
        let bytes: Vec<_> = (0..70).collect();
        assert!(usb.receive_serial(&bytes));
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[64..96], &bytes[..32]);
        assert_eq!(sie_read(&mut usb, &mut ram, 4), 1 << 2);
        assert_eq!(sie_read(&mut usb, &mut ram, 4), 0); // IRQ read acknowledges latch.
        sie_write(&mut usb, &mut ram, 14, 2);
        assert_eq!(sie_read(&mut usb, &mut ram, 22), 32);
        assert_eq!(sie_read(&mut usb, &mut ram, 20) & 1, 1);
        usb.advance(24_000, &mut ram).unwrap();
        assert_eq!(&ram[64..96], &bytes[..32]); // NAK: unread packet stays intact.
        assert_eq!(usb.input.len(), 38);
        sie_write(&mut usb, &mut ram, 20, 0x11); // Guest RX FIFO flush/ack.
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[64..96], &bytes[32..64]);
        assert_eq!(sie_read(&mut usb, &mut ram, 4), 1 << 2);
        sie_write(&mut usb, &mut ram, 20, 0); // Clear RXPKTRDY without flush.
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[64..70], &bytes[64..]);
        assert_eq!(sie_read(&mut usb, &mut ram, 22), 6);
        assert!(usb.input.is_empty());
    }

    #[test]
    fn cdc_out_waits_for_configuration_enable_and_valid_dma() {
        let (mut usb, mut ram) = configured();
        assert!(usb.receive_serial(b"help\n"));
        usb.phase = 4;
        usb.waiting = true;
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[64..69], &[0; 5]);
        usb.phase = 5;
        usb.regs[0] |= 1 << 21; // EP2 disabled.
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(usb.input.len(), 5);
        usb.regs[0] &= !(1 << 21);
        usb.regs[10] = RAM + 255;
        assert_eq!(usb.advance(1, &mut ram), Err("USB DMA exceeds SRAM"));
        assert_eq!(usb.input.len(), 5); // Fault must not lose host bytes.
        usb.regs[10] = RAM + 64;
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[64..69], b"help\n");
    }

    #[test]
    fn midi_host_finds_the_streaming_endpoints_and_moves_packets_both_ways() {
        let mut usb = Usb::default();
        usb.enable_midi_host();
        usb.io = 0x40;
        usb.regs[0] = 4;
        usb.attached = true;
        usb.phase = 2;
        // The configuration of the CDC test above: MIDI streaming OUT 1 and,
        // here, a MIDI IN 0x81 next to the CDC OUT 2 / IN 3.
        usb.response = vec![
            9, 4, 1, 0, 2, 1, 3, 0, 0, 7, 5, 1, 2, 64, 0, 0, 7, 5, 0x81, 2, 64, 0, 0, 9, 4, 2, 0,
            0, 2, 2, 1, 0, 9, 4, 3, 0, 2, 10, 0, 0, 0, 7, 5, 2, 2, 32, 0, 0, 7, 5, 0x83, 2, 64, 0,
            0,
        ];
        usb.complete();
        assert_eq!((usb.midi.out_ep, usb.midi.in_ep), (Some(1), Some(1)));
        assert_eq!(usb.cdc_out, Some((2, 32)));
        assert!(
            !usb.midi_ready(),
            "the product string request is still to come"
        );
        usb.phase = 6;
        assert!(usb.midi_ready());
        let mut ram = vec![0; 256];
        usb.regs[8] = RAM + 128; // EP1 RX DMA.
        usb.endpoints[1][3] = 0xff;
        let packets: Vec<_> = (0..20u8).map(|i| [0x09, 0x90, i, 100]).collect();
        usb.midi_send(&packets);
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[128..132], &[0x09, 0x90, 0, 100]);
        assert_eq!((usb.endpoints[1][4] & 1, usb.endpoints[1][6]), (1, 64));
        assert_eq!(usb.sie[4], 1 << 1);
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(usb.midi_pending(), 4, "a held packet waits for the guest");
        usb.endpoints[1][4] = 0; // Guest clears RxPktRdy.
        usb.advance(1, &mut ram).unwrap();
        assert_eq!(&ram[128..132], &[0x09, 0x90, 16, 100]);
        assert_eq!((usb.midi_pending(), usb.endpoints[1][6]), (0, 16));
        // Device to host: the guest arms its MIDI IN endpoint.
        usb.regs[7] = RAM + 192; // EP1 TX DMA.
        usb.regs[3] = 8;
        ram[192..200].copy_from_slice(&[0x0b, 0xb0, 7, 64, 0x0f, 0xf8, 0, 0]);
        usb.index = 1;
        usb.write_inner(0x11804, 17 << 8 | 1, &mut ram).unwrap();
        assert_eq!(
            usb.midi_received.drain(..).collect::<Vec<_>>(),
            [[0x0b, 0xb0, 7, 64], [0x0f, 0xf8, 0, 0]]
        );
    }

    #[test]
    fn a_device_without_the_console_skips_the_line_state_for_the_midi_host() {
        let mut usb = Usb::default();
        usb.enable_midi_host();
        assert_eq!(usb.setup(4), Ok(None));
        assert_eq!(usb.setup(5), Ok(None), "no product string index");
        usb.midi.product_index = 2;
        assert_eq!(usb.setup(5), Ok(Some([0x80, 6, 2, 3, 9, 4, 255, 0])));
        assert_eq!(
            string_descriptor(&[
                16, 3, b'F', 0, b'e', 0, b'l', 0, b'u', 0, b'c', 0, b'c', 0, b'a', 0
            ]),
            Some("Felucca".into())
        );
    }

    #[test]
    fn host_input_is_bounded_and_failed_enqueue_keeps_existing_bytes() {
        let (mut usb, mut ram) = configured();
        assert!(usb.receive_serial(&vec![42; 4096]));
        assert!(!usb.receive_serial(b"discard nothing"));
        assert_eq!(usb.input.len(), 4096);
        usb.advance(1, &mut ram).unwrap();
        assert!(usb.receive_serial(&[7; 32]));
        assert!(!usb.receive_serial(b"x"));
        assert_eq!(usb.input.back(), Some(&7));
    }
}
impl Usb {
    /// Queue terminal bytes for the guest's CDC bulk OUT endpoint. Backpressure
    /// keeps host input bounded while the guest is paused or its RX FIFO is full.
    pub fn receive_serial(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > 4096 - self.input.len() {
            return false;
        }
        self.input.extend(bytes.iter().copied());
        true
    }
    pub fn read(&self, a: u32) -> Option<u32> {
        match a {
            0x51000 => Some(self.io),
            // FM-1_997: CON1=0 at SPL handoff (CON0=0xe0c), then
            // CON1=2 with the full-speed D+ pull-up/input enabled (0x164c).
            // Model the idle wire level; individual bus symbols are absent.
            0x51004 => Some(if self.io & 0x1040 == 0x1040 { 2 } else { 0 }),
            0x10010 => Some(self.clock),
            // SDK H0_SIE_CON: stock startup disables the unused high-speed port.
            0x16800 => Some(0),
            0x11800..=0x1183c if a.is_multiple_of(4) => {
                Some(self.regs[((a - 0x11800) / 4) as usize])
            }
            _ => None,
        }
    }
    fn dma(ram: &mut [u8], a: u32, n: usize) -> Result<&mut [u8], &'static str> {
        let start = a.checked_sub(RAM).ok_or("USB DMA must address SRAM")? as usize;
        ram.get_mut(start..start + n).ok_or("USB DMA exceeds SRAM")
    }
    fn complete(&mut self) {
        match self.phase {
            0 if self.response.len() >= 18 => self.midi.product_index = self.response[15],
            2 => self.parse_configuration(),
            5 => self.midi.product = string_descriptor(&self.response),
            _ => {}
        }
        self.phase += 1;
        self.waiting = false;
        self.deadline = self.ticks + 24000;
    }
    /// The CDC data endpoints and the MIDI streaming endpoints (interface
    /// class 1 subclass 3, bulk) from the configuration descriptor.
    fn parse_configuration(&mut self) {
        let mut i = 0;
        let (mut cdc_data, mut midi_streaming) = (false, false);
        while i + 2 <= self.response.len() {
            let n = self.response[i] as usize;
            if n < 2 || i + n > self.response.len() {
                break;
            }
            let d = &self.response[i..i + n];
            if d[1] == 4 && n >= 9 {
                if d[5] == 2 {
                    self.cdc_interface = Some(d[2]);
                }
                cdc_data = d[5] == 10;
                midi_streaming = d[5] == 1 && d[6] == 3;
            }
            if d[1] == 5 && n >= 7 && d[3] & 3 == 2 {
                let (endpoint, input) = ((d[2] & 15) as usize, d[2] & 128 != 0);
                if cdc_data && input {
                    self.cdc_endpoint = Some(endpoint);
                } else if cdc_data {
                    self.cdc_out = Some((endpoint, u16::from_le_bytes([d[4], d[5]]) as usize));
                }
                if midi_streaming && (1..=3).contains(&endpoint) {
                    *if input {
                        &mut self.midi.in_ep
                    } else {
                        &mut self.midi.out_ep
                    } = Some(endpoint);
                }
            }
            i += n;
        }
    }
    /// Requests the host makes: 5 without a MIDI host (up to the CDC line
    /// state), 6 with one (then the product string).
    fn last_phase(&self) -> usize {
        if self.midi.enabled {
            6
        } else {
            5
        }
    }
    /// The SETUP packet of `phase`, or None for a request this device does
    /// not need (no CDC console: no line state; no product string to read).
    fn setup(&self, phase: usize) -> Result<Option<[u8; 8]>, &'static str> {
        Ok(Some(match phase {
            0 => [0x80, 6, 0, 1, 0, 0, 18, 0],
            1 => [0, 5, 1, 0, 0, 0, 0, 0],
            2 => [0x80, 6, 0, 2, 0, 0, 255, 0],
            3 => [0, 9, 1, 0, 0, 0, 0, 0],
            4 => match self.cdc_interface {
                Some(interface) => [0x21, 0x22, 1, 0, interface, 0, 0, 0],
                // No console in this configuration (e.g. USB audio in its
                // place): there is no line state to set.
                None => return Ok(None),
            },
            _ if self.midi.product_index == 0 => return Ok(None),
            _ => [0x80, 6, self.midi.product_index, 3, 0x09, 0x04, 255, 0],
        }))
    }
    /// Turn the host into a USB-MIDI host as well: after the CDC requests it
    /// reads the product string and moves packets on the MIDI endpoints.
    pub fn enable_midi_host(&mut self) {
        self.midi.enabled = true;
    }
    /// Enumerated, configured, and the device has MIDI endpoints.
    pub fn midi_ready(&self) -> bool {
        self.midi.enabled && self.phase >= self.last_phase() && self.midi.out_ep.is_some()
    }
    /// The device's product string (iProduct), once the MIDI host read it.
    pub fn product(&self) -> Option<&str> {
        self.midi.product.as_deref()
    }
    /// Queue packets for the device's MIDI OUT endpoint (host to device).
    pub fn midi_send(&mut self, packets: &[Packet]) {
        self.midi.to_device.extend(packets.iter().copied());
    }
    /// Packets queued for the device and not yet delivered.
    pub fn midi_pending(&self) -> usize {
        self.midi.to_device.len()
    }
    /// One bulk OUT packet (at most 64 bytes = 16 events) into the
    /// endpoint's RX buffer as the controller's DMA does, then RxPktRdy, the
    /// count and the endpoint's RX interrupt flag. A held packet, a disabled
    /// or stalled endpoint waits (NAK), as for the CDC console.
    fn deliver_midi(&mut self, ram: &mut [u8]) -> Result<(), &'static str> {
        let Some(ep) = self.midi.out_ep else {
            return Ok(());
        };
        if self.regs[0] & (1 << (19 + ep)) != 0
            || self.endpoints[ep][3] == 0
            || self.endpoints[ep][4] & 0x21 != 0
        {
            return Ok(());
        }
        let n = self.midi.to_device.len().min(16);
        let buffer = Self::dma(ram, self.regs[8 + (ep - 1) * 2], n * 4)?;
        for (slot, packet) in buffer
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.midi.to_device.drain(..n))
        {
            slot.copy_from_slice(&packet);
        }
        self.endpoints[ep][4] |= 1;
        self.endpoints[ep][6] = (n * 4) as u8;
        self.endpoints[ep][7] = 0;
        self.sie[4] |= 1 << ep;
        Ok(())
    }
    fn send(&mut self, ep: usize, ram: &mut [u8]) -> Result<(), &'static str> {
        let n = self.regs[2 + ep] as usize;
        if n > 64 {
            return Err("USB full-speed packet exceeds 64 bytes");
        }
        let a = if ep == 0 {
            self.regs[6]
        } else {
            self.regs[7 + (ep - 1) * 2]
        };
        let bytes = Self::dma(ram, a, n)?;
        if ep == 0 {
            self.response.extend_from_slice(bytes);
        } else if Some(ep) == self.cdc_endpoint {
            self.serial.extend(bytes.iter().copied());
            self.packets += 1;
        } else if self.midi.enabled && Some(ep) == self.midi.in_ep {
            for chunk in bytes.as_chunks::<4>().0 {
                if self.midi_received.len() == MIDI_RECEIVED_MAX {
                    self.midi_received.pop_front();
                }
                self.midi_received.push_back(*chunk);
            }
        }
        self.sie[2] |= 1 << ep;
        Ok(())
    }
    pub fn write(&mut self, a: u32, v: u32, ram: &mut [u8]) -> Option<Result<(), &'static str>> {
        self.read(a)?;
        Some(self.write_inner(a, v, ram))
    }
    fn write_inner(&mut self, a: u32, v: u32, ram: &mut [u8]) -> Result<(), &'static str> {
        match a {
            0x16800 => {
                if v != 0 {
                    return Err("high-speed USB controller is not implemented");
                }
            }
            0x51000 => self.io = v,
            0x51004 => return Err("USB I/O input status is read-only in the supported model"),
            0x10010 => self.clock = v,
            0x11800 => {
                self.regs[0] = v & !0x1000;
                if v & 0x1000 != 0 {
                    self.regs[0] &= !0x2000;
                }
                if v & 4 == 0 {
                    self.attached = false;
                    self.phase = 0;
                    self.waiting = false;
                    self.sie = [0; 16];
                    self.endpoints = [[0; 8]; 4];
                    self.cdc_interface = None;
                    self.cdc_endpoint = None;
                    self.cdc_out = None;
                }
            }
            0x11804 => {
                if self.regs[0] & 4 == 0 {
                    self.regs[1] = v;
                    return Ok(());
                }
                let r = ((v >> 8) & 0x3f) as usize;
                if r > 23 {
                    return Err("unimplemented USB SIE register");
                }
                let data = if v & 0x4000 != 0 {
                    let data = if r < 16 {
                        self.sie[r]
                    } else {
                        self.endpoints[self.index][r - 16]
                    };
                    if (2..=6).contains(&r) {
                        self.sie[r] = 0;
                    }
                    data
                } else {
                    let data = v as u8;
                    if r == 14 {
                        if data > 3 {
                            return Err("USB endpoint index exceeds modeled controller");
                        }
                        self.index = data as usize;
                        self.sie[r] = data;
                    } else if r < 16 {
                        self.sie[r] = data;
                    } else if r == 17 {
                        if self.index == 0 {
                            if data & 0x20 != 0 {
                                if self.waiting && self.phase == 5 && self.midi.enabled {
                                    // No product string: an optional request.
                                    self.complete();
                                    return Ok(());
                                }
                                return Err("guest stalled host USB control request");
                            }
                            if data & 0x40 != 0 {
                                self.endpoints[0][1] &= !1;
                            }
                            if data & 2 != 0 {
                                self.send(0, ram)?;
                            }
                            if data & 8 != 0 && self.waiting {
                                self.complete();
                            }
                        } else {
                            self.endpoints[self.index][1] = data & !0xc9;
                            if data & 1 != 0 {
                                self.send(self.index, ram)?;
                            }
                        }
                    } else if r == 20 && self.index != 0 {
                        // RXCSR: flush FIFO and clear data toggle are commands.
                        // A held RXPKTRDY packet must survive polling until the
                        // guest clears it or explicitly flushes the FIFO.
                        self.endpoints[self.index][4] = data & !0x90;
                        if data & 0x10 != 0 || data & 1 == 0 {
                            self.endpoints[self.index][4] &= !1;
                            self.endpoints[self.index][6] = 0;
                            self.endpoints[self.index][7] = 0;
                        }
                    } else {
                        self.endpoints[self.index][r - 16] = data;
                    }
                    data
                };
                self.regs[1] = 0x8000 | data as u32;
            }
            _ => self.regs[((a - 0x11800) / 4) as usize] = v,
        }
        Ok(())
    }
    pub fn advance(&mut self, ticks: u32, ram: &mut [u8]) -> Result<(), &'static str> {
        self.ticks += ticks as u64;
        if self.regs[0] & 4 == 0 || self.io & 0x40 == 0 {
            return Ok(());
        }
        let frame = (self.ticks / 24000) as u16 & 2047;
        self.sie[12] = frame as u8;
        self.sie[13] = (frame >> 8) as u8;
        if self.ticks % 24000 < ticks as u64 {
            self.regs[0] |= 0x2000;
        }
        if !self.attached {
            self.attached = true;
            self.sie[6] |= 4;
            self.deadline = self.ticks + 120000;
        }
        if self.phase >= 5 && !self.input.is_empty() {
            self.receive(ram)?;
        }
        if self.midi_ready() && !self.midi.to_device.is_empty() {
            self.deliver_midi(ram)?;
        }
        if self.waiting || self.ticks < self.deadline || self.phase >= self.last_phase() {
            return Ok(());
        }
        let setup = loop {
            match self.setup(self.phase)? {
                Some(setup) => break setup,
                None => self.phase += 1,
            }
            if self.phase >= self.last_phase() {
                return Ok(());
            }
        };
        Self::dma(ram, self.regs[6], 8)?.copy_from_slice(&setup);
        self.response.clear();
        self.endpoints[0][1] = 1;
        self.endpoints[0][6] = 8;
        self.sie[2] |= 1;
        self.waiting = true;
        self.setups += 1;
        Ok(())
    }

    fn receive(&mut self, ram: &mut [u8]) -> Result<(), &'static str> {
        let Some((ep, packet_size)) = self.cdc_out else {
            return Ok(());
        };
        if !(1..=3).contains(&ep) || !(1..=64).contains(&packet_size) {
            return Err("CDC OUT endpoint exceeds modeled full-speed controller");
        }
        if self.regs[0] & (1 << (19 + ep)) != 0
            || self.endpoints[ep][3] == 0
            || self.endpoints[ep][4] & 0x21 != 0
        {
            return Ok(()); // Endpoint disabled, stalled, or still holding a packet (NAK).
        }
        let n = self.input.len().min(packet_size);
        let destination = Self::dma(ram, self.regs[8 + (ep - 1) * 2], n)?;
        for (byte, &input) in destination.iter_mut().zip(self.input.iter()) {
            *byte = input;
        }
        self.input.drain(..n);
        self.endpoints[ep][4] |= 1;
        self.endpoints[ep][6] = n as u8;
        self.endpoints[ep][7] = 0;
        self.sie[4] |= 1 << ep;
        Ok(())
    }
}
