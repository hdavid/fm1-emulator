// SPDX-License-Identifier: GPL-3.0-only
//! Flash that survives a power cycle. A real FM-1 keeps whatever its firmware
//! wrote to the serial NOR (projects, autosave, presets, settings); the
//! emulator reloads the package at every start, so without this the writes
//! of a session are gone at the next one.
//!
//! The state of one firmware family (`family`) is two files in a folder:
//! `FAMILY.nor`, a 1 MiB NOR image holding the sectors the firmware erased or
//! programmed (every other sector reads erased, 0xff), and `FAMILY.index`, a
//! short text file naming those sectors. At start the saved sectors are laid
//! over the freshly loaded package (`restore`), except where they overlap the
//! package's own boot and application area: a new build of the same firmware
//! gets its new code and keeps its data, like an update on the device.
//!
//! Writes are atomic (temporary file, then rename). The image is written
//! before the index, and the sectors a run tracks only grow, so an index left
//! from an interrupted save names sectors the newer image holds.
use crate::{bus::Bus, nor::SECTOR};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Size of the FM-1's NOR (P25Q80H, 8 Mbit).
pub const NOR_SIZE: usize = 1024 * 1024;
const FORMAT: &str = "fm1-emu flash state 1";

/// The firmware family a package file belongs to: its file name, lowercase,
/// up to the first dash or underscore that starts a version number.
/// `optimist-0.1-dev-5379036.fwsc` and `optimist-0.2.fwsc` are both
/// `optimist`, `felucca-1.0.3.fwsc` is `felucca`, `BaudGirl-FM-1_096.fwsc`
/// is `baudgirl-fm`. Characters outside `[a-z0-9_-]` become `_`.
pub fn family(firmware: &Path) -> String {
    let stem = firmware
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let tokens: Vec<&str> = stem.split(['-', '_']).collect();
    let versioned = |token: &str| {
        let token = token.strip_prefix('v').unwrap_or(token);
        token.starts_with(|c: char| c.is_ascii_digit())
    };
    let kept = tokens
        .iter()
        .take_while(|token| !versioned(token))
        .copied()
        .collect::<Vec<_>>()
        .join("-");
    let name = if kept.is_empty() { stem } else { kept };
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "firmware".into()
    } else {
        clean
    }
}

/// Default folder for flash states: `state/` in the emulator's directory
/// (the crate directory for a binary under `target/`, else the binary's).
pub fn default_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let home = dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "target"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or(dir);
    home.join("state")
}

/// Sectors read from a state file: the image and which sectors of it count.
#[derive(Debug, PartialEq)]
pub struct Saved {
    pub image: Vec<u8>,
    pub sectors: Vec<usize>,
}

/// What `restore` did.
#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    /// Sectors laid over the package.
    pub applied: Vec<usize>,
    /// Saved sectors overlapping the package's code area: the package won.
    pub kept_package: Vec<usize>,
}

/// One family's state files.
#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    pub image: PathBuf,
    pub index: PathBuf,
}

impl Store {
    /// The store of `firmware`'s family. `state`: a file to use (its index
    /// is beside it, `.index`), or a folder (existing, or written with a
    /// trailing slash) to hold `FAMILY.nor`; None: `default_dir()`.
    pub fn for_firmware(state: Option<&Path>, firmware: &Path) -> Self {
        let name = format!("{}.nor", family(firmware));
        let image = match state {
            None => default_dir().join(name),
            Some(path) if path.is_dir() || path.to_string_lossy().ends_with(['/', '\\']) => {
                path.join(name)
            }
            Some(path) => path.to_path_buf(),
        };
        Self::at(image)
    }
    pub fn at(image: PathBuf) -> Self {
        let index = image.with_extension("index");
        Self { image, index }
    }

