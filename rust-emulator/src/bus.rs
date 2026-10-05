// SPDX-License-Identifier: GPL-3.0-only
use crate::devices::Devices;
use crate::lcd::{Lcd, SPI};
use crate::{RAM, RAM_SIZE, XIP, XIP_END};
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct AccessFault {
    pub address: u32,
    pub size: usize,
    pub operation: &'static str,
    pub reason: &'static str,
}

impl fmt::Display for AccessFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} bytes at 0x{:08x}: {}",
            self.operation, self.size, self.address, self.reason
        )
    }
}

pub struct Bus {
    flash: Vec<u8>,
    pub devices: Devices,
    pub lcd: Lcd,
    pub system: crate::system::System,
    ram: Vec<u8>,
    guards: crate::guards::Guards,
    nor: crate::nor::Nor,
    pub usb: crate::usb::Usb,
    pub audio: crate::audio::Audio,
    cache: crate::cache::Cache,
    crc: crate::crc::Crc,
    clock: crate::clock::Clock,
    resample: crate::resample::Resampler,
    rng: crate::rng::Rng,
    pub radio: crate::radio::Radio,
    pub spi2: crate::spi2::Spi2,
    code: crate::code_cache::CodeCache,
    /// NOR generation the code cache was last synchronized with.
    nor_generation: u64,
    /// Optional diagnostic counts of MMIO reads/writes by address.
    /// Keyed by (address, PC of the accessing instruction).
    pub mmio_stats: std::cell::RefCell<Option<std::collections::BTreeMap<(u32, u32), [u64; 2]>>>,
    /// PC of the instruction being executed (diagnostics only).
    pub pc_hint: std::cell::Cell<u32>,
    /// Oscillator ticks since reset (the CPU counts them).
    pub(crate) now: u64,
    /// Ticks the clocked devices have been advanced by.
    pub(crate) synced: u64,
    /// The tick that must run through the exact per-tick device path:
    /// until then every device only counts (see `catch_up`).
    pub(crate) next_event: u64,
}

impl Bus {
    pub(crate) fn load_flash(&mut self, bytes: &[u8], key: u16) {
        self.nor.load(bytes, key);
        self.sync_code_cache();
        // SPL handoff values measured before peripheral initialization.
        self.usb
            .write(0x10010, 0x10000, &mut self.ram)
            .unwrap()
            .unwrap();
        self.audio.write(0x10014, 6).unwrap().unwrap();
    }
    pub fn new(flash: Vec<u8>) -> Result<Self, String> {
        if flash.is_empty() || flash.len() > (XIP_END - XIP) as usize {
            return Err("application image is empty or exceeds the XIP window".into());
        }
        Ok(Self {
            flash,
            devices: Devices::default(),
            lcd: Lcd::default(),
            system: Default::default(),
            ram: vec![0; RAM_SIZE],
            guards: Default::default(),
            nor: Default::default(),
            usb: Default::default(),
            audio: Default::default(),
            cache: Default::default(),
            crc: Default::default(),
            clock: Default::default(),
            resample: Default::default(),
            rng: Default::default(),
            radio: Default::default(),
            spi2: Default::default(),
            code: Default::default(),
            nor_generation: 0,
            mmio_stats: Default::default(),
            pc_hint: Default::default(),
            now: 0,
            synced: 0,
            next_event: 0,
        })
    }

    fn fault(
        address: u32,
        size: usize,
        operation: &'static str,
        reason: &'static str,
    ) -> AccessFault {
        AccessFault {
            address,
            size,
            operation,
            reason,
        }
    }

    #[inline]
    fn count_mmio(&self, address: u32, kind: usize) {
        if let Some(stats) = self.mmio_stats.borrow_mut().as_mut() {
            stats.entry((address, self.pc_hint.get())).or_default()[kind] += 1;
        }
    }

