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

#[cfg(test)]
mod dma_tests {
    use super::*;
    #[test]
    fn uart_dma_completion_uses_the_selected_clock_and_source_20() {
        let mut b = Bus::new(vec![0; 8]).unwrap();
        b.write(RAM, 0x007f3c90, 4).unwrap();
        b.write(0x10010, 1 << 10, 4).unwrap(); // PLL48M
        b.write(crate::devices::IRQ_CONFIG + 2 * 4, 3 << 16, 4)
            .unwrap();
        b.write(0x12100, 0x6d, 2).unwrap();
        b.write(0x12108, 383, 2).unwrap();
        b.write(0x12114, RAM, 4).unwrap();
        b.write(0x12118, 3, 2).unwrap();
        b.advance_devices(23039);
        assert_eq!(b.pending_irq(0x100), None);
        b.advance_devices(1);
        assert_eq!(b.pending_irq(0x100), Some(20));
        assert_eq!(b.read(crate::devices::IRQ_PENDING, 4).unwrap(), 1 << 20);
        b.write(0x12100, 0x206d, 2).unwrap();
        assert_eq!(b.pending_irq(0x100), None);
        b.write(0x10010, 0, 4).unwrap(); // OSC24M halves the baud rate.
        b.write(0x12118, 3, 2).unwrap();
        b.advance_devices(46079);
        assert_eq!(b.pending_irq(0x100), None);
        b.advance_devices(1);
        assert_eq!(b.pending_irq(0x100), Some(20));
        assert_eq!(b.read(RAM, 4).unwrap(), 0x007f3c90);
    }
    #[test]
    fn packaged_lcd_dma_honors_decryption_mapping_and_xip_enable() {
        let mut plain = vec![0; 0x140];
        plain[0x121..0x129].copy_from_slice(&[0xf8, 0, 7, 0xe0, 0, 0x1f, 0xff, 0xff]);
        crate::package::sfc(&mut plain, 0x980f);
        let mut raw = vec![255; 0x4140];
        raw[0x4000..].copy_from_slice(&plain);
        // The application view deliberately differs from the real flash;
        // DMA must go through SFC/encryption, not read Bus::flash directly.
        let mut b = Bus::new(vec![0; 8]).unwrap();
        b.load_flash(&raw, 0x980f);
        for (address, value) in [
            (0x51020, 0x10),
            (0x50088, !0x780),
            (SPI, 0x4021),
            (0x50080, 0),
            (SPI + 8, 0x11),
            (SPI + 8, 0x3a),
            (0x50080, 0x100),
            (SPI + 8, 0x55),
            (0x50080, 0),
            (SPI + 8, 0x2c),
            (0x50080, 0x100),
            (SPI + 12, XIP + 1),
            (SPI + 16, 4),
        ] {
            b.write(address, value, 4).unwrap();
        }
        assert_eq!(&b.lcd.pixels[..2], &[0xff0000, 0x00ff00]);
        b.write(0x4020c, 0x4004, 4).unwrap();
        b.write(SPI + 16, 4, 4).unwrap();
        assert_eq!(&b.lcd.pixels[2..4], &[0x0000ff, 0xffffff]);
        b.write(0x40200, 0, 4).unwrap();
        assert!(b.write(SPI + 16, 4, 4).unwrap_err().reason.contains("XIP"));
        assert_eq!(b.lcd.pixels_written, 4);
    }
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
    oscillator_ticks: u64,
    /// The oscillator tick the clocked devices have been advanced to. Between
    /// device events they only count, so they are advanced lazily: in one
    /// span when an event is due, a register is written, or a host asks.
    synced: u64,
    /// The first oscillator tick at which a device may do more than count:
    /// the instruction whose time reaches it advances devices exactly.
    next_event: u64,
    wireless: crate::wireless::Wireless,
    shift_spi: crate::shift_spi::ShiftSpi,
    perf: crate::perf::Perf,
    /// What XIP reads of the application window return, byte by byte,
    /// rebuilt whenever anything they depend on changes (`Nor::xip_key`):
    /// reads stay live, but skip the SFC checks. None: take the full path
    /// (XIP disabled or busy, or an unaligned plain window).
    xip_view: Option<Box<[u8]>>,
    xip_key: Option<crate::nor::XipKey>,
    /// `device_clocks`, recomputed after every register write (the only
    /// way the guest changes a clock selection).
    clocks: (u32, u32, u32),
    /// Diagnostics: MMIO reads and writes by (address, PC), while counting.
    mmio_stats: std::cell::RefCell<Option<MmioStats>>,
    pub(crate) mmio_counting: bool,
    /// PC of the instruction executing, kept while counting MMIO accesses.
    pub(crate) pc_hint: std::cell::Cell<u32>,
    /// No interrupt source was pending at the last `irq_possible` and no
    /// device state has changed since: the bus clears it whenever its own
    /// methods change a device, the CPU at each public entry (callers may
    /// have changed devices through the public fields in between).
    irq_quiet: bool,
}

