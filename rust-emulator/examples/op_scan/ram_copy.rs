// SPDX-License-Identifier: GPL-3.0-only
//! RAM code in symbol-less images. Startup copies part of the image to RAM
//! and runs it there, so the listing (image addresses) and the branch targets
//! and code pointers (RAM addresses) disagree. Booting the firmware for a
//! while and matching RAM against the image finds those copies; the scan then
//! translates addresses between the two.
use fm1_emu::{cpu::Cpu, firmware::Firmware, RAM, RAM_SIZE, XIP};
use std::collections::HashMap;

/// Instructions to boot before reading RAM (startup copies within the first
/// few hundred thousand on every firmware here).
pub const BOOT_STEPS: u64 = 5_000_000;
/// Window matched between RAM and the image.
const BLOCK: usize = 32;
/// Shortest run reported as a copy.
const MIN_COPY: usize = 64;

/// `len` bytes the firmware copied from image address `lma` to RAM `vma`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Copy {
    pub vma: u32,
    pub lma: u32,
    pub len: u32,
}

#[derive(Default)]
pub struct Copies(Vec<Copy>);

impl Copies {
    /// Boot `firmware` for `steps` instructions (a fault ends the boot
    /// early) and find the RAM runs that equal the image.
    pub fn find(firmware: &Firmware, steps: u64) -> Result<Self, String> {
        let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
        // As examples/diagnose: the loader leaves r0 pointing at its
        // parameter block.
        cpu.r[0] = 0x01c7_fe08;
        let _ = cpu.run_steps(steps);
        let ram: Vec<u8> = (0..(RAM_SIZE / 4) as u32)
            .flat_map(|i| cpu.bus.read(RAM + 4 * i, 4).unwrap_or(0).to_le_bytes())
            .collect();
        Ok(Self(matches(&ram, &firmware.image)))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Copy> {
        self.0.iter()
    }

    /// Image address of RAM address `vma`, when it lies in a copy.
    pub fn to_image(&self, vma: u32) -> Option<u32> {
        self.0
            .iter()
            .find(|c| vma.wrapping_sub(c.vma) < c.len)
            .map(|c| c.lma + (vma - c.vma))
    }

    /// RAM address the image address `lma` runs at, when it lies in a copy.
    pub fn to_ram(&self, lma: u32) -> Option<u32> {
        self.0
            .iter()
            .find(|c| lma.wrapping_sub(c.lma) < c.len)
            .map(|c| c.vma + (lma - c.lma))
    }

    /// Where a branch of the instruction at image address `at` really goes,
    /// as an image address: `target` is what objdump computed as if the
    /// code ran where the listing has it.
    pub fn branch(&self, at: u32, target: u32) -> u32 {
        match self.to_ram(at) {
            // Code in a copy: relative to where it runs.
            Some(ram) => {
                let actual = target.wrapping_add(ram.wrapping_sub(at));
                self.to_image(actual).unwrap_or(actual)
            }
            None => self.to_image(target).unwrap_or(target),
        }
    }
}

/// Windows of the image worth matching: varied enough not to be padding.
fn varied(window: &[u8]) -> bool {
    let mut seen = [false; 256];
    window.iter().for_each(|&b| seen[b as usize] = true);
    seen.iter().filter(|&&s| s).count() >= 8
}

/// Maximal RAM runs equal to the image, found from unique `BLOCK`-byte
/// windows (a window the image holds twice is skipped, it cannot place a
/// copy).
fn matches(ram: &[u8], image: &[u8]) -> Vec<Copy> {
    let mut index: HashMap<&[u8], Option<usize>> = HashMap::new();
    for at in (0..image.len().saturating_sub(BLOCK)).step_by(2) {
        let window = &image[at..at + BLOCK];
        if varied(window) {
            index
                .entry(window)
                .and_modify(|e| *e = None)
                .or_insert(Some(at));
        }
    }
    let mut copies = Vec::new();
    let mut at = 0;
    while at + BLOCK <= ram.len() {
        let window = &ram[at..at + BLOCK];
        let Some(Some(lma)) = varied(window).then(|| index.get(window)).flatten() else {
            at += BLOCK;
            continue;
        };
        let (mut start, mut from) = (at, *lma);
        let floor = copies
            .last()
            .map_or(0, |c: &Copy| (c.vma - RAM + c.len) as usize);
        while start > floor && from > 0 && ram[start - 1] == image[from - 1] {
            start -= 1;
            from -= 1;
        }
        let mut end = at + BLOCK;
        while end < ram.len()
            && from + (end - start) < image.len()
            && ram[end] == image[from + (end - start)]
        {
            end += 1;
        }
        if end - start >= MIN_COPY {
            copies.push(Copy {
                vma: RAM + start as u32,
                lma: XIP + from as u32,
                len: (end - start) as u32,
            });
        }
        at = end.next_multiple_of(BLOCK);
    }
    copies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_copied_run_and_translates_branches() {
        // A pseudo-random image: no 32-byte window occurs twice.
        let image: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let mut ram = vec![0u8; 8192];
        ram[1000..1600].copy_from_slice(&image[2000..2600]);
        let copies = Copies(matches(&ram, &image));
        assert_eq!(
            copies.iter().copied().collect::<Vec<_>>(),
            [Copy {
                vma: RAM + 1000,
                lma: XIP + 2000,
                len: 600
            }]
        );
        assert_eq!(copies.to_image(RAM + 1010), Some(XIP + 2010));
        assert_eq!(copies.to_ram(XIP + 2010), Some(RAM + 1010));
        // Flash code calling into the copy.
        assert_eq!(copies.branch(XIP, RAM + 1100), XIP + 2100);
        // Code in the copy calling flash: objdump computed the target from
        // the image address; the real one is relative to the RAM address.
        let at = XIP + 2100;
        let flash = XIP + 40;
        let as_listed = flash.wrapping_sub(RAM + 1100).wrapping_add(at);
        assert_eq!(copies.branch(at, as_listed), flash);
        // And within the copy.
        assert_eq!(copies.branch(at, XIP + 2200), XIP + 2200);
    }
}