    /// The saved state; None when there is none.
    pub fn load(&self) -> Result<Option<Saved>, String> {
        if !self.index.exists() && !self.image.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&self.index)
            .map_err(|error| format!("{}: {error}", self.index.display()))?;
        let image =
            fs::read(&self.image).map_err(|error| format!("{}: {error}", self.image.display()))?;
        if image.len() != NOR_SIZE {
            return Err(format!(
                "{}: {} bytes, expected {NOR_SIZE}",
                self.image.display(),
                image.len()
            ));
        }
        let mut lines = text.lines().filter(|line| !line.starts_with('#'));
        if lines.next() != Some(FORMAT) {
            return Err(format!(
                "{}: not a flash state index ({FORMAT})",
                self.index.display()
            ));
        }
        let mut sectors = Vec::new();
        for line in lines {
            let Some(list) = line.strip_prefix("sectors") else {
                continue;
            };
            for word in list.split_whitespace() {
                let offset = word
                    .strip_prefix("0x")
                    .and_then(|hex| usize::from_str_radix(hex, 16).ok())
                    .filter(|offset| offset.is_multiple_of(SECTOR) && *offset < NOR_SIZE)
                    .ok_or_else(|| format!("{}: bad sector {word}", self.index.display()))?;
                sectors.push(offset);
            }
        }
        sectors.sort_unstable();
        sectors.dedup();
        Ok(Some(Saved { image, sectors }))
    }

    /// Save `sectors` of `nor` (every other sector is written erased).
    pub fn save(&self, nor: &[u8], sectors: &[usize], firmware: &str) -> Result<(), String> {
        let mut image = vec![0xff; NOR_SIZE];
        for &offset in sectors {
            let end = (offset + SECTOR).min(nor.len()).min(NOR_SIZE);
            if offset < end {
                image[offset..end].copy_from_slice(&nor[offset..end]);
            }
        }
        let list: Vec<String> = sectors
            .iter()
            .map(|offset| format!("{offset:#07x}"))
            .collect();
        let index = format!(
            "{FORMAT}\n# The 4 KiB sectors of {} the firmware erased or programmed, laid over the\n# package at the next start (fm1-ui --fresh, or Reset flash state, starts clean).\nfirmware {firmware}\nsectors {}\n",
            self.image.file_name().unwrap_or_default().to_string_lossy(),
            list.join(" ")
        );
        if let Some(dir) = self
            .image
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
        {
            fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        }
        write_atomic(&self.image, &image)?;
        write_atomic(&self.index, index.as_bytes())
    }

    /// Forget the state (both files); fine when there is none.
    pub fn remove(&self) -> Result<(), String> {
        for path in [&self.index, &self.image] {
            match fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("{}: {error}", path.display()))
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Write `bytes` to a temporary file beside `path`, then rename it over
/// `path`: a reader sees the old file or the new one, never a part.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let result = fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&temporary, path));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{}: {error}", path.display()));
    }
    Ok(())
}

/// Lay `saved` over a freshly loaded flash. Sectors starting below
/// `code_end` (the package's boot and application area) keep the package.
pub fn restore(bus: &mut Bus, saved: &Saved, code_end: usize) -> Result<Restored, String> {
    let mut restored = Restored::default();
    for &offset in &saved.sectors {
        if offset < code_end {
            restored.kept_package.push(offset);
            continue;
        }
        bus.restore_nor_sector(offset, &saved.image[offset..offset + SECTOR])?;
        restored.applied.push(offset);
    }
    Ok(restored)
}

/// One running machine's state: restores it at start, then saves it a
/// moment after the firmware stops writing, and when asked (quit).
pub struct Persistence {
    pub store: Store,
    firmware: String,
    /// `Bus::nor_writes` when the state was last in step with the flash.
    saved_writes: u64,
    /// Host time of the last flash activity not yet saved.
    dirty_since: Option<(u64, Instant)>,
    debounce: Duration,
}

/// Quiet time after the last erase or program before the state is saved.
pub const DEBOUNCE: Duration = Duration::from_secs(1);

impl Persistence {
    /// Start `bus` (just loaded from `firmware`) from the saved state, or
    /// from the package alone when `fresh` (the old state is then removed).
    /// Returns the persistence and a line saying what happened.
    pub fn attach(
        store: Store,
        bus: &mut Bus,
        firmware: &Path,
        code_end: usize,
        fresh: bool,
    ) -> (Self, String) {
        let name = firmware
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let shown = store.image.display().to_string();
        let message = if fresh {
            match store.remove() {
                Ok(()) => {
                    format!("flash state: fresh start, {shown} replaced on the first flash write")
                }
                Err(error) => format!("flash state: could not remove the old state: {error}"),
            }
        } else {
            match store.load() {
                Ok(None) => {
                    format!("flash state: none yet, {shown} written on the first flash write")
                }
                Ok(Some(saved)) => match restore(bus, &saved, code_end) {
                    Ok(restored) => {
                        let mut line = format!(
                            "flash state: restored {} sectors from {shown}",
                            restored.applied.len()
                        );
                        if !restored.kept_package.is_empty() {
                            let list: Vec<String> = restored
                                .kept_package
                                .iter()
                                .map(|o| format!("{o:#x}"))
                                .collect();
                            line += &format!(
                                "; {} saved sectors overlap {name}'s code area (below {code_end:#x}): the package wins ({})",
                                list.len(),
                                list.join(" ")
                            );
                        }
                        line
                    }
                    Err(error) => format!("flash state: not restored: {error}"),
                },
                Err(error) => format!(
                    "flash state: ignored ({error}); it is replaced on the first flash write"
                ),
            }
        };
        let persistence = Self {
            store,
            firmware: name,
            saved_writes: bus.nor_writes(),
            dirty_since: None,
            debounce: DEBOUNCE,
        };
        (persistence, message)
    }