/// MMIO reads and writes per (address, PC of the accessing instruction).
pub type MmioStats = std::collections::BTreeMap<(u32, u32), [u64; 2]>;

impl Bus {
    /// EMU_CON of `core`: exception enables (bit 2 divide by zero).
    pub(crate) fn emu_con(&self, core: usize) -> u32 {
        self.guards.emu_con(core)
    }
    pub(crate) fn raise_emu_msg(&mut self, core: usize, bits: u32) {
        self.guards.raise_emu_msg(core, bits);
    }
    pub(crate) fn core_control(&self, core: usize) -> u32 {
        self.cache.core_control(core)
    }
    /// Rebuild `xip_view` if what XIP reads depend on changed.
    fn refresh_xip_view(&mut self) {
        let key = self.nor.xip_key();
        if self.xip_key == Some(key) {
            return;
        }
        self.xip_key = Some(key);
        self.xip_view = None;
        if !self.nor.packaged() || !self.nor.plain_window_aligned() {
            return;
        }
        let view: Option<Box<[u8]>> = (0..self.flash.len() as u32)
            .map(|offset| match self.nor.xip(XIP + offset, 1) {
                Some(Ok(byte)) => Some(byte as u8),
                _ => None,
            })
            .collect();
        self.xip_view = view;
    }

    pub(crate) fn load_flash(&mut self, bytes: &[u8], key: u16) {
        self.nor.load(bytes, key);
        self.refresh_xip_view();
        // SPL handoff values measured before peripheral initialization.
        self.usb
            .write(0x10010, 0x10000, &mut self.ram)
            .unwrap()
            .unwrap();
        self.usb
            .write(0x51000, 0xe0c, &mut self.ram)
            .unwrap()
            .unwrap();
        self.audio.write(0x10014, 6).unwrap().unwrap();
        self.clocks = self.select_clocks();
    }
    pub fn new(flash: Vec<u8>) -> Result<Self, String> {
        if flash.is_empty() || flash.len() > (XIP_END - XIP) as usize {
            return Err("application image is empty or exceeds the XIP window".into());
        }
        let mut bus = Self {
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
            oscillator_ticks: 0,
            synced: 0,
            next_event: 0,
            wireless: Default::default(),
            shift_spi: Default::default(),
            perf: Default::default(),
            xip_view: None,
            xip_key: None,
            clocks: (0, 0, 0),
            mmio_stats: Default::default(),
            mmio_counting: false,
            pc_hint: Default::default(),
            irq_quiet: false,
        };
        bus.clocks = bus.select_clocks();
        Ok(bus)
    }

    /// Start counting MMIO reads and writes (diagnostics), from zero.
    pub fn start_mmio_stats(&mut self) {
        *self.mmio_stats.get_mut() = Some(MmioStats::new());
        self.mmio_counting = true;
    }

    /// Stop counting and return the counts, if counting was started.
    pub fn take_mmio_stats(&mut self) -> Option<MmioStats> {
        self.mmio_counting = false;
        self.mmio_stats.get_mut().take()
    }

    #[inline(always)]
    fn count_mmio(&self, address: u32, kind: usize) {
        if self.mmio_counting {
            self.record_mmio(address, kind);
        }
    }