    #[inline]
    /// The core control word at 0x1eee000 + 4 * core, as a 4-byte bus read
    /// returns it. The CPU reads it before every instruction; this skips the
    /// MMIO dispatch chain (it never faults) but still counts the access.
    pub(crate) fn core_control(&self, core: usize) -> u32 {
        self.count_mmio(0x1eee000 + 4 * core as u32, 0);
        self.cache.cores[core]
    }

    fn check(address: u32, size: usize, operation: &'static str) -> Result<(), AccessFault> {
        if !matches!(size, 1 | 2 | 4) {
            return Err(Self::fault(
                address,
                size,
                operation,
                "unsupported access width",
            ));
        }
        if !address.is_multiple_of(size as u32) {
            return Err(Self::fault(address, size, operation, "unaligned access"));
        }
        Ok(())
    }

    fn offset(address: u32, size: usize, base: u32, length: usize) -> Option<usize> {
        let offset = address.checked_sub(base)? as usize;
        (offset.checked_add(size)? <= length).then_some(offset)
    }

    /// SRAM bytes at `offset` as a little-endian value of `size` (1, 2 or 4)
    /// bytes; the caller has checked the range.
    #[inline(always)]
    fn ram_value(&self, offset: usize, size: usize) -> u32 {
        match size {
            4 => u32::from_le_bytes(self.ram[offset..offset + 4].try_into().unwrap()),
            2 => u16::from_le_bytes(self.ram[offset..offset + 2].try_into().unwrap()) as u32,
            _ => self.ram[offset] as u32,
        }
    }

    #[inline]
    fn read_as(
        &self,
        address: u32,
        size: usize,
        operation: &'static str,
    ) -> Result<u32, AccessFault> {
        Self::check(address, size, operation)?;
        // SRAM first: no device or XIP window overlaps it.
        let ram_offset = address.wrapping_sub(RAM) as usize;
        if ram_offset < RAM_SIZE {
            // Aligned accesses of at most 4 bytes never cross the end.
            return Ok(self.ram_value(ram_offset, size));
        }
        self.read_slow(address, size, operation)
    }

    fn read_slow(
        &self,
        address: u32,
        size: usize,
        operation: &'static str,
    ) -> Result<u32, AccessFault> {
        let bytes = if let Some(offset) = Self::offset(address, size, XIP, self.flash.len()) {
            if !self.nor.xip_active() {
                return Err(Self::fault(
                    address,
                    size,
                    operation,
                    "application XIP is disabled",
                ));
            }
            if self.nor.packaged() {
                return self
                    .nor
                    .xip(address, size)
                    .unwrap()
                    .map_err(|reason| Self::fault(address, size, operation, reason));
            }
            &self.flash[offset..offset + size]
        } else if let Some(offset) = Self::offset(address, size, RAM, self.ram.len()) {
            &self.ram[offset..offset + size]
        } else {
            self.count_mmio(address, 0);
            if let Some(value) = self.clock.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.crc.read(address) {
                return Ok(value);
            }
            if crate::spi2::Spi2::contains(address) {
                return if size == 4 && address.is_multiple_of(4) {
                    Ok(self.spi2.read(address))
                } else {
                    Err(Self::fault(
                        address,
                        size,
                        operation,
                        "SPI registers require word accesses",
                    ))
                };
            }
            if crate::radio::Radio::contains(address) {
                return match (size, self.radio.read(address)) {
                    (4, Some(value)) => Ok(value),
                    _ => Err(Self::fault(
                        address,
                        size,
                        operation,
                        "JL_WL stub requires word accesses",
                    )),
                };
            }
            if crate::rng::Rng::contains(address & !3) {
                return match (size, self.rng.read(address)) {
                    (4, Some(value)) => Ok(value),
                    _ => Err(Self::fault(
                        address,
                        size,
                        operation,
                        "JL_RAND registers require word reads",
                    )),
                };
            }
            if let Some(value) = self.resample.read(address & !3) {
                return if size == 4 {
                    Ok(value)
                } else {
                    Err(Self::fault(
                        address,
                        size,
                        operation,
                        "JL_SRC registers require word accesses",
                    ))
                };
            }
            if let Some(value) = self.cache.read(address, size) {
                return Ok(value);
            }
            if let Some(value) = self.audio.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.nor.xip(address, size) {
                return value.map_err(|reason| Self::fault(address, size, operation, reason));
            }
            if let Some(value) = self.usb.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.nor.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.guards.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.system.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.lcd.read(address & !3) {
                return if size == 4 {
                    Ok(value)
                } else {
                    Err(Self::fault(
                        address,
                        size,
                        operation,
                        "SPI registers require word accesses",
                    ))
                };
            }
            if let Some(value) = self.devices.read_at(address, size, self.now - self.synced) {
                return value
                    .map(|value| {
                        if address == crate::devices::IRQ_PENDING && self.audio.pending_irq() {
                            value | (1 << crate::audio::IRQ)
                        } else {
                            value
                        }
                    })
                    .map_err(|reason| Self::fault(address, size, operation, reason));
            }
            // No generic zero-filled MMIO: missing peripherals must be visible.
            return Err(Self::fault(
                address,
                size,
                operation,
                "unmapped memory or unimplemented MMIO",
            ));
        };
        Ok(bytes
            .iter()
            .enumerate()
            .fold(0, |value, (i, byte)| value | ((*byte as u32) << (i * 8))))
    }

