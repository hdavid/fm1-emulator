// SPDX-License-Identifier: GPL-3.0-only
// Private GUI worker: one owner of the CPU, and one replaceable UI snapshot.
use fm1_emu::{cpu::Cpu, firmware::Firmware};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct Snapshot {
    pub generation: u64,
    pub loaded: bool,
    pub steps: u64,
    pub fault: Option<String>,
    pub pixels: Option<Vec<u32>>,
}

enum Command {
    Restart(u64, PathBuf),
    Input([bool; 41]),
    Pause(bool),
    Stop,
    #[cfg(test)]
    Inspect(Box<dyn FnOnce(&mut Machine) + Send>),
}

pub(super) struct Worker {
    commands: mpsc::Sender<Command>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    thread: Option<JoinHandle<()>>,
}

pub(super) struct Machine {
    pub cpu: Option<Cpu>,
    pub fault: Option<String>,
    pub paused: bool,
    generation: u64,
    last_lcd: Option<(u64, bool)>,
}

impl Machine {
    fn new() -> Self {
        Self {
            cpu: None,
            fault: None,
            paused: false,
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
                match Firmware::load(&path).and_then(|firmware| {
                    let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
                    cpu.r[0] = 0x01c7fe08;
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
    fn execute(&mut self) {
        if !self.running() {
            return;
        }
        let cpu = self.cpu.as_mut().unwrap();
        // Poll commands between small batches, independently of repaint rate.
        for _ in 0..1024 {
            if let Err(error) = cpu.step() {
                self.fault = Some(error.to_string());
                break;
            }
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
        let snapshot = Arc::new(Mutex::new(None));
        let mailbox = Arc::clone(&snapshot);
        let thread = thread::spawn(move || {
            let mut machine = Machine::new();
            let mut next_frame = Instant::now();
            loop {
                while let Ok(command) = receiver.try_recv() {
                    if !machine.command(command) {
                        return;
                    }
                    machine.publish(&mailbox);
                }
                if machine.running() {
                    machine.execute();
                } else {
                    match receiver.recv_timeout(Duration::from_millis(16)) {
                        Ok(command) => {
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
    pub fn pause(&self, paused: bool) {
        let _ = self.commands.send(Command::Pause(paused));
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.snapshot.lock().unwrap().take()
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