    #[cold]
    fn record_mmio(&self, address: u32, kind: usize) {
        if let Some(stats) = self.mmio_stats.borrow_mut().as_mut() {
            stats.entry((address, self.pc_hint.get())).or_default()[kind] += 1;
        }
    }

    /// Watchdog ticks since the last feed, as of the current tick.
    pub fn watchdog_ticks(&self) -> u64 {
        let elapsed = self.oscillator_ticks - self.synced;
        if self.system.watchdog_timeout().is_some() {
            self.system.watchdog_ticks + elapsed
        } else {
            self.system.watchdog_ticks
        }
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

    /// Little-endian value of `size` (1, 2 or 4) bytes at `offset`; the
    /// caller has checked the range.
    #[inline(always)]
    fn value_at(bytes: &[u8], offset: usize, size: usize) -> u32 {
        match size {
            4 => u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            2 => u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) as u32,
            _ => bytes[offset] as u32,
        }
    }

    /// SRAM and the application XIP view inline (instruction fetches and
    /// most data); everything else in `read_slow`.
    #[inline(always)]
    fn read_as(
        &self,
        address: u32,
        size: usize,
        operation: &'static str,
    ) -> Result<u32, AccessFault> {
        if matches!(size, 1 | 2 | 4) && address.is_multiple_of(size as u32) {
            // Aligned accesses of at most 4 bytes never cross either end.
            let ram_offset = address.wrapping_sub(RAM) as usize;
            if ram_offset < RAM_SIZE {
                return Ok(Self::value_at(&self.ram, ram_offset, size));
            }
            if let Some(view) = &self.xip_view {
                let offset = address.wrapping_sub(XIP) as usize;
                if offset + size <= view.len() {
                    return Ok(Self::value_at(view, offset, size));
                }
            }
        }
        self.read_slow(address, size, operation)
    }

