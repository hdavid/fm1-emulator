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
    pub flash: Vec<u8>,
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
    wireless: crate::wireless::Wireless,
}

impl Bus {
    pub(crate) fn load_flash(&mut self, bytes: &[u8], key: u16) {
        self.nor.load(bytes, key);
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
            wireless: Default::default(),
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

    fn read_as(
        &self,
        address: u32,
        size: usize,
        operation: &'static str,
    ) -> Result<u32, AccessFault> {
        Self::check(address, size, operation)?;
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
            if let Some(result) = self.wireless.read(address, size) {
                return result.map_err(|reason| Self::fault(address, size, operation, reason));
            }
            if let Some(value) = self.clock.read(address) {
                return Ok(value);
            }
            if let Some(value) = self.crc.read(address) {
                return Ok(value);
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
            if let Some(value) = self.devices.read(address, size) {
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

    pub fn read(&self, address: u32, size: usize) -> Result<u32, AccessFault> {
        self.read_as(address, size, "read")
    }

    pub fn fetch(&self, address: u32) -> Result<u16, AccessFault> {
        // Startup later copies .ram_text here; instructions can execute from RAM.
        self.read_as(address, 2, "fetch").map(|value| value as u16)
    }

    pub fn write(&mut self, address: u32, value: u32, size: usize) -> Result<(), AccessFault> {
        Self::check(address, size, "write")?;
        if let Some(result) = self.wireless.write(address, value, size) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if self.clock.write(address, value).is_some() {
            return Ok(());
        }
        if self.crc.write(address, value).is_some() {
            return Ok(());
        }
        if let Some(result) = self.audio.write(address, value) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if let Some(result) = self.usb.write(address, value, &mut self.ram) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if address == 0x500c0 {
            self.nor.chip_select(value & 1 == 0);
        }
        if let Some(result) = self.nor.write(address, value) {
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
            self.devices.adc.select_pmu(self.system.adc_pmu_selection());
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
        Ok(())
    }

    pub(crate) fn advance_nor(&mut self, ticks: u32) {
        self.nor.advance(ticks);
    }

    pub(crate) fn advance_wireless(&mut self, ticks: u32) {
        self.wireless.advance(ticks);
    }
    pub(crate) fn advance_devices(&mut self, ticks: u32) {
        let clk_con3 = self.audio.read(0x10014).unwrap();
        self.devices
            .advance_with_timer_clock(ticks, self.clock.timer_hz(clk_con3));
    }

    pub fn advance_usb(&mut self, ticks: u32) -> Result<(), AccessFault> {
        self.usb
            .advance(ticks, &mut self.ram)
            .map_err(|reason| Self::fault(0x11800, 4, "USB host", reason))
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
        let timer = self.devices.pending_irq_for(icfg, core);
        let audio_priority = self.devices.irq_priority_for(crate::audio::IRQ, icfg, core);
        if self.audio.pending_irq() {
            if let Some(priority) = audio_priority {
                if timer.is_none_or(|source| {
                    priority > self.devices.irq_priority_for(source, icfg, core).unwrap()
                }) {
                    return Some(crate::audio::IRQ);
                }
            }
        }
        timer
    }
}
