// SPDX-License-Identifier: GPL-3.0-only
//! USB Audio Class 1.0 (UAC1, "USB Device Class Definition for Audio
//! Devices" 1.0) from the host's side: the configuration checks a class
//! driver makes before it opens a stream, the 10.14 explicit-feedback frame
//! clock of a full-speed asynchronous OUT stream, PCM packing and WAV dumps.
//! The USB host model (usb.rs) uses it to stream audio with a firmware.

/// 44.1 kHz in 10.14 frames per 1 ms frame (UAC1 5.12.4.2), rounded down.
pub const NOMINAL_44K1: u32 = 44_100 * 16_384 / 1_000;

/// One alternate setting of an audio streaming interface (PCM, type I).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltSetting {
    pub alt: u8,
    pub channels: u8,
    /// Bytes per sample (bSubframeSize).
    pub width: u8,
    pub bits: u8,
    /// The highest discrete sampling frequency.
    pub rate: u32,
    /// Endpoint address, direction bit included.
    pub endpoint: u8,
    pub max_packet: u16,
    /// The explicit feedback endpoint of an asynchronous OUT stream.
    pub feedback: Option<u8>,
}

/// An audio streaming interface and its alternate settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInterface {
    pub interface: u8,
    /// iInterface of the audio control interface that lists it (the name a
    /// host gives the device).
    pub function_name: u8,
    pub alts: Vec<AltSetting>,
}

impl StreamInterface {
    pub fn alt(&self, alt: u8) -> Option<&AltSetting> {
        self.alts.iter().find(|a| a.alt == alt)
    }
}

/// The streams of a configuration: OUT (playback, host to device) and IN
/// (capture). At most one of each is modeled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioFunctions {
    pub playback: Option<StreamInterface>,
    pub capture: Option<StreamInterface>,
}

struct Interface<'a> {
    number: u8,
    alt: u8,
    class: (u8, u8),
    name: u8,
    endpoints: u8,
    /// Class-specific and endpoint descriptors up to the next interface.
    body: Vec<&'a [u8]>,
}

fn descriptors(config: &[u8]) -> Result<Vec<&[u8]>, String> {
    if config.len() < 9 || config[1] != 2 {
        return Err("not a configuration descriptor".into());
    }
    let total = u16::from_le_bytes([config[2], config[3]]) as usize;
    if total != config.len() {
        return Err(format!("wTotalLength {total} but {} bytes", config.len()));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < config.len() {
        let n = config[i] as usize;
        if n < 2 || i + n > config.len() {
            return Err(format!("descriptor at {i} has bad bLength {n}"));
        }
        out.push(&config[i..i + n]);
        i += n;
    }
    Ok(out)
}

fn interfaces<'a>(records: &[&'a [u8]]) -> Result<Vec<Interface<'a>>, String> {
    let mut out: Vec<Interface> = Vec::new();
    for d in &records[1..] {
        match d[1] {
            4 if d.len() >= 9 => out.push(Interface {
                number: d[2],
                alt: d[3],
                endpoints: d[4],
                class: (d[5], d[6]),
                name: d[8],
                body: Vec::new(),
            }),
            0x0B => {}
            _ => match out.last_mut() {
                Some(i) => i.body.push(d),
                None => return Err("descriptor before the first interface".into()),
            },
        }
    }
    for i in &out {
        let n = i.body.iter().filter(|d| d[1] == 5).count();
        if n != i.endpoints as usize {
            return Err(format!(
                "interface {} alt {} has {n} endpoints, bNumEndpoints {}",
                i.number, i.alt, i.endpoints
            ));
        }
    }
    let mut numbers: Vec<u8> = out.iter().map(|i| i.number).collect();
    numbers.dedup();
    let distinct = {
        let mut n = numbers.clone();
        n.sort_unstable();
        n.dedup();
        n
    };
    if distinct.len() != numbers.len() {
        return Err("alternate settings of an interface are not adjacent".into());
    }
    if distinct.len() != records[0][4] as usize {
        return Err(format!(
            "bNumInterfaces {} but {} interfaces",
            records[0][4],
            distinct.len()
        ));
    }
    if distinct.iter().enumerate().any(|(k, n)| k != *n as usize) {
        return Err("interface numbers are not 0..n".into());
    }
    Ok(out)
}

fn check_iads(records: &[&[u8]], ifs: &[Interface]) -> Result<(), String> {
    for d in records.iter().filter(|d| d[1] == 0x0B) {
        if d.len() < 8 {
            return Err("short IAD".into());
        }
        let (first, count) = (d[2], d[3]);
        let Some(head) = ifs.iter().find(|i| i.number == first) else {
            return Err(format!("IAD names missing interface {first}"));
        };
        if (d[4], d[5]) != head.class {
            return Err(format!("IAD class differs from interface {first}"));
        }
        if !(first..first.saturating_add(count)).all(|n| ifs.iter().any(|i| i.number == n)) {
            return Err(format!("IAD {first}+{count} covers a missing interface"));
        }
    }
    Ok(())
}

