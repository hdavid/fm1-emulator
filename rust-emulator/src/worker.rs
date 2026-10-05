// SPDX-License-Identifier: GPL-3.0-only
// Private GUI worker: one owner of the CPU, and one replaceable UI snapshot.
use super::host_audio::{AudioQueue, TARGET_FRAMES};
use fm1_emu::{cpu::Cpu, encoders::Encoders, firmware::Firmware};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct Snapshot {
    pub generation: u64,
    pub loaded: bool,
    pub steps: u64,
    /// Stereo frames the guest has rendered through its audio DMA.
    pub frames: u64,
    pub fault: Option<String>,
    pub pixels: Option<Vec<u32>>,
}

enum Command {
    Restart(u64, PathBuf),
    Input([bool; 41]),
    /// Queue detents (+ clockwise) on a matrix encoder.
    Turn(usize, i32),
    /// The MASTER potentiometer as the ADC reads it (0..=1023).
    Master(u16),
    /// Instructions per second of guest time; None follows the firmware.
    Clock(Option<u32>),
    /// Where guest audio goes; the worker then paces the guest by it.
    Audio(Option<AudioQueue>),
    Pause(bool),
    Stop,
    #[cfg(test)]
    Inspect(Box<dyn FnOnce(&mut Machine) + Send>),
}

pub(super) struct Worker {
    commands: mpsc::Sender<Command>,
    serial: mpsc::SyncSender<Vec<u8>>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    thread: Option<JoinHandle<()>>,
}

pub(super) struct Machine {
    pub cpu: Option<Cpu>,
    pub fault: Option<String>,
    pub paused: bool,
    pub encoders: Encoders,
    master: u16,
    clock: Option<u32>,
    audio: Option<AudioQueue>,
    generation: u64,
    last_lcd: Option<(u64, bool)>,
}