    /// A code halfword from memory whose reads have no side effects and can
    /// be cached: the first two cases of `read_as`, when they succeed.
    fn code_halfword(&self, address: u32) -> Option<u16> {
        if address & 1 != 0 {
            return None;
        }
        let bytes = if let Some(offset) = Self::offset(address, 2, XIP, self.flash.len()) {
            if !self.nor.xip_active() {
                return None;
            }
            if self.nor.packaged() {
                return self.nor.xip(address, 2)?.ok().map(|value| value as u16);
            }
            &self.flash[offset..offset + 2]
        } else {
            let offset = Self::offset(address, 2, RAM, self.ram.len())?;
            &self.ram[offset..offset + 2]
        };
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// The decoded instruction at `pc`, if it lies in cacheable memory.
    #[inline]
    pub(crate) fn decoded(&mut self, pc: u32) -> Option<crate::code_cache::Entry> {
        if let Some(entry) = self.code.get(pc) {
            return Some(entry);
        }
        let h = self.code_halfword(pc)?;
        let x = self.code_halfword(pc.wrapping_add(2));
        let y = self.code_halfword(pc.wrapping_add(4));
        Some(self.code.fill(pc, h, x, y))
    }

    /// Whether the decoded instruction at `pc` is cached and still current.
    pub(crate) fn is_cached(&self, pc: u32) -> bool {
        self.code.get(pc).is_some()
    }

    /// Drop cached code after NOR contents or XIP configuration changed.
    fn sync_code_cache(&mut self) {
        let generation = self.nor.generation();
        if generation != self.nor_generation {
            self.nor_generation = generation;
            self.code.flush();
        }
    }

    pub fn read(&self, address: u32, size: usize) -> Result<u32, AccessFault> {
        self.read_as(address, size, "read")
    }

    pub fn fetch(&self, address: u32) -> Result<u16, AccessFault> {
        // Startup later copies .ram_text here; instructions can execute from RAM.
        self.read_as(address, 2, "fetch").map(|value| value as u16)
    }

    #[inline]
    pub fn write(&mut self, address: u32, value: u32, size: usize) -> Result<(), AccessFault> {
        Self::check(address, size, "write")?;
        let offset = address.wrapping_sub(RAM) as usize;
        if offset < RAM_SIZE {
            // No device claims SRAM addresses, so of the MMIO chain below
            // only the write-protection check and the store itself apply.
            self.guards
                .check_write(address, size)
                .map_err(|reason| Self::fault(address, size, "write", reason))?;
            match size {
                4 => self.ram[offset..offset + 4].copy_from_slice(&value.to_le_bytes()),
                2 => self.ram[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes()),
                _ => self.ram[offset] = value as u8,
            }
            self.code.invalidate_ram(offset, size);
            return Ok(());
        }
        self.write_slow(address, value, size)
    }

    /// A write outside SRAM. Devices are brought up to the current tick
    /// first, and since a register write can change what the next tick does
    /// (start a transfer, enable a timer, reset a controller), that tick runs
    /// through the exact per-tick path.
    fn write_slow(&mut self, address: u32, value: u32, size: usize) -> Result<(), AccessFault> {
        self.catch_up(self.now)?;
        let result = self.write_mmio(address, value, size);
        self.next_event = self.now + 1;
        result
    }

    /// Advance every clocked device to tick `to` in one step. Only valid
    /// while no device has an event in between (`to < next_event`): each
    /// device's `advance(n)` then equals n calls of `advance(1)`.
    pub(crate) fn catch_up(&mut self, to: u64) -> Result<(), AccessFault> {
        let span = to - self.synced;
        if span == 0 {
            return Ok(());
        }
        self.synced = to;
        let ticks = u32::try_from(span).expect("device catch-up spans are bounded");
        self.devices.advance(ticks);
        self.system
            .advance(ticks)
            .map_err(|reason| Self::fault(0x13e08, 4, "watchdog", reason))?;
        self.advance_usb(ticks)?;
        self.advance_audio(ticks)?;
        self.spi2.advance(ticks);
        Ok(())
    }

    /// Ticks from now until the next device event, capped so that a
    /// catch-up span fits the devices' tick arguments.
    pub(crate) fn ticks_to_event(&self) -> u64 {
        [
            self.devices.ticks_to_event(),
            self.system.ticks_to_event(),
            self.usb.ticks_to_event(),
            self.audio.ticks_to_event(),
            self.spi2.ticks_to_event(),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(u64::MAX)
        .min(1 << 30)
    }

    /// Watchdog ticks since the last feed, as of the current tick.
    pub fn watchdog_ticks(&self) -> u64 {
        let elapsed = self.now - self.synced;
        if self.system.watchdog_timeout().is_some() {
            self.system.watchdog_ticks + elapsed
        } else {
            self.system.watchdog_ticks
        }
    }

    fn write_mmio(&mut self, address: u32, value: u32, size: usize) -> Result<(), AccessFault> {
        self.count_mmio(address, 1);
        if self.clock.write(address, value).is_some() {
            return Ok(());
        }
        if self.crc.write(address, value).is_some() {
            return Ok(());
        }
        if crate::spi2::Spi2::contains(address) {
            if size != 4 {
                return Err(Self::fault(
                    address,
                    size,
                    "write",
                    "SPI registers require word accesses",
                ));
            }
            self.spi2.write(address, value);
            return Ok(());
        }
        if crate::radio::Radio::contains(address) {
            if size != 4 {
                return Err(Self::fault(
                    address,
                    size,
                    "write",
                    "JL_WL stub requires word accesses",
                ));
            }
            self.radio.write(address, value);
            return Ok(());
        }
        if crate::rng::Rng::contains(address & !3) {
            return Err(Self::fault(
                address,
                size,
                "write",
                "JL_RAND registers are read-only",
            ));
        }
        if self.resample.read(address & !3).is_some() {
            if size != 4 {
                return Err(Self::fault(
                    address,
                    size,
                    "write",
                    "JL_SRC registers require word accesses",
                ));
            }
            if let Some(result) = self.resample.write(address, value) {
                return result.map_err(|reason| Self::fault(address, size, "write", reason));
            }
        }
        if let Some(result) = self.audio.write(address, value) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if let Some(result) = self.usb.write(address, value, &mut self.ram) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if address == 0x500c0 {
            self.nor.chip_select(value & 1 == 0);
            self.sync_code_cache();
        }
        if let Some(result) = self.nor.write(address, value) {
            self.sync_code_cache();
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        self.guards
            .check_write(address, size)
            .map_err(|reason| Self::fault(address, size, "write", reason))?;
        if self.cache.write(address, size, value).is_some() {
            return Ok(());
        }
        if let Some(result) = self.guards.write(address, value) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if let Some(result) = self.system.write(address, value) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if self.lcd.read(address & !3).is_some() {
            if size != 4 {
                return Err(Self::fault(
                    address,
                    size,
                    "write",
                    "SPI registers require word accesses",
                ));
            }
            let bytes = if address == SPI + 16 {
                let source = self.lcd.dma_address();
                let offset =
                    Self::offset(source, value as usize, RAM, self.ram.len()).ok_or_else(|| {
                        Self::fault(
                            source,
                            value as usize,
                            "DMA read",
                            "LCD DMA source must be in SRAM",
                        )
                    })?;
                &self.ram[offset..offset + value as usize]
            } else {
                &[]
            };
            let pc_out = self.devices.gpio.read(0x50080).unwrap();
            let pc_dir = self.devices.gpio.read(0x50088).unwrap();
            let selected = pc_dir & 0x180 == 0 && pc_out & 0x80 == 0;
            return self
                .lcd
                .write(address, value, selected, pc_out & 0x100 != 0, bytes)
                .map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if address == crate::adc::CONTROL {
            self.devices.adc.pmu_select = self.system.p33_register(4);
        }
        if let Some(result) = self.devices.write(address, value, size) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        let offset = Self::offset(address, size, RAM, self.ram.len()).ok_or_else(|| {
            Self::fault(
                address,
                size,
                "write",
                "unmapped memory, read-only XIP, or unimplemented MMIO",
            )
        })?;
        self.ram[offset..offset + size].copy_from_slice(&value.to_le_bytes()[..size]);
        self.code.invalidate_ram(offset, size);
        Ok(())
    }

    pub fn advance_usb(&mut self, ticks: u32) -> Result<(), AccessFault> {
        let setups = self.usb.setups;
        self.usb
            .advance(ticks, &mut self.ram)
            .map_err(|reason| Self::fault(0x11800, 4, "USB host", reason))?;
        if self.usb.setups != setups {
            // The host model wrote an 8-byte SETUP packet into SRAM.
            let offset = self.usb.setup_address().wrapping_sub(RAM) as usize;
            self.code.invalidate_ram(offset, 8);
        }
        Ok(())
    }

    pub fn screen_visible(&self) -> bool {
        self.lcd.display_on
            && !self.lcd.sleeping
            && self.devices.gpio.read(0x50000).unwrap() & 4 == 0
            && self.devices.gpio.read(0x50008).unwrap() & 4 == 0
    }

    pub fn advance_audio(&mut self, ticks: u32) -> Result<(), AccessFault> {
        self.audio
            .advance(ticks, &self.ram)
            .map_err(|reason| Self::fault(0x12e1c, 4, "audio DMA", reason))
    }

    pub fn pending_irq(&self, icfg: u32) -> Option<usize> {
        self.pending_irq_for(icfg, 0)
    }
    pub(crate) fn pending_irq_for(&self, icfg: u32, core: usize) -> Option<usize> {
        // With the controller disabled no source has a priority.
        if icfg & 0x100 == 0 {
            return None;
        }
        let timer = self.devices.pending_irq_for(icfg, core);
        let mut best = timer.map(|source| {
            (source, self.devices.irq_priority_for(source, icfg, core).unwrap())
        });
        // Peripheral sources outside devices.rs; a strictly higher priority
        // wins, so the timer keeps ties (unchanged ordering for audio).
        for (source, pending) in [
            (crate::audio::IRQ, self.audio.pending_irq()),
            (crate::spi2::IRQ, self.spi2.pending_irq()),
        ] {
            if !pending {
                continue;
            }
            if let Some(priority) = self.devices.irq_priority_for(source, icfg, core) {
                if best.is_none_or(|(_, highest)| priority > highest) {
                    best = Some((source, priority));
                }
            }
        }
        best.map(|(source, _)| source)
    }
}
