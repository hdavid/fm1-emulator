// SPDX-License-Identifier: GPL-3.0-only
// The window's own settings, kept between runs of fm1-ui: the MASTER volume,
// the panel theme, the LEDs switch and the window size. They live in a short
// text file (`fm1-ui.toml`, a few `key = value` lines) beside the flash state,
// written a second after the last change and when the window closes.
// Headless tools never read or write it.
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The settings file's name.
pub(super) const FILE_NAME: &str = "fm1-ui.toml";
/// Quiet time after a change before the file is written: a drag or a window
/// resize is saved once, when it settles.
pub(super) const DEBOUNCE: Duration = Duration::from_secs(1);
/// Window sizes outside this range (points) are not restored.
const WINDOW_MIN: [u32; 2] = [800, 600];
const WINDOW_MAX: u32 = 16_384;
/// Highest MASTER reading (the ADC is 10-bit).
const MASTER_MAX: u16 = 1023;
/// How many opened firmware files are remembered.
pub(super) const RECENT_MAX: usize = 8;

/// What the window remembers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Settings {
    /// MASTER potentiometer as the ADC reads it (0..=1023).
    pub master: u16,
    /// The panel theme's name; None: the default.
    pub theme: Option<String>,
    /// The LEDs switch.
    pub leds: bool,
    /// The window's inner size in points; None: the default.
    pub window: Option<[u32; 2]>,
    /// Firmware files opened, the latest first (at most `RECENT_MAX`).
    pub recent: Vec<PathBuf>,
    /// Start with the latest firmware when none is named on the command line.
    pub reopen_last: bool,
}

impl Settings {
    pub fn new(master: u16) -> Self {
        Self {
            master,
            theme: None,
            leds: true,
            window: None,
            recent: Vec::new(),
            reopen_last: true,
        }
    }
    /// The firmware to start with when none is named: the latest that is
    /// still a file, if the setting allows it.
    pub fn startup_firmware(&self) -> Option<&Path> {
        self.reopen_last
            .then(|| self.recent.first())
            .flatten()
            .map(PathBuf::as_path)
            .filter(|path| path.is_file())
    }
}

/// `path` as the latest firmware: first in `recent`, once, the list capped.
pub(super) fn remember(recent: &mut Vec<PathBuf>, path: &Path) {
    recent.retain(|known| known != path);
    recent.insert(0, path.to_path_buf());
    recent.truncate(RECENT_MAX);
}

/// The settings file in `state` (the `--state` folder, or the folder of a
/// `--state` file), else in `default_dir`.
pub(super) fn path_for(state: Option<&Path>, default_dir: &Path) -> PathBuf {
    let dir = match state {
        None => default_dir.to_path_buf(),
        Some(path) if path.is_dir() || path.to_string_lossy().ends_with(['/', '\\']) => {
            path.to_path_buf()
        }
        Some(path) => path.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    dir.join(FILE_NAME)
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
            "master" => parse_master(value).map(|master| settings.master = master),
            "theme" => parse_string(value).map(|theme| settings.theme = Some(theme)),
            "leds" => parse_bool(value).map(|leds| settings.leds = leds),
            "window" => parse_window(value).map(|window| settings.window = Some(window)),
            "reopen_last" => parse_bool(value).map(|reopen| settings.reopen_last = reopen),
            "recent" => parse_list(value).map(|list| {
                settings.recent = list
                    .into_iter()
                    .take(RECENT_MAX)
                    .map(PathBuf::from)
                    .collect()
            }),
            _ => Some(()),
        };
        if applied.is_none() {
            problems.push(format!("line {}: bad value for {key}: {value}", number + 1));
        }
    }
    (settings, problems)
}

fn parse_master(value: &str) -> Option<u16> {
    value.parse().ok().filter(|master| *master <= MASTER_MAX)
}

/// A quoted string; `\\` and `\"` are the only escapes (so Windows paths fit).
fn parse_string(value: &str) -> Option<String> {
    let (text, rest) = take_string(value)?;
    rest.trim().is_empty().then_some(text)
}

/// The quoted string at the start of `value` and what follows it.
fn take_string(value: &str) -> Option<(String, &str)> {
    let mut chars = value.strip_prefix('"')?.char_indices();
    let mut text = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((text, &value[at + 2..])),
            '\\' => match chars.next()?.1 {
                escaped @ ('\\' | '"') => text.push(escaped),
                _ => return None,
            },
            c => text.push(c),
        }
    }
    None
}