    /// Whether the flash changed since the state was last saved.
    pub fn dirty(&self, bus: &Bus) -> bool {
        bus.nor_writes() != self.saved_writes
    }

    /// Call often: saves `DEBOUNCE` after the last flash write.
    /// Some(result) when it saved.
    pub fn poll(&mut self, bus: &Bus, now: Instant) -> Option<Result<(), String>> {
        let writes = bus.nor_writes();
        if writes == self.saved_writes {
            self.dirty_since = None;
            return None;
        }
        match self.dirty_since {
            Some((seen, _)) if seen != writes => self.dirty_since = Some((writes, now)),
            None => self.dirty_since = Some((writes, now)),
            Some((_, since)) if now.duration_since(since) >= self.debounce => {
                return Some(self.save(bus))
            }
            Some(_) => {}
        }
        None
    }

    /// Save now when the flash changed. Some(result) when it saved.
    pub fn flush(&mut self, bus: &Bus) -> Option<Result<(), String>> {
        self.dirty(bus).then(|| self.save(bus))
    }

    fn save(&mut self, bus: &Bus) -> Result<(), String> {
        let writes = bus.nor_writes();
        self.store
            .save(bus.nor_bytes(), &bus.nor_tracked_sectors(), &self.firmware)?;
        self.saved_writes = writes;
        self.dirty_since = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fm1-flash-state-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn family_is_the_name_before_its_version() {
        for (file, expected) in [
            ("optimist-0.1-dev-5379036.fwsc", "optimist"),
            ("build/optimist-0.2.fwsc", "optimist"),
            ("felucca-1.0.3.fwsc", "felucca"),
            ("felucca.fwsc", "felucca"),
            ("sloop-2.3.fwsc", "sloop"),
            ("x0x-0.10.1-beta.fwsc", "x0x"),
            ("melodee-v0.10.fwsc", "melodee"),
            ("BaudGirl-FM-1_096.fwsc", "baudgirl-fm"),
            ("FM-1_093.fwsc", "fm"),
            ("1.2.fwsc", "1_2"),
            ("my firmware.elf", "my_firmware"),
        ] {
            assert_eq!(family(Path::new(file)), expected, "{file}");
        }
    }

    #[test]
    fn a_state_folder_or_file_can_be_given() {
        let dir = scratch("paths");
        let fw = Path::new("optimist-0.1.fwsc");
        assert_eq!(
            Store::for_firmware(Some(&dir), fw).image,
            dir.join("optimist.nor")
        );
        assert_eq!(
            Store::for_firmware(Some(Path::new("new/")), fw).image,
            Path::new("new/optimist.nor")
        );
        let file = Store::for_firmware(Some(&dir.join("mine.nor")), fw);
        assert_eq!(file.index, dir.join("mine.index"));
        assert!(Store::for_firmware(None, fw)
            .image
            .ends_with("state/optimist.nor"));
    }

    #[test]
    fn saved_sectors_round_trip_and_the_rest_reads_erased() {
        let dir = scratch("round-trip");
        let store = Store::at(dir.join("sub").join("x.nor"));
        assert_eq!(store.load(), Ok(None));
        let mut nor = vec![0x11; NOR_SIZE];
        nor[0x9f000] = 0x22;
        nor[0xfefff] = 0x33;
        store.save(&nor, &[0x9f000, 0xfe000], "x-1.fwsc").unwrap();
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.sectors, vec![0x9f000, 0xfe000]);
        assert_eq!(saved.image[0x9f000..0xa0000], nor[0x9f000..0xa0000]);
        assert_eq!(saved.image[0xfe000..0xff000], nor[0xfe000..0xff000]);
        assert!(saved.image[..0x9f000].iter().all(|&b| b == 0xff));
        assert!(saved.image[0xff000..].iter().all(|&b| b == 0xff));
        let leftovers: Vec<_> = fs::read_dir(dir.join("sub")).unwrap().collect();
        assert_eq!(leftovers.len(), 2, "no temporary files left behind");
        store.remove().unwrap();
        assert_eq!(store.load(), Ok(None));
        store.remove().unwrap();
    }

    #[test]
    fn a_damaged_state_is_reported_not_used() {
        let dir = scratch("damaged");
        let store = Store::at(dir.join("x.nor"));
        store.save(&vec![0; NOR_SIZE], &[0x9f000], "x").unwrap();
        fs::write(&store.image, [0; 10]).unwrap();
        assert!(store.load().unwrap_err().contains("expected"));
        store.save(&vec![0; NOR_SIZE], &[0x9f000], "x").unwrap();
        fs::write(&store.index, "fm1-emu flash state 1\nsectors 0x9f001\n").unwrap();
        assert!(store.load().unwrap_err().contains("bad sector"));
        fs::write(&store.index, "something else\n").unwrap();
        assert!(store.load().unwrap_err().contains("not a flash state"));
        fs::remove_file(&store.index).unwrap();
        assert!(store.load().is_err(), "an image without its index");
    }