/// A terminal of an audio control interface: (id, type, channels, source).
type Terminal = (u8, u16, u8, Option<u8>);

/// The terminals and streaming interfaces of one audio control interface.
fn audio_control(ac: &Interface, ifs: &[Interface]) -> Result<(Vec<Terminal>, Vec<u8>), String> {
    let cs: Vec<&[u8]> = ac.body.iter().copied().filter(|d| d[1] == 0x24).collect();
    let header = cs
        .first()
        .filter(|h| h.len() >= 8 && h[2] == 1)
        .ok_or("audio control without a header")?;
    if u16::from_le_bytes([header[3], header[4]]) != 0x0100 {
        return Err("bcdADC is not 1.00 (UAC1)".into());
    }
    let total = u16::from_le_bytes([header[5], header[6]]) as usize;
    let sum: usize = cs.iter().map(|d| d.len()).sum();
    if total != sum {
        return Err(format!("AC wTotalLength {total} but {sum} bytes"));
    }
    let n = header[7] as usize;
    if header.len() != 8 + n {
        return Err("AC header bLength and bInCollection differ".into());
    }
    let streams = header[8..].to_vec();
    for s in &streams {
        let ok = ifs
            .iter()
            .any(|i| i.number == *s && i.class.0 == 1 && (i.class.1 == 2 || i.class.1 == 3));
        if !ok {
            return Err(format!("baInterfaceNr {s} is not a streaming interface"));
        }
    }
    let mut terminals = Vec::new();
    for d in &cs[1..] {
        match d[2] {
            2 if d.len() >= 12 => {
                terminals.push((d[3], u16::from_le_bytes([d[4], d[5]]), d[7], None))
            }
            3 if d.len() >= 9 => {
                terminals.push((d[3], u16::from_le_bytes([d[4], d[5]]), 0, Some(d[7])))
            }
            _ => {}
        }
    }
    for t in &terminals {
        if let Some(src) = t.3 {
            if !terminals.iter().any(|s| s.0 == src && s.3.is_none()) {
                return Err(format!("terminal {} has missing bSourceID {src}", t.0));
            }
        }
    }
    Ok((terminals, streams))
}

/// Channels at a terminal: an input terminal's own, an output terminal's
/// from its source.
fn channels(terminals: &[Terminal], t: &Terminal) -> u8 {
    match t.3 {
        None => t.2,
        Some(src) => terminals.iter().find(|s| s.0 == src).map_or(0, |s| s.2),
    }
}

fn stream_alt(i: &Interface, terminals: &[Terminal]) -> Result<AltSetting, String> {
    let at = format!("interface {} alt {}", i.number, i.alt);
    let general = i
        .body
        .iter()
        .find(|d| d[1] == 0x24 && d[2] == 1 && d.len() >= 7)
        .ok_or(format!("{at}: no AS_GENERAL"))?;
    if u16::from_le_bytes([general[5], general[6]]) != 1 {
        return Err(format!("{at}: wFormatTag is not PCM"));
    }
    let format = i
        .body
        .iter()
        .find(|d| d[1] == 0x24 && d[2] == 2 && d.len() >= 8)
        .ok_or(format!("{at}: no FORMAT_TYPE"))?;
    let (kind, ch, width, bits, nrates) = (format[3], format[4], format[5], format[6], format[7]);
    if kind != 1 || nrates == 0 || format.len() != 8 + 3 * nrates as usize {
        return Err(format!("{at}: not a type I format with discrete rates"));
    }
    if !(1..=4).contains(&width) || bits == 0 || bits > width * 8 {
        return Err(format!("{at}: bad subframe {width} / {bits} bits"));
    }
    let rate = format[8..]
        .chunks(3)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], 0]))
        .max()
        .unwrap_or(0);
    let ep = i
        .body
        .iter()
        .find(|d| d[1] == 5 && d.len() >= 7 && d[3] & 0x30 == 0)
        .ok_or(format!("{at}: no data endpoint"))?;
    let input = ep[2] & 0x80 != 0;
    let link = general[3];
    let terminal = terminals
        .iter()
        .find(|t| t.0 == link && t.1 == 0x0101 && t.3.is_none() != input)
        .ok_or(format!(
            "{at}: bTerminalLink {link} is not its USB streaming terminal"
        ))?;
    if channels(terminals, terminal) != ch {
        return Err(format!(
            "{at}: {ch} channels, terminal {link} has {}",
            channels(terminals, terminal)
        ));
    }
    if ep.len() != 9 || ep[3] & 3 != 1 {
        return Err(format!(
            "{at}: data endpoint is not a 9-byte isochronous one"
        ));
    }
    let max_packet = u16::from_le_bytes([ep[4], ep[5]]) & 0x7FF;
    let need = rate.div_ceil(1000) * ch as u32 * width as u32;
    if (max_packet as u32) < need || max_packet > 1023 {
        return Err(format!(
            "{at}: wMaxPacketSize {max_packet}, needs {need}..1023"
        ));
    }
    if !i
        .body
        .iter()
        .any(|d| d[1] == 0x25 && d.len() >= 7 && d[2] == 1)
    {
        return Err(format!("{at}: no class-specific endpoint descriptor"));
    }
    let sync = (ep[3] >> 2) & 3;
    let feedback = if !input && sync == 1 {
        let address = ep[8];
        let fb = i
            .body
            .iter()
            .find(|d| d[1] == 5 && d.len() == 9 && d[2] == address && address & 0x80 != 0)
            .ok_or(format!(
                "{at}: bSynchAddress {address:#x} is no feedback endpoint"
            ))?;
        let size = u16::from_le_bytes([fb[4], fb[5]]);
        if fb[3] & 3 != 1 || size < 3 || !(1..=9).contains(&fb[7]) || fb[8] != 0 {
            return Err(format!(
                "{at}: feedback endpoint {address:#x} is not 10.14 isochronous"
            ));
        }
        Some(address)
    } else {
        None
    };
    Ok(AltSetting {
        alt: i.alt,
        channels: ch,
        width,
        bits,
        rate,
        endpoint: ep[2],
        max_packet,
        feedback,
    })
}