    #[inline(never)]
    fn read_slow(
        &self,
        address: u32,
        size: usize,
        operation: &'static str,
    ) -> Result<u32, AccessFault> {
        Self::check(address, size, operation)?;
        let ram_offset = address.wrapping_sub(RAM) as usize;
        if ram_offset < RAM_SIZE {
            // Aligned accesses of at most 4 bytes never cross the end.
            let ram = &self.ram;
            return Ok(match size {
                4 => u32::from_le_bytes(ram[ram_offset..ram_offset + 4].try_into().unwrap()),
                2 => u16::from_le_bytes(ram[ram_offset..ram_offset + 2].try_into().unwrap()) as u32,
                _ => ram[ram_offset] as u32,
            });
        }
        if let Some(view) = &self.xip_view {
            let offset = address.wrapping_sub(XIP) as usize;
            if offset + size <= view.len() {
                return Ok(match size {
                    4 => u32::from_le_bytes(view[offset..offset + 4].try_into().unwrap()),
                    2 => u16::from_le_bytes(view[offset..offset + 2].try_into().unwrap()) as u32,
                    _ => view[offset] as u32,
                });
            }
        }
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
            if let Some(value) = self.shift_spi.read(address & !3) {
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
            if size == 4 {
                if let Some(value) = self.perf.read(address, self.clock.cycles()) {
                    return Ok(value);
                }
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
            let (peripheral_hz, core_hz, _) = self.device_clocks();
            if let Some(value) = self.devices.read_at(
                address,
                size,
                self.oscillator_ticks - self.synced,
                peripheral_hz,
                core_hz,
            ) {
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

    #[inline]
    pub fn read(&self, address: u32, size: usize) -> Result<u32, AccessFault> {
        self.read_as(address, size, "read")
    }

    #[inline]
    pub fn fetch(&self, address: u32) -> Result<u16, AccessFault> {
        // Startup later copies .ram_text here; instructions can execute from RAM.
        self.read_as(address, 2, "fetch").map(|value| value as u16)
    }

    pub fn write(&mut self, address: u32, value: u32, size: usize) -> Result<(), AccessFault> {
        Self::check(address, size, "write")?;
        if let Some(offset) = Self::offset(address, size, RAM, self.ram.len()) {
            self.guards
                .check_write(address, size)
                .map_err(|reason| Self::fault(address, size, "write", reason))?;
            self.ram[offset..offset + size].copy_from_slice(&value.to_le_bytes()[..size]);
            return Ok(());
        }
        self.sync()?;
        let result = self.write_mmio(address, value, size);
        self.clocks = self.select_clocks();
        self.next_event = self.oscillator_ticks + 1;
        self.irq_quiet = false;
        self.refresh_xip_view();
        result
    }

    fn write_mmio(&mut self, address: u32, value: u32, size: usize) -> Result<(), AccessFault> {
        self.count_mmio(address, 1);
        if let Some(result) = self.devices.write_uart(address, value, size, &self.ram) {
            return result.map_err(|reason| Self::fault(address, size, "write", reason));
        }
        if self.shift_spi.read(address & !3).is_some() {
            if size != 4 {
                return Err(Self::fault(
                    address,
                    size,
                    "write",
                    "SPI registers require word accesses",
                ));
            }
            if self.shift_spi.write(address, value) {
                let single = [value as u8];
                let bytes = if address == crate::shift_spi::BASE + 8 {
                    &single[..]
                } else {
                    let source = self.shift_spi.dma_address();
                    let length = self.shift_spi.dma_length();
                    let offset =
                        Self::offset(source, length, RAM, self.ram.len()).ok_or_else(|| {
                            Self::fault(
                                source,
                                length,
                                "DMA read",
                                "matrix SPI DMA source must be in SRAM",
                            )
                        })?;
                    &self.ram[offset..offset + length]
                };
                let iomap = self.lcd.read(crate::lcd::IOMAP).unwrap();
                self.shift_spi
                    .start(bytes, iomap)
                    .map_err(|reason| Self::fault(address, size, "write", reason))?;
            }
            return Ok(());
        }
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
        if size == 4
            && self
                .perf
                .write(address, value, self.clock.cycles())
                .is_some()
        {
            return Ok(());
        }
        if address == crate::perf::DBG_CON {
            self.perf.control(value, self.clock.cycles());
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
                let length = value as usize;
                if let Some(offset) = Self::offset(source, length, RAM, self.ram.len()) {
                    self.ram[offset..offset + length].to_vec()
                } else if Self::offset(source, length, XIP, self.flash.len()).is_some() {
                    // Stock LCD initialization sends constant data from flash.
                    // Read bytes through XIP so packaged encryption/mapping,
                    // flash modifications and disabled-XIP faults still apply.
                    (0..length)
                        .map(|i| {
                            self.read_as(source + i as u32, 1, "DMA read")
                                .map(|v| v as u8)
                        })
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    return Err(Self::fault(
                        source,
                        length,
                        "DMA read",
                        "LCD DMA source is outside SRAM and application XIP",
                    ));
                }
            } else {
                Vec::new()
            };
            let pc_out = self.devices.gpio.read(0x50080).unwrap();
            let pc_dir = self.devices.gpio.read(0x50088).unwrap();
            let selected = pc_dir & 0x180 == 0 && pc_out & 0x80 == 0;
            return self
                .lcd
                .write(address, value, selected, pc_out & 0x100 != 0, &bytes)
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

    /// The peripheral (LSB), core and UART clocks the guest selected.
    #[inline]
    fn device_clocks(&self) -> (u32, u32, u32) {
        self.clocks
    }

    fn select_clocks(&self) -> (u32, u32, u32) {
        let clk_con3 = self.audio.read(0x10014).unwrap();
        let peripheral_hz = self.clock.timer_hz(clk_con3);
        let core_hz = self.clock.system_hz(clk_con3);
        // spec_uart.c: CLK_CON2[11:10] selects OSC, PLL48M, or LSB.
        let uart_hz = match (self.usb.read(0x10010).unwrap() >> 10) & 3 {
            0 => 24_000_000,
            1 => 48_000_000,
            2 => peripheral_hz,
            _ => 0,
        };
        (peripheral_hz, core_hz, uart_hz)
    }

    fn advance_devices(&mut self, ticks: u32) {
        let (peripheral_hz, core_hz, uart_hz) = self.device_clocks();
        self.devices
            .advance_with_clocks(ticks, peripheral_hz, core_hz);
        self.devices.advance_uart(ticks, uart_hz, &mut self.ram);
        if let Some(bytes) = self.shift_spi.advance(ticks, peripheral_hz) {
            self.devices.gpio.shift_spi(&bytes);
        }
    }

    /// Advance every clocked device by `ticks` oscillator ticks.
    fn advance_clocked(&mut self, ticks: u32) -> Result<(), AccessFault> {
        self.irq_quiet = false;
        self.advance_devices(ticks);
        self.nor.advance(ticks);
        self.refresh_xip_view();
        self.wireless.advance(ticks);
        self.system
            .advance(ticks)
            .map_err(|reason| Self::fault(0x13e08, 4, "watchdog", reason))?;
        self.advance_usb(ticks)?;
        self.advance_audio(ticks)
    }

    /// Bring the clocked devices up to tick `to` in one span. Only valid
    /// while no device event lies in between (`to < next_event`): each
    /// device's `advance(n)` then equals n single ticks.
    fn catch_up(&mut self, to: u64) -> Result<(), AccessFault> {
        let span = to - self.synced;
        if span == 0 {
            return Ok(());
        }
        self.synced = to;
        self.advance_clocked(u32::try_from(span).expect("device spans are bounded"))
    }

    /// Bring the clocked devices up to the current tick (host inspection of
    /// device state; register writes do this themselves).
    pub fn sync(&mut self) -> Result<(), AccessFault> {
        self.catch_up(self.oscillator_ticks)
    }

    /// Ticks from `synced` to the next device event, capped so that a
    /// catch-up span fits the devices' tick arguments.
    fn ticks_to_event(&self) -> u64 {
        let (peripheral_hz, core_hz, uart_hz) = self.device_clocks();
        [
            self.devices.ticks_to_event(peripheral_hz, core_hz, uart_hz),
            self.shift_spi.ticks_to_event(peripheral_hz),
            self.nor.ticks_to_event(),
            self.wireless.ticks_to_event(),
            self.system.ticks_to_event(),
            self.usb.ticks_to_event(),
            self.audio.ticks_to_event(),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(u64::MAX)
        .min(1 << 30)
    }

    /// Device time after an instruction that took `ticks` oscillator ticks.
    /// Until the next device event devices only count, so nothing runs; the
    /// instruction that reaches the event brings them up to date and then
    /// advances its own ticks exactly, as every instruction once did.
    #[inline(always)]
    pub(crate) fn advance_time(&mut self, ticks: u32) -> Result<(), AccessFault> {
        if self.oscillator_ticks < self.next_event {
            return Ok(());
        }
        self.advance_event(ticks)
    }

    #[inline(never)]
    fn advance_event(&mut self, ticks: u32) -> Result<(), AccessFault> {
        let now = self.oscillator_ticks;
        self.catch_up(now - ticks as u64)?;
        self.synced = now;
        // Should this tick fault, the next one runs exactly too.
        self.next_event = now + 1;
        self.advance_clocked(ticks)?;
        self.next_event = now + self.ticks_to_event();
        Ok(())
    }

    /// Queue terminal bytes for the guest's CDC OUT endpoint (see
    /// `Usb::receive_serial`); the next tick delivers them.
    pub fn receive_serial(&mut self, bytes: &[u8]) -> bool {
        self.irq_quiet = false;
        self.next_event = self.next_event.min(self.oscillator_ticks + 1);
        self.usb.receive_serial(bytes)
    }
    /// Issue one instruction per `hz` of guest time instead of following the
    /// firmware's system clock; `None` restores the firmware clock. Timers,
    /// DMA, USB and the watchdog keep their own clocks, so a lower rate gives
    /// the guest fewer instructions per second of guest time.
    pub fn set_instruction_clock(&mut self, hz: Option<u32>) {
        self.clock.issue_override = hz;
    }

    pub(crate) fn instruction_ticks(&mut self) -> u32 {
        let ticks = self
            .clock
            .instruction_ticks(self.audio.read(0x10014).unwrap());
        self.oscillator_ticks += ticks as u64;
        ticks
    }

    /// The most instructions (of the core carrying guest time) that can
    /// issue before the next device event; the instruction reaching the
    /// event is left to the normal path.
    pub(crate) fn issues_before_event(&mut self) -> u64 {
        let Some(room) = self.next_event.checked_sub(self.oscillator_ticks + 1) else {
            return 0;
        };
        let clk_con3 = self.audio.read(0x10014).unwrap();
        self.clock.issues_within(clk_con3, room)
    }

    /// Account `count` instructions in which nothing but time passes, all
    /// before the next device event (`issues_before_event`).
    pub(crate) fn skip_issues(&mut self, count: u64) {
        let clk_con3 = self.audio.read(0x10014).unwrap();
        self.oscillator_ticks += self.clock.issue(clk_con3, count);
        debug_assert!(self.oscillator_ticks < self.next_event);
    }

    /// Device state may have changed other than through the bus's methods
    /// (the public device fields): `irq_possible` checks again.
    pub(crate) fn devices_changed(&mut self) {
        self.irq_quiet = false;
    }

    /// `any_irq_pending`, without checking again while no device state
    /// changed since a check found nothing pending.
    #[inline(always)]
    pub(crate) fn irq_possible(&mut self) -> bool {
        !self.irq_quiet && self.irq_check()
    }

    #[inline(never)]
    fn irq_check(&mut self) -> bool {
        let pending = self.any_irq_pending();
        self.irq_quiet = !pending;
        pending
    }

    /// Whether any interrupt source is pending for either core.
    #[inline]
    pub(crate) fn any_irq_pending(&self) -> bool {
        self.devices.pending_sources(0) != 0
            || self.devices.pending_sources(1) != 0
            || self.lcd.pending_irq()
            || self.audio.pending_irq()
            || self.shift_spi.pending_irq()
            || self.wireless.clock_pending_irq()
            || self.wireless.slot_pending_irq()
    }

    /// 24 MHz oscillator ticks of guest time so far.
    pub fn oscillator_ticks(&self) -> u64 {
        self.oscillator_ticks
    }

    /// Queue USB-MIDI packets for the firmware (host to device). They are
    /// delivered once the MIDI host has configured the device, one bulk
    /// packet (up to 16 events) whenever its OUT endpoint buffer is free.
    pub fn usb_midi_send(&mut self, packets: &[crate::usb_midi::Packet]) {
        self.irq_quiet = false;
        self.next_event = self.next_event.min(self.oscillator_ticks + 1);
        self.usb.midi_send(packets);
    }

    /// Put MIDI bytes on the UART1 RX line (the FM-1's DIN/TRS MIDI IN):
    /// they arrive back to back at 31250 baud.
    pub fn uart_midi_send(&mut self, bytes: &[u8]) {
        self.irq_quiet = false;
        self.next_event = self.next_event.min(self.oscillator_ticks + 1);
        self.devices.uart_mut().receive(bytes);
    }

    /// UART1 receive line: bytes still waiting, received, lost.
    pub fn uart_rx_counts(&self) -> (usize, u64, u64) {
        self.devices.uart().rx_counts()
    }

    /// Bytes UART1 finished transmitting (MIDI OUT; the first 4096).
    pub fn uart_transmitted(&self) -> &[u8] {
        self.devices.uart().transmitted()
    }

    pub fn advance_usb(&mut self, ticks: u32) -> Result<(), AccessFault> {
        self.irq_quiet = false;
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
        self.irq_quiet = false;
        self.audio
            .advance(ticks, &self.ram)
            .map_err(|reason| Self::fault(0x12e1c, 4, "audio DMA", reason))
    }

    pub fn pending_irq(&self, icfg: u32) -> Option<usize> {
        self.pending_irq_for(icfg, 0)
    }
    pub(crate) fn pending_irq_for(&self, icfg: u32, core: usize) -> Option<usize> {
        let sources = self.devices.pending_sources(core)
            | ((self.lcd.pending_irq() as u128) << crate::lcd::IRQ)
            | ((self.audio.pending_irq() as u128) << crate::audio::IRQ)
            | ((self.shift_spi.pending_irq() as u128) << crate::shift_spi::IRQ)
            | ((self.wireless.clock_pending_irq() as u128) << 40)
            | ((self.wireless.slot_pending_irq() as u128) << 41);
        self.devices.select_irq(sources, icfg, core)
    }
}