    /// A package-like bus: 0x55 everywhere up to 0x94000, encrypted XIP on.
    fn bus() -> Bus {
        let mut bus = Bus::new(vec![0; 8]).unwrap();
        bus.load_flash(&vec![0x55; 0x94000], 0x980f);
        bus
    }

    #[test]
    fn saved_sectors_overlay_the_package_except_its_code_area() {
        let mut image = vec![0xff; NOR_SIZE];
        image[0x92000..0x93000].fill(0x01); // in the code area (below 0x9225b)
        image[0x93000..0x94000].fill(0x02); // data above the code
        image[0xfe000..0xff000].fill(0x03);
        let saved = Saved {
            image,
            sectors: vec![0x92000, 0x93000, 0xfe000],
        };
        let mut bus = bus();
        let restored = restore(&mut bus, &saved, 0x9225b).unwrap();
        assert_eq!(restored.applied, vec![0x93000, 0xfe000]);
        assert_eq!(restored.kept_package, vec![0x92000]);
        assert!(bus.nor_bytes()[0x92000..0x93000].iter().all(|&b| b == 0x55));
        assert!(bus.nor_bytes()[0x93000..0x94000].iter().all(|&b| b == 0x02));
        assert!(bus.nor_bytes()[0xfe000..0xff000].iter().all(|&b| b == 0x03));
        assert_eq!(bus.nor_tracked_sectors(), vec![0x93000, 0xfe000]);
        assert_eq!(bus.nor_writes(), 0);
        // The XIP view decrypts the restored bytes: the plain value back.
        let mut block = [0x02; 32];
        crate::package::enc(&mut block, 0x980f ^ ((0x93000 - 0x4000) / 4) as u16);
        let raw = bus.read(0x0208f000, 4).unwrap();
        assert_eq!(raw, u32::from_le_bytes(block[..4].try_into().unwrap()));
    }

    #[test]
    fn persistence_saves_after_the_debounce_and_restores_at_the_next_start() {
        let dir = scratch("persist");
        let store = Store::at(dir.join("optimist.nor"));
        let fw = Path::new("optimist-0.1.fwsc");
        let mut bus = bus();
        let (mut persistence, message) =
            Persistence::attach(store.clone(), &mut bus, fw, 0x9225b, false);
        assert!(message.contains("none yet"), "{message}");
        let start = Instant::now();
        assert!(persistence.poll(&bus, start).is_none());
        // The guest erases 0x9f000 and programs a byte there (SPI0, chip
        // select on 0x500c0 bit 0, low = selected).
        let spi = |bus: &mut Bus, bytes: &[u32]| {
            bus.write(0x500c0, 0, 4).unwrap();
            for &byte in bytes {
                bus.write(0x11c08, byte, 4).unwrap();
            }
            bus.write(0x500c0, 1, 4).unwrap();
        };
        for (command, ticks) in [
            (&[0x20, 9, 0xf0, 0][..], 192_000),
            (&[2, 9, 0xf0, 0, 0x42], 48_000),
        ] {
            spi(&mut bus, &[6]);
            spi(&mut bus, command);
            bus.advance_nor(ticks);
        }
        assert_eq!(bus.nor_writes(), 2);
        assert_eq!(bus.nor_tracked_sectors(), vec![0x9f000]);
        assert!(persistence.poll(&bus, start).is_none(), "debounce starts");
        assert!(persistence.poll(&bus, start + DEBOUNCE / 2).is_none());
        assert_eq!(persistence.poll(&bus, start + DEBOUNCE), Some(Ok(())));
        assert!(persistence.flush(&bus).is_none(), "nothing new to save");
        // Next start: the package, then the saved sector over it.
        let mut next = self::bus();
        let (_, message) = Persistence::attach(store.clone(), &mut next, fw, 0x9225b, false);
        assert!(message.contains("restored 1 sectors"), "{message}");
        assert_eq!(next.nor_bytes()[0x9f000], 0x42);
        assert_eq!(next.nor_bytes()[0x9f001], 0xff);
        // --fresh: the package alone, and the state is gone.
        let mut fresh = self::bus();
        let (_, message) = Persistence::attach(store.clone(), &mut fresh, fw, 0x9225b, true);
        assert!(message.contains("fresh start"), "{message}");
        assert_eq!(fresh.nor_bytes()[0x9f000], 0xff);
        assert_eq!(store.load(), Ok(None));
    }
}