/// The audio streams of a configuration descriptor, after the checks a UAC1
/// class driver needs (structure, IADs, AC headers and terminals, formats,
/// endpoints and feedback). A configuration without audio streams gives
/// none; an inconsistent one an error naming the field.
pub fn parse_uac1(config: &[u8]) -> Result<AudioFunctions, String> {
    let records = descriptors(config)?;
    let ifs = interfaces(&records)?;
    check_iads(&records, &ifs)?;
    let mut functions = AudioFunctions::default();
    let mut claimed = Vec::new();
    for ac in ifs.iter().filter(|i| i.class == (1, 1)) {
        let (terminals, streams) = audio_control(ac, &ifs)?;
        let iad = records
            .iter()
            .find(|d| d[1] == 0x0B && (d[2]..d[2].saturating_add(d[3])).contains(&ac.number));
        for number in streams {
            if claimed.contains(&number) {
                return Err(format!(
                    "baInterfaceNr {number} listed by two audio controls"
                ));
            }
            claimed.push(number);
            if iad.is_some_and(|d| !(d[2]..d[2].saturating_add(d[3])).contains(&number)) {
                return Err(format!(
                    "baInterfaceNr {number} is outside the function of interface {}",
                    ac.number
                ));
            }
            let set: Vec<&Interface> = ifs.iter().filter(|i| i.number == number).collect();
            if set[0].class.1 != 2 {
                continue; // MIDI streaming
            }
            if set.iter().any(|i| i.alt == 0 && i.endpoints != 0) {
                return Err(format!("interface {number} alt 0 has endpoints"));
            }
            let alts = set
                .iter()
                .filter(|i| i.alt != 0)
                .map(|i| stream_alt(i, &terminals))
                .collect::<Result<Vec<_>, _>>()?;
            let Some(first) = alts.first() else {
                return Err(format!("interface {number} has no streaming alternate"));
            };
            let input = first.endpoint & 0x80 != 0;
            let stream = StreamInterface {
                interface: number,
                function_name: ac.name,
                alts,
            };
            let slot = if input {
                &mut functions.capture
            } else {
                &mut functions.playback
            };
            if slot.replace(stream).is_some() {
                return Err("more than one stream in a direction".into());
            }
        }
    }
    Ok(functions)
}

/// The frames of an asynchronous OUT stream, one 1 ms frame at a time, as a
/// host derives them from the device's 10.14 feedback.
#[derive(Clone, Copy, Debug, Default)]
pub struct FeedbackClock {
    fraction: u32,
}

impl FeedbackClock {
    pub fn frames(&mut self, feedback_10_14: u32) -> u32 {
        self.fraction += feedback_10_14;
        let n = self.fraction >> 14;
        self.fraction &= 0x3FFF;
        n
    }
}

/// Little-endian signed PCM of `width` bytes per sample.
pub fn decode_pcm(bytes: &[u8], width: usize) -> Vec<i32> {
    bytes
        .chunks_exact(width)
        .map(|c| {
            let mut v = [0u8; 4];
            v[4 - width..].copy_from_slice(c);
            i32::from_le_bytes(v) >> (8 * (4 - width))
        })
        .collect()
}

pub fn encode_pcm(samples: &[i32], width: usize) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|s| s.to_le_bytes().into_iter().take(width))
        .collect()
}

/// A PCM WAV file (16 or 24 bit) of interleaved samples.
pub fn wav_bytes(rate: u32, channels: u16, bits: u16, samples: &[i32]) -> Vec<u8> {
    let width = bits as usize / 8;
    let data = encode_pcm(samples, width);
    let align = channels * bits / 8;
    let mut w = Vec::with_capacity(44 + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * align as u32).to_le_bytes());
    w.extend_from_slice(&align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    w
}
