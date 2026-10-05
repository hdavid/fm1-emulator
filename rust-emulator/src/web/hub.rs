// SPDX-License-Identifier: GPL-3.0-only
//! Where the emulator and the WebSocket clients meet: complete MIDI
//! messages for the device, and the device's messages for every client.
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, MutexGuard,
    },
};

/// Messages waiting for the emulator before the oldest are dropped (the
/// emulator drains every UI frame; this only bounds a stopped emulator).
const TO_DEVICE_MAX: usize = 4096;

/// Counters for the status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// WebSocket clients connected now.
    pub clients: usize,
    /// Complete messages from the clients to the device / back.
    pub to_device: u64,
    pub from_device: u64,
}

#[derive(Default)]
struct State {
    to_device: VecDeque<Vec<u8>>,
    clients: Vec<(u64, Sender<Vec<u8>>)>,
    next_id: u64,
    stats: Stats,
    product: Option<String>,
    ready: bool,
}

#[derive(Default)]
pub struct Hub {
    state: Mutex<State>,
}

impl Hub {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking client thread cannot leave the queues half-updated
        // (every update is a single push or retain), so poison is harmless.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// A new client: its id and where its device messages arrive.
    pub(crate) fn connect(&self) -> (u64, Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::channel();
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.clients.push((id, sender));
        state.stats.clients = state.clients.len();
        (id, receiver)
    }

    pub(crate) fn disconnect(&self, id: u64) {
        let mut state = self.lock();
        state.clients.retain(|(client, _)| *client != id);
        state.stats.clients = state.clients.len();
    }

    /// A complete message from a client, for the device.
    pub(crate) fn to_device(&self, message: Vec<u8>) {
        let mut state = self.lock();
        if state.to_device.len() == TO_DEVICE_MAX {
            state.to_device.pop_front();
        }
        state.to_device.push_back(message);
        state.stats.to_device += 1;
    }

    /// Everything the clients sent since the last call, oldest first.
    pub fn take_to_device(&self) -> Vec<Vec<u8>> {
        self.lock().to_device.drain(..).collect()
    }

    /// A complete message from the device, to every client.
    pub fn broadcast(&self, message: &[u8]) {
        let mut state = self.lock();
        state
            .clients
            .retain(|(_, client)| client.send(message.to_vec()).is_ok());
        state.stats.clients = state.clients.len();
        state.stats.from_device += 1;
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// What the USB host learned: the product string and whether the MIDI
    /// endpoints are open.
    pub fn set_device(&self, product: Option<&str>, ready: bool) {
        let mut state = self.lock();
        state.product = product.map(str::to_owned);
        state.ready = ready;
    }

    /// The Web MIDI port name: the USB product string, as an OS names a
    /// class-compliant device (editors look for "Felucca"), and that it is
    /// the emulator.
    pub fn port_name(&self) -> String {
        match &self.lock().product {
            Some(product) => format!("{product} (FM-1 Emulator)"),
            None => String::from("FM-1 Emulator"),
        }
    }

    pub fn ready(&self) -> bool {
        self.lock().ready
    }
}