/// `["a", "b"]`: quoted strings in brackets.
fn parse_list(value: &str) -> Option<Vec<String>> {
    let mut rest = value.strip_prefix('[')?.trim_start();
    let mut list = Vec::new();
    while !rest.starts_with(']') {
        let (text, after) = take_string(rest)?;
        list.push(text);
        rest = after.trim_start();
        rest = match rest.strip_prefix(',') {
            Some(after) => after.trim_start(),
            None if rest.starts_with(']') => rest,
            None => return None,
        };
    }
    rest[1..].trim().is_empty().then_some(list)
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
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
    text.push_str(&format!("master = {}\n", settings.master));
    if let Some(theme) = &settings.theme {
        text.push_str(&format!("theme = {}\n", quote(theme)));
    }
    text.push_str(&format!("leds = {}\n", settings.leds));
    if let Some([width, height]) = settings.window {
        text.push_str(&format!("window = [{width}, {height}]\n"));
    }
    text.push_str(&format!("reopen_last = {}\n", settings.reopen_last));
    // A path that is not valid text cannot be written, so it is not kept.
    let recent: Vec<String> = settings
        .recent
        .iter()
        .filter_map(|path| path.to_str().map(quote))
        .collect();
    if !recent.is_empty() {
        text.push_str(&format!("recent = [{}]\n", recent.join(", ")));
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

/// Write `settings` to `path` atomically, creating its folder.
pub(super) fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    fm1_emu::flash_state::write_atomic(path, format(settings).as_bytes())
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

    fn sample() -> Settings {
        Settings {
            master: 700,
            theme: Some("Mint".into()),
            leds: false,
            window: Some([1300, 900]),
            recent: vec![
                PathBuf::from("/fw/optimist-0.2.fwsc"),
                PathBuf::from("C:\\Users\\ann\\My \"fw\", too.fwsc"),
            ],
            reopen_last: false,
        }
    }

    #[test]
    fn defaults_when_there_is_no_file() {
        let dir = scratch("missing");
        let (settings, problems) = load(&dir.join(FILE_NAME), Settings::new(512));
        assert_eq!(settings, Settings::new(512));
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!dir.exists(), "loading creates nothing");
    }

    #[test]
    fn saved_settings_load_back_and_the_write_is_atomic() {
        let dir = scratch("roundtrip");
        let path = dir.join("nested").join(FILE_NAME);
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path, Settings::new(512)), (sample(), Vec::new()));
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [FILE_NAME], "no temporary file left");
        let defaults = Settings::new(512);
        save(&path, &defaults).unwrap();
        assert_eq!(load(&path, Settings::new(0)).0, defaults);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_file_falls_back_per_line_and_says_why() {
        let text =
            "master = 2000\ntheme = Mint\nleds = maybe\nwindow = [10, 10]\nnonsense\nfuture = 1\n";
        let (settings, problems) = parse(text, Settings::new(512));
        assert_eq!(settings, Settings::new(512));
        assert_eq!(problems.len(), 5, "{problems:?}");
        assert!(problems[0].starts_with("line 1: bad value for master"));
        assert!(problems[4].starts_with("line 5: expected"));
        // Good lines among bad ones still count.
        let (settings, problems) =
            parse("leds = false\n\u{0}\u{1}\nmaster = 3\n", Settings::new(512));
        assert_eq!((settings.leds, settings.master), (false, 3));
        assert_eq!(problems.len(), 1);
        // Not UTF-8 at all: the defaults and one message.
        let dir = scratch("binary");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        fs::write(&path, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let (settings, problems) = load(&path, Settings::new(512));
        assert_eq!(settings, Settings::new(512));
        assert_eq!(problems.len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_file_sits_beside_the_flash_state() {
        let default = Path::new("/emu/state");
        assert_eq!(path_for(None, default), default.join(FILE_NAME));
        assert_eq!(
            path_for(Some(Path::new("/s/emulator/state/")), default),
            Path::new("/s/emulator/state/").join(FILE_NAME)
        );
        assert_eq!(
            path_for(Some(Path::new("/s/mine.nor")), default),
            Path::new("/s").join(FILE_NAME)
        );
    }

    #[test]
    fn quotes_and_backslashes_survive_the_file() {
        let settings = Settings {
            theme: Some("a\"b\\c".into()),
            ..sample()
        };
        assert_eq!(
            parse(&format(&settings), Settings::new(9)),
            (settings, vec![])
        );
        // Odd strings are refused, not misread.
        assert_eq!(parse_string(r#""a\nb""#), None);
        assert_eq!(parse_string(r#""open"#), None);
        assert_eq!(parse_string(r#""a" x"#), None);
        assert_eq!(parse_list(r#"["a", "b""#), None);
        assert_eq!(parse_list(r#"["a" "b"]"#), None);
        assert_eq!(parse_list("[]"), Some(vec![]));
        assert_eq!(
            parse_list(r#"[ "a,b" , "c" ]"#),
            Some(vec!["a,b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn the_last_firmware_is_remembered_and_reopened_only_if_it_exists() {
        let dir = scratch("recent");
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.fwsc"), dir.join("b.fwsc"));
        fs::write(&a, b"x").unwrap();
        let mut recent = Vec::new();
        remember(&mut recent, &a);
        remember(&mut recent, &b);
        remember(&mut recent, &a);
        assert_eq!(
            recent,
            [a.clone(), b.clone()],
            "latest first, no duplicates"
        );
        for n in 0..20 {
            remember(&mut recent, Path::new(&format!("/fw/{n}.fwsc")));
        }
        assert_eq!(recent.len(), RECENT_MAX);
        assert_eq!(recent[0], Path::new("/fw/19.fwsc"));
        // Saved and read back, in the file.
        let path = dir.join(FILE_NAME);
        let settings = Settings {
            recent: vec![a.clone(), b.clone()],
            ..Settings::new(512)
        };
        save(&path, &settings).unwrap();
        let (loaded, problems) = load(&path, Settings::new(0));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(loaded, settings);
        // The default reopens the latest if it is still a file.
        assert_eq!(loaded.startup_firmware(), Some(a.as_path()));
        let missing = Settings {
            recent: vec![b.clone()],
            ..settings.clone()
        };
        assert_eq!(missing.startup_firmware(), None, "b.fwsc does not exist");
        let off = Settings {
            reopen_last: false,
            ..settings
        };
        assert_eq!(off.startup_firmware(), None, "the setting is off");
        assert_eq!(
            Settings::new(1).startup_firmware(),
            None,
            "nothing opened yet"
        );
        // An older file without these keys keeps the defaults.
        let (old, _) = parse("master = 5\n", Settings::new(512));
        assert!(old.reopen_last && old.recent.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changes_are_saved_once_they_settle_and_on_quit() {
        let dir = scratch("debounce");
        let path = dir.join(FILE_NAME);
        let mut saver = Saver::new(path.clone(), Settings::new(512));
        let start = Instant::now();
        assert_eq!(saver.update(&Settings::new(512), start), None, "unchanged");
        // A drag: the value keeps moving, so nothing is written yet.
        for step in 0..10u16 {
            let at = start + Duration::from_millis(100 * u64::from(step));
            assert_eq!(saver.update(&Settings::new(600 + step), at), None);
        }
        assert!(!path.exists());
        let last = Settings::new(609);
        let settled = start + Duration::from_millis(900) + DEBOUNCE;
        assert_eq!(saver.update(&last, settled), Some(Ok(())));
        assert_eq!(load(&path, Settings::new(0)).0, last);
        assert_eq!(
            saver.update(&last, settled + DEBOUNCE),
            None,
            "written once"
        );
        // Changed back before the second passes: nothing to write.
        assert_eq!(saver.update(&Settings::new(1), settled), None);
        assert_eq!(saver.update(&last, settled + DEBOUNCE * 3), None);
        // Quitting writes a change at once.
        saver.update(&sample(), settled);
        saver.flush(&sample()).unwrap();
        assert_eq!(load(&path, Settings::new(0)).0, sample());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_is_reported_and_not_retried_every_frame() {
        let dir = scratch("readonly");
        fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("file");
        fs::write(&blocker, b"").unwrap();
        // A folder that cannot exist: its parent is a file.
        let mut saver = Saver::new(blocker.join(FILE_NAME), Settings::new(512));
        let start = Instant::now();
        let changed = Settings::new(100);
        saver.update(&changed, start);
        assert!(matches!(
            saver.update(&changed, start + DEBOUNCE),
            Some(Err(_))
        ));
        assert_eq!(saver.update(&changed, start + DEBOUNCE * 2), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