impl Machine {
    fn new() -> Self {
        Self {
            cpu: None,
            fault: None,
            paused: false,
            encoders: Encoders::default(),
            master: super::MASTER_DEFAULT,
            clock: None,
            audio: None,
            generation: 0,
            last_lcd: None,
        }
    }
    fn command(&mut self, command: Command) -> bool {
        match command {
            Command::Restart(generation, path) => {
                self.generation = generation;
                self.paused = false;
                self.last_lcd = None;
                self.encoders = Encoders::default();
                if let Some(audio) = &self.audio {
                    audio.clear();
                }
                let (master, clock) = (self.master, self.clock);
                match Firmware::load(&path).and_then(|firmware| {
                    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
                    cpu.r[0] = 0x01c7fe08;
                    cpu.bus.devices.adc.master = master;
                    cpu.bus.set_instruction_clock(clock);
                    Ok(cpu)
                }) {
                    Ok(cpu) => {
                        self.cpu = Some(cpu);
                        self.fault = None;
                    }
                    Err(error) => {
                        self.cpu = None;
                        self.fault = Some(error);
                    }
                }
            }
            Command::Input(pressed) => {
                if let Some(cpu) = &mut self.cpu {
                    for (row, ids) in super::KEYMAP.iter().enumerate() {
                        for (column, &id) in ids.iter().enumerate() {
                            if id >= 0 {
                                cpu.bus
                                    .devices
                                    .gpio
                                    .press(column, row + 1, pressed[id as usize])
                                    .unwrap();
                            }
                        }
                    }
                }
            }
            Command::Turn(encoder, detents) => self.encoders.turn(encoder, detents),
            Command::Master(value) => {
                self.master = value;
                if let Some(cpu) = &mut self.cpu {
                    cpu.bus.devices.adc.master = value;
                }
            }
            Command::Clock(hz) => {
                self.clock = hz;
                if let Some(cpu) = &mut self.cpu {
                    cpu.bus.set_instruction_clock(hz);
                }
            }
            Command::Audio(queue) => self.audio = queue,
            Command::Pause(paused) => self.paused = paused,
            Command::Stop => return false,
            #[cfg(test)]
            Command::Inspect(inspect) => inspect(self),
        }
        true
    }
    fn running(&self) -> bool {
        self.cpu.is_some() && !self.paused && self.fault.is_none()
    }
    /// While the guest streams audio to the host, its pace is the playback
    /// queue: real time when the host keeps up.
    fn ahead(&self) -> bool {
        match (&self.audio, &self.cpu) {
            (Some(audio), Some(cpu)) => cpu.bus.audio.frames > 0 && audio.queued() >= TARGET_FRAMES,
            _ => false,
        }
    }
    fn execute(&mut self) {
        if !self.running() {
            return;
        }
        let cpu = self.cpu.as_mut().unwrap();
        // Encoder phases advance with the guest's own matrix scans.
        self.encoders.drive(&mut cpu.bus.devices.gpio);
        // Poll commands between small batches, independently of repaint rate.
        for _ in 0..1024 {
            if let Err(error) = cpu.step() {
                self.fault = Some(error.to_string());
                break;
            }
        }
        if let Some(audio) = &self.audio {
            audio.push(cpu.bus.audio.samples.drain(..));
        }
        if !cpu.bus.usb.serial.is_empty() {
            let bytes: Vec<_> = cpu.bus.usb.serial.drain(..).collect();
            let mut stdout = io::stdout().lock();
            if let Err(error) = stdout.write_all(&bytes).and_then(|_| stdout.flush()) {
                self.fault = Some(format!("USB serial stdout: {error}"));
            }
        }
    }
    fn publish(&mut self, mailbox: &Mutex<Option<Snapshot>>) {
        let mut snapshot = Snapshot {
            generation: self.generation,
            loaded: self.cpu.is_some(),
            steps: self.cpu.as_ref().map_or(0, |cpu| cpu.steps),
            frames: self.cpu.as_ref().map_or(0, |cpu| cpu.bus.audio.frames),
            fault: self.fault.clone(),
            pixels: None,
        };
        if let Some(cpu) = &self.cpu {
            let visible = cpu.bus.screen_visible();
            let lcd = (cpu.bus.lcd.pixels_written, visible);
            if self.last_lcd != Some(lcd) {
                self.last_lcd = Some(lcd);
                snapshot.pixels = Some(if visible {
                    cpu.bus.lcd.pixels.clone()
                } else {
                    vec![0; 240 * 240]
                });
            }
        }
        let mut pending = mailbox.lock().unwrap();
        // A status-only update must not overwrite an unconsumed LCD frame.
        if snapshot.pixels.is_none() {
            if let Some(previous) = pending
                .as_mut()
                .filter(|p| p.generation == snapshot.generation)
            {
                snapshot.pixels = previous.pixels.take();
            }
        }
        *pending = Some(snapshot);
    }
}

