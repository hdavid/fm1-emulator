// SPDX-License-Identifier: GPL-3.0-only
//! The web editor bridge: a loopback HTTP server for a firmware's web
//! editor, a Web MIDI shim injected into its pages, and a `/midi`
//! WebSocket that `pump` connects to the emulated USB-MIDI device.
mod hub;
mod server;
mod source;
mod ws;

pub use hub::{Hub, Stats};
pub use server::{inject_shim, Server, ADDRESS};
pub use source::{clean_path, mime_type, percent_decode, sidecar_for, Source};

use crate::{
    bus::Bus,
    usb_midi::{encode, Decoder},
};

/// Move MIDI between the hub and the emulated device once: client messages
/// become USB-MIDI packets for the device's OUT endpoint, the device's
/// packets become complete messages for every client. Call it from the
/// thread that runs the CPU (each UI frame); the MIDI host must be enabled
/// (`bus.usb.enable_midi_host()`).
pub fn pump(hub: &Hub, bus: &mut Bus, decoder: &mut Decoder) {
    for message in hub.take_to_device() {
        bus.usb_midi_send(&encode(&message, 0));
    }
    let packets: Vec<_> = bus.usb.midi_received.drain(..).collect();
    for message in packets.into_iter().filter_map(|p| decoder.push(p)) {
        hub.broadcast(&message);
    }
    hub.set_device(bus.usb.product(), bus.usb.midi_ready());
}
