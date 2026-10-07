// SPDX-License-Identifier: GPL-3.0-only
// The window's own settings, kept between runs of fm1-ui: for now its size.
// They live in a short text file (`fm1-ui.toml`, a few `key = value` lines)
// in the emulator's directory, written a second after the last change and
// when the window closes. Headless tools never read or write it.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The settings file's name.
pub(super) const FILE_NAME: &str = "fm1-ui.toml";
/// Quiet time after a change before the file is written: a resize is saved
/// once, when it settles.
pub(super) const DEBOUNCE: Duration = Duration::from_secs(1);
/// Window sizes outside this range (points) are not restored.
const WINDOW_MIN: [u32; 2] = [800, 600];
const WINDOW_MAX: u32 = 16_384;

/// What the window remembers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Settings {
    /// The window's inner size in points; None: the default.
    pub window: Option<[u32; 2]>,
}

/// The settings file in the emulator's directory (the crate directory for a
/// binary under `target/`, else the binary's).
pub(super) fn default_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    dir.ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "target"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or(dir)
        .join(FILE_NAME)
}

/// `text` read over `defaults`, with a message per line it could not use.
/// Unknown keys are skipped silently, so an older build reads a newer file.
pub(super) fn parse(text: &str, defaults: Settings) -> (Settings, Vec<String>) {
    let mut settings = defaults;
    let mut problems = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            problems.push(format!("line {}: expected key = value", number + 1));
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let applied = match key {
            "window" => parse_window(value).map(|window| settings.window = Some(window)),
            _ => Some(()),
        };
        if applied.is_none() {
            problems.push(format!("line {}: bad value for {key}: {value}", number + 1));
        }
    }
    (settings, problems)
}

fn parse_window(value: &str) -> Option<[u32; 2]> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let (width, height) = inner.split_once(',')?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height.trim().parse().ok()?;
    let fits = |size: u32, min: u32| (min..=WINDOW_MAX).contains(&size);
    (fits(width, WINDOW_MIN[0]) && fits(height, WINDOW_MIN[1])).then_some([width, height])
}

/// The file's text for `settings`.
pub(super) fn format(settings: &Settings) -> String {
    let mut text = String::from(
        "# fm1-ui window settings, rewritten when they change in the window.\n# Delete this file to start from the defaults.\n",
    );
    if let Some([width, height]) = settings.window {
        text.push_str(&format!("window = [{width}, {height}]\n"));
    }
    text
}

/// The settings at `path` over `defaults`, and what was wrong with the file.
/// A missing file is the defaults; an unreadable one is the defaults and a
/// message (the next save replaces it).
pub(super) fn load(path: &Path, defaults: Settings) -> (Settings, Vec<String>) {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => {
                let (settings, problems) = parse(&text, defaults);
                let problems = problems
                    .into_iter()
                    .map(|problem| format!("{}: {problem}", path.display()))
                    .collect();
                (settings, problems)
            }
            Err(_) => (
                defaults,
                vec![format!("{}: not text, ignored", path.display())],
            ),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => (defaults, Vec::new()),
        Err(error) => (defaults, vec![format!("{}: {error}", path.display())]),
    }
}

/// Write `settings` to `path`: a temporary file beside it, then renamed over
/// it, so a reader sees the old file or the new one, never a part.
pub(super) fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let result = fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(format(settings).as_bytes())?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&temporary, path));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("{}: {error}", path.display()));
    }
    Ok(())
}

/// Saves the settings once they stop changing.
pub(super) struct Saver {
    path: PathBuf,
    /// What the file holds (or was last attempted).
    saved: Settings,
    /// Settings that differ from `saved`, and since when.
    pending: Option<(Settings, Instant)>,
}

