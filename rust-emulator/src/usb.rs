// SPDX-License-Identifier: GPL-3.0-only
// USB0 register bridge and a small USB host for the CDC console and, when
// enabled, USB-MIDI (the web editor bridge) and USB audio (UAC1 streams).
use crate::{usb_audio_host::AudioHost, usb_midi::Packet, RAM};
use std::collections::VecDeque;

/// Packets kept from the device when nobody drains them (oldest dropped).
const MIDI_RECEIVED_MAX: usize = 4096;
#[derive(Default)]
pub struct Usb {
    regs: [u32; 16],
    io: u32,
    /// JL_USB_IO CON1/CON2 (WL82.h psfr 0x1000). STUB: undocumented; the
    /// stock battery/charge code (0x02025872) reads CON1 bit 1. Reset value
    /// unmeasured, 0 assumed; writes are kept.
    io_more: [u32; 2],
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
    pub serial: VecDeque<u8>,
    pub setups: u64,
    pub packets: u64,
    midi: MidiHost,
    /// USB-MIDI packets the device sent on its MIDI IN endpoint.
    pub midi_received: VecDeque<Packet>,
    audio: AudioHost,
    /// SRAM ranges the host wrote since the last take (code cache).
    dma_writes: Vec<(u32, usize)>,
}
/// The optional MIDI side of the host model (off unless enabled, so the CDC
/// enumeration every baseline was measured with stays byte for byte).
#[derive(Default)]
struct MidiHost {
    enabled: bool,
    /// Device endpoints: bulk OUT (host to device) and bulk IN.
    out_ep: Option<usize>,
    in_ep: Option<usize>,
    product_index: u8,
    product: Option<String>,
    to_device: VecDeque<Packet>,
    /// Bytes of the last OUT packet written into SRAM (address, length).
    last_rx: (u32, usize),
    rx_packets: u64,
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
impl Usb {
    pub fn read(&self, a: u32) -> Option<u32> {
        match a {
            0x51000 => Some(self.io),
            0x51004 | 0x51008 => Some(self.io_more[((a - 0x51004) / 4) as usize]),
            0x10010 => Some(self.clock),
            0x11800..=0x1183c if a.is_multiple_of(4) => {
                Some(self.regs[((a - 0x11800) / 4) as usize])
            }
            _ => None,
        }
    }
    /// Where the host model writes SETUP packets (the endpoint 0 buffer).
    pub(crate) fn setup_address(&self) -> u32 {
        self.regs[6]
    }
    fn dma(ram: &mut [u8], a: u32, n: usize) -> Result<&mut [u8], &'static str> {
        let start = a.checked_sub(RAM).ok_or("USB DMA must address SRAM")? as usize;
        ram.get_mut(start..start + n).ok_or("USB DMA exceeds SRAM")
    }
    fn complete(&mut self) {
        match self.phase {
            0 if self.response.len() >= 18 => self.midi.product_index = self.response[15],
            2 => {
                self.parse_configuration();
                if self.audio.enabled() {
                    self.audio.configure(&self.response);
                }
            }
            5 => self.midi.product = string_descriptor(&self.response),
            p if p >= self.base_phases() => {
                self.audio.completed(p - self.base_phases(), &self.response)
            }
            _ => {}
        }
        self.phase += 1;
        self.waiting = false;
        self.deadline = self.ticks + 24000;
    }
    /// The CDC data IN endpoint and the MIDI streaming endpoints (interface
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
                let (ep, input) = ((d[2] & 15) as usize, d[2] & 128 != 0);
                if cdc_data && input {
                    self.cdc_endpoint = Some(ep);
                }
                if midi_streaming && (1..=3).contains(&ep) {
                    *if input {
                        &mut self.midi.in_ep
                    } else {
                        &mut self.midi.out_ep
                    } = Some(ep);
                }
            }
            i += n;
        }
    }
    /// The last host request: 5 (CDC line state) without a MIDI host, then
    /// 6 (the product string) with one.
    fn base_phases(&self) -> usize {
        if self.midi.enabled {
            6
        } else {
            5
        }
    }
    /// ...then the audio host's requests (stream alternates, rates).
    fn last_phase(&self) -> usize {
        self.base_phases() + self.audio.requests()
    }
    /// The SETUP packet of `phase`, or None for a request this device does
    /// not need (only skipped by the MIDI host: no CDC, or no product string).
    fn setup(&self, phase: usize) -> Result<Option<[u8; 8]>, &'static str> {
        Ok(Some(match phase {
            0 => [0x80, 6, 0, 1, 0, 0, 18, 0],
            1 => [0, 5, 1, 0, 0, 0, 0, 0],
            // The audio host needs the whole configuration (411 bytes on the
            // FM-1 audio builds); 255 stays for the baselines without it.
            2 if self.audio.enabled() => [0x80, 6, 0, 2, 0, 0, 0, 4],
            2 => [0x80, 6, 0, 2, 0, 0, 255, 0],
            3 => [0, 9, 1, 0, 0, 0, 0, 0],
            4 => match self.cdc_interface {
                Some(interface) => [0x21, 0x22, 1, 0, interface, 0, 0, 0],
                // A device without the console (a USB audio build): no line state.
                None => return Ok(None),
            },
            p if p >= self.base_phases() => return Ok(self.audio.setup(p - self.base_phases())),
            _ if self.midi.product_index == 0 => return Ok(None),
            _ => [0x80, 6, self.midi.product_index, 3, 0x09, 0x04, 255, 0],
        }))
    }
    /// Turn the host into a USB-MIDI host as well: after the CDC requests it
    /// reads the product string and moves packets on the MIDI endpoints.
    pub fn enable_midi_host(&mut self) {
        self.midi.enabled = true;
    }
    /// Turn the host into a USB audio host as well (and a MIDI host): after
    /// enumeration it selects alternate `alt` (1 = 16 bit, 2 = 24 bit on the
    /// FM-1 firmwares) of every audio stream, sets and reads the sampling
    /// rate, then moves isochronous packets every 1 ms frame.
    pub fn enable_audio_host(&mut self, alt: u8) {
        self.enable_midi_host();
        self.audio.enable(alt);
    }
    pub fn audio(&self) -> &AudioHost {
        &self.audio
    }
    pub fn audio_mut(&mut self) -> &mut AudioHost {
        &mut self.audio
    }
    /// SRAM ranges the host model wrote (control and audio OUT data).
    pub(crate) fn take_dma_writes(&mut self) -> Vec<(u32, usize)> {
        std::mem::take(&mut self.dma_writes)
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
    pub(crate) fn midi_send(&mut self, packets: &[Packet]) {
        self.midi.to_device.extend(packets.iter().copied());
    }
    /// Packets queued for the device and not yet delivered.
    pub fn midi_pending(&self) -> usize {
        self.midi.to_device.len()
    }
    /// OUT packets delivered to the device so far.
    pub fn midi_rx_packets(&self) -> u64 {
        self.midi.rx_packets
    }
    /// SRAM bytes the last OUT delivery wrote (address, length).
    pub(crate) fn midi_last_rx(&self) -> (u32, usize) {
        self.midi.last_rx
    }
    /// Whether the next tick delivers an OUT packet: there is one, the
    /// device is configured and its endpoint buffer is free (RxPktRdy clear).
    fn midi_deliverable(&self) -> bool {
        self.midi_ready()
            && !self.midi.to_device.is_empty()
            && self
                .midi
                .out_ep
                .is_some_and(|ep| self.endpoints[ep][4] & 1 == 0)
    }
    /// One bulk OUT packet (at most 64 bytes = 16 events) into the
    /// endpoint's RX buffer, as the controller's DMA does, then RxPktRdy,
    /// the count and the endpoint's RX interrupt flag.
    fn deliver_midi(&mut self, ram: &mut [u8]) -> Result<(), &'static str> {
        let ep = self.midi.out_ep.ok_or("USB MIDI has no OUT endpoint")?;
        let n = self.midi.to_device.len().min(16);
        let address = self.regs[8 + (ep - 1) * 2];
        let buffer = Self::dma(ram, address, n * 4)?;
        for (slot, packet) in buffer
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.midi.to_device.drain(..n))
        {
            slot.copy_from_slice(&packet);
        }
        let e = &mut self.endpoints[ep];
        e[4] |= 1;
        e[6] = (n * 4) as u8;
        e[7] = 0;
        self.sie[4] |= 1 << ep;
        self.midi.last_rx = (address, n * 4);
        self.midi.rx_packets += 1;
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
            0x51000 => self.io = v,
            0x51004 | 0x51008 => self.io_more[((a - 0x51004) / 4) as usize] = v,
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
                                if data & 8 == 0 && self.waiting {
                                    self.audio.data_stage_wanted();
                                }
                            }
                            if data & 2 != 0 {
                                self.send(0, ram)?;
                            }
                            if data & 8 != 0 && self.waiting {
                                self.complete();
                            }
                        } else if self.audio.enabled() && self.endpoints[self.index][2] & 0x40 != 0
                        {
                            // Isochronous IN: the packet waits for the host's
                            // IN token in the next frame (FlushFIFO drops it).
                            let ready = u8::from(data & 9 == 1);
                            self.endpoints[self.index][1] = (data & !0xc9) | ready;
                        } else {
                            self.endpoints[self.index][1] = data & !0xc9;
                            if data & 1 != 0 {
                                self.send(self.index, ram)?;
                            }
                        }
                    } else if r == 20 && self.index > 0 {
                        // RXCSR1: FlushFIFO (bit 4) or a 0 in RxPktRdy (bit 0)
                        // frees the buffer; ClrDataTog (bit 7) self-clears.
                        let ready = self.endpoints[self.index][4] & 1;
                        let keep = if data & 0x11 == 1 { ready } else { 0 };
                        self.endpoints[self.index][4] = (data & !0x91) | keep;
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
    /// Oscillator ticks until the next tick at which `advance(1)` does more
    /// than count: a frame boundary, attachment or a host SETUP. Valid when
    /// the previous tick already ran with the current register state.
    pub(crate) fn ticks_to_event(&self) -> Option<u64> {
        if self.regs[0] & 4 == 0 || self.io & 0x40 == 0 {
            return None;
        }
        if !self.attached {
            return Some(1);
        }
        if self.midi_deliverable() || self.audio.data_stage_pending() {
            return Some(1);
        }
        let frame = 24000 - self.ticks % 24000;
        if self.waiting || self.phase >= self.last_phase() {
            return Some(frame);
        }
        Some(frame.min(self.deadline.saturating_sub(self.ticks).max(1)))
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
            if self.audio.streaming() {
                self.audio_frame(ram)?;
            }
        }
        if self.audio.data_stage_pending() {
            let data = self.audio.take_data_stage();
            Self::dma(ram, self.regs[6], data.len())?.copy_from_slice(&data);
            self.dma_writes.push((self.regs[6], data.len()));
            self.endpoints[0][1] |= 1;
            self.endpoints[0][6] = data.len() as u8;
            self.sie[2] |= 1;
        }
        if !self.attached {
            self.attached = true;
            self.sie[6] |= 4;
            self.deadline = self.ticks + 120000;
        }
        if self.midi_deliverable() {
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
        if self.phase >= self.base_phases() {
            self.audio.begin(self.phase - self.base_phases());
        }
        Self::dma(ram, self.regs[6], 8)?.copy_from_slice(&setup);
        self.response.clear();
        self.endpoints[0][1] = 1;
        self.endpoints[0][6] = 8;
        self.sie[2] |= 1;
        self.waiting = true;
        self.setups += 1;
        Ok(())
    }
    /// One 1 ms frame of the open audio streams: the IN tokens of the
    /// capture and feedback endpoints take what the device armed, then the
    /// OUT stream's packet, sized by the device's feedback, goes into the
    /// endpoint's RX buffer unless the device still owns it.
    fn audio_frame(&mut self, ram: &mut [u8]) -> Result<(), &'static str> {
        for ep in self.audio.in_endpoints() {
            let e = &mut self.endpoints[ep];
            if e[1] & 1 == 0 {
                self.audio.in_missed(ep);
                continue;
            }
            e[1] &= !1;
            self.sie[2] |= 1 << ep;
            let n = self.regs[2 + ep] as usize;
            if n > 1023 {
                return Err("USB isochronous packet exceeds 1023 bytes");
            }
            let bytes = Self::dma(ram, self.regs[7 + (ep - 1) * 2], n)?;
            self.audio.in_packet(ep, bytes);
        }
        if let Some((ep, packet)) = self.audio.out_packet() {
            let e = &mut self.endpoints[ep];
            if e[4] & 1 != 0 {
                self.audio.out_lost();
                return Ok(());
            }
            let address = self.regs[8 + (ep - 1) * 2];
            Self::dma(ram, address, packet.len())?.copy_from_slice(&packet);
            self.dma_writes.push((address, packet.len()));
            e[4] |= 1;
            e[6] = packet.len() as u8;
            e[7] = (packet.len() >> 8) as u8;
            self.sie[4] |= 1 << ep;
            self.audio.out_delivered();
        }
        Ok(())
    }
}