impl Worker {
    pub fn new() -> Self {
        let (commands, receiver) = mpsc::channel();
        let (serial, serial_receiver) = mpsc::sync_channel(16);
        let snapshot = Arc::new(Mutex::new(None));
        let mailbox = Arc::clone(&snapshot);
        let thread = thread::spawn(move || {
            let mut machine = Machine::new();
            let mut pending_serial: Option<Vec<u8>> = None;
            let mut next_frame = Instant::now();
            loop {
                while let Ok(command) = receiver.try_recv() {
                    if matches!(&command, Command::Restart(..)) && machine.generation != 0 {
                        pending_serial = None;
                        while serial_receiver.try_recv().is_ok() {}
                    }
                    if !machine.command(command) {
                        return;
                    }
                    machine.publish(&mailbox);
                }
                if let Some(cpu) = &mut machine.cpu {
                    if pending_serial.is_none() {
                        pending_serial = serial_receiver.try_recv().ok();
                    }
                    if pending_serial
                        .as_ref()
                        .is_some_and(|bytes| cpu.bus.usb.receive_serial(bytes))
                    {
                        pending_serial = None;
                    }
                }
                if machine.running() && machine.ahead() {
                    thread::sleep(Duration::from_millis(1));
                } else if machine.running() {
                    machine.execute();
                } else {
                    match receiver.recv_timeout(Duration::from_millis(16)) {
                        Ok(command) => {
                            if matches!(&command, Command::Restart(..)) && machine.generation != 0 {
                                pending_serial = None;
                                while serial_receiver.try_recv().is_ok() {}
                            }
                            if !machine.command(command) {
                                return;
                            }
                            machine.publish(&mailbox);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                if Instant::now() >= next_frame {
                    machine.publish(&mailbox);
                    next_frame = Instant::now() + Duration::from_millis(16);
                }
            }
        });
        Self {
            commands,
            serial,
            snapshot,
            thread: Some(thread),
        }
    }
    pub fn restart(&self, generation: u64, path: PathBuf) {
        let _ = self.commands.send(Command::Restart(generation, path));
    }
    pub fn input(&self, pressed: [bool; 41]) {
        let _ = self.commands.send(Command::Input(pressed));
    }
    pub fn turn(&self, encoder: usize, detents: i32) {
        let _ = self.commands.send(Command::Turn(encoder, detents));
    }
    pub fn master(&self, value: u16) {
        let _ = self.commands.send(Command::Master(value));
    }
    pub fn clock(&self, hz: Option<u32>) {
        let _ = self.commands.send(Command::Clock(hz));
    }
    pub fn audio(&self, queue: Option<AudioQueue>) {
        let _ = self.commands.send(Command::Audio(queue));
    }
    pub fn pause(&self, paused: bool) {
        let _ = self.commands.send(Command::Pause(paused));
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.snapshot.lock().unwrap().take()
    }
    pub fn read_stdin(&self) {
        let serial = self.serial.clone();
        // A separate reader can block on a terminal without stopping either the
        // CPU or the window. The bounded queue also backpressures piped input.
        // Do not join it on close: portable stdin reads cannot be cancelled.
        thread::spawn(move || {
            let mut input = io::stdin().lock();
            let mut bytes = [0; 64];
            loop {
                match input.read(&mut bytes) {
                    Ok(0) => return,
                    Ok(n) => {
                        if serial.send(bytes[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        eprintln!("USB serial stdin: {error}");
                        return;
                    }
                }
            }
        });
    }
    #[cfg(test)]
    pub fn inspect<T: Send + 'static>(
        &self,
        inspect: impl FnOnce(&mut Machine) -> T + Send + 'static,
    ) -> T {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mailbox = Arc::clone(&self.snapshot);
        self.commands
            .send(Command::Inspect(Box::new(move |machine| {
                let result = inspect(machine);
                machine.publish(&mailbox);
                sender.send(result).unwrap();
            })))
            .unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm1_emu::bus::Bus;

    #[test]
    fn status_updates_preserve_an_unconsumed_lcd_frame_but_restart_discards_it() {
        let mailbox = Mutex::new(None);
        let mut machine = Machine::new();
        machine.cpu = Some(Cpu::new(Bus::new(vec![0; 8]).unwrap(), fm1_emu::XIP));
        let cpu = machine.cpu.as_mut().unwrap();
        cpu.bus.lcd.display_on = true;
        cpu.bus.lcd.sleeping = false;
        cpu.bus.write(0x50008, !4, 4).unwrap();
        cpu.bus.lcd.pixels[0] = 0x123456;
        cpu.bus.lcd.pixels_written = 1;
        machine.publish(&mailbox);
        machine.cpu.as_mut().unwrap().steps = 200;
        machine.publish(&mailbox);
        let snapshot = mailbox.lock().unwrap().take().unwrap();
        assert_eq!(snapshot.steps, 200);
        assert_eq!(snapshot.pixels.unwrap()[0], 0x123456);
        machine.last_lcd = None;
        machine.publish(&mailbox);
        machine.generation += 1;
        machine.cpu = None;
        machine.publish(&mailbox);
        let snapshot = mailbox.lock().unwrap().take().unwrap();
        assert_eq!(snapshot.generation, 1);
        assert!(!snapshot.loaded);
        assert!(snapshot.pixels.is_none());
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