impl Saver {
    /// A saver for `path`, which holds `saved`.
    pub fn new(path: PathBuf, saved: Settings) -> Self {
        Self {
            path,
            saved,
            pending: None,
        }
    }
    /// Note the settings as of `now`; once they have held still for
    /// `DEBOUNCE`, write them. Some(result) when it wrote (or tried to).
    pub fn update(&mut self, current: &Settings, now: Instant) -> Option<Result<(), String>> {
        if *current == self.saved {
            self.pending = None;
            return None;
        }
        match &self.pending {
            Some((pending, since)) if pending == current => {
                if now.duration_since(*since) < DEBOUNCE {
                    return None;
                }
            }
            _ => {
                self.pending = Some((current.clone(), now));
                return None;
            }
        }
        Some(self.write(current))
    }
    /// Write `current` now if it differs from the file (quitting).
    pub fn flush(&mut self, current: &Settings) -> Result<(), String> {
        if *current == self.saved {
            return Ok(());
        }
        self.write(current)
    }
    /// A failed write is not retried until the settings change again, so a
    /// read-only folder does not cost a write every frame.
    fn write(&mut self, current: &Settings) -> Result<(), String> {
        self.pending = None;
        self.saved = current.clone();
        save(&self.path, current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fm1-ui-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sized(width: u32, height: u32) -> Settings {
        Settings {
            window: Some([width, height]),
        }
    }

    #[test]
    fn defaults_when_there_is_no_file() {
        let dir = scratch("missing");
        let (settings, problems) = load(&dir.join(FILE_NAME), Settings::default());
        assert_eq!(settings, Settings::default());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!dir.exists(), "loading creates nothing");
    }

    #[test]
    fn saved_settings_load_back_and_the_write_is_atomic() {
        let dir = scratch("roundtrip");
        let path = dir.join("nested").join(FILE_NAME);
        save(&path, &sized(1300, 900)).unwrap();
        assert_eq!(
            load(&path, Settings::default()),
            (sized(1300, 900), Vec::new())
        );
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [FILE_NAME], "no temporary file left");
        save(&path, &Settings::default()).unwrap();
        assert_eq!(load(&path, sized(900, 700)).0, sized(900, 700));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_file_falls_back_per_line_and_says_why() {
        let text = "window = [10, 10]\nwindow = 1200x800\nnonsense\nfuture = 1\n";
        let (settings, problems) = parse(text, Settings::default());
        assert_eq!(settings, Settings::default());
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("line 1: bad value for window"));
        assert!(problems[2].starts_with("line 3: expected"));
        // Good lines among bad ones still count.
        let (settings, problems) = parse("\u{0}\u{1}\nwindow = [1000, 700]\n", Settings::default());
        assert_eq!(settings, sized(1000, 700));
        assert_eq!(problems.len(), 1);
        // Not UTF-8 at all: the defaults and one message.
        let dir = scratch("binary");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        fs::write(&path, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let (settings, problems) = load(&path, Settings::default());
        assert_eq!(settings, Settings::default());
        assert_eq!(problems.len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_file_sits_in_the_emulator_directory() {
        let path = default_path();
        assert_eq!(path.file_name().unwrap(), FILE_NAME);
        // Test binaries run from target/: the file goes beside Cargo.toml.
        assert_eq!(
            path.parent().unwrap(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
        );
    }

    #[test]
    fn changes_are_saved_once_they_settle_and_on_quit() {
        let dir = scratch("debounce");
        let path = dir.join(FILE_NAME);
        let mut saver = Saver::new(path.clone(), Settings::default());
        let start = Instant::now();
        assert_eq!(saver.update(&Settings::default(), start), None, "unchanged");
        // A resize: the size keeps moving, so nothing is written yet.
        for step in 0..10u32 {
            let at = start + Duration::from_millis(100 * u64::from(step));
            assert_eq!(saver.update(&sized(1000 + step, 700), at), None);
        }
        assert!(!path.exists());
        let last = sized(1009, 700);
        let settled = start + Duration::from_millis(900) + DEBOUNCE;
        assert_eq!(saver.update(&last, settled), Some(Ok(())));
        assert_eq!(load(&path, Settings::default()).0, last);
        assert_eq!(
            saver.update(&last, settled + DEBOUNCE),
            None,
            "written once"
        );
        // Changed back before the second passes: nothing to write.
        assert_eq!(saver.update(&sized(900, 600), settled), None);
        assert_eq!(saver.update(&last, settled + DEBOUNCE * 3), None);
        // Quitting writes a change at once.
        saver.update(&sized(1500, 1000), settled);
        saver.flush(&sized(1500, 1000)).unwrap();
        assert_eq!(load(&path, Settings::default()).0, sized(1500, 1000));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_is_reported_and_not_retried_every_frame() {
        let dir = scratch("readonly");
        fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("file");
        fs::write(&blocker, b"").unwrap();
        // A folder that cannot exist: its parent is a file.
        let mut saver = Saver::new(blocker.join(FILE_NAME), Settings::default());
        let start = Instant::now();
        let changed = sized(1000, 700);
        saver.update(&changed, start);
        assert!(matches!(
            saver.update(&changed, start + DEBOUNCE),
            Some(Err(_))
        ));
        assert_eq!(saver.update(&changed, start + DEBOUNCE * 2), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
