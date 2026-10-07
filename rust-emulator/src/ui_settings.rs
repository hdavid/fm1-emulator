// SPDX-License-Identifier: GPL-3.0-only
// What fm1-ui remembers between runs: the firmware files it opened and whether
// to reopen the latest at start. They live in a short text file (`fm1-ui.toml`,
// a few `key = value` lines), written when a firmware is loaded or the
// checkbox changes. Headless tools never read or write it.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// The settings file's name.
pub(super) const FILE_NAME: &str = "fm1-ui.toml";
/// How many opened firmware files are remembered.
pub(super) const RECENT_MAX: usize = 8;

/// What the window remembers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Settings {
    /// Firmware files opened, the latest first (at most `RECENT_MAX`).
    pub recent: Vec<PathBuf>,
    /// Start with the latest firmware when none is named on the command line.
    pub reopen_last: bool,
}

impl Settings {
    pub fn new() -> Self {
        Self {
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
            problems.push(format!("line {}: cannot read {key}", number + 1));
        }
    }
    (settings, problems)
}

/// The quoted string at the start of `value` and what follows it. `\\` and
/// `\"` are the only escapes (so Windows paths fit).
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

/// The file's text for `settings`.
pub(super) fn format(settings: &Settings) -> String {
    let mut text = String::from(
        "# fm1-ui settings, rewritten when they change in the window.\n# Delete this file to start from the defaults.\n",
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fm1-ui-settings-{name}-{}", std::process::id()))
    }

    #[test]
    fn quotes_and_backslashes_survive_the_file() {
        let settings = Settings {
            recent: vec![
                PathBuf::from("/fw/optimist-0.2.fwsc"),
                PathBuf::from("C:\\Users\\ann\\My \"fw\", too.fwsc"),
            ],
            reopen_last: false,
        };
        assert_eq!(
            parse(&format(&settings), Settings::new()),
            (settings, vec![])
        );
        // Odd strings are refused, not misread.
        assert_eq!(take_string(r#""a\nb""#), None);
        assert_eq!(take_string(r#""open"#), None);
        assert_eq!(parse_list(r#"["a", "b""#), None);
        assert_eq!(parse_list(r#"["a" "b"]"#), None);
        assert_eq!(parse_list("[]"), Some(vec![]));
        assert_eq!(
            parse_list(r#"[ "a,b" , "c" ]"#),
            Some(vec!["a,b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn bad_lines_are_reported_and_unknown_keys_skipped() {
        let (settings, problems) = parse(
            "reopen_last = maybe\nnonsense\nfuture_key = 3\nrecent = [\"/a.fwsc\"]\n",
            Settings::new(),
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(settings.reopen_last, "the default stays");
        assert_eq!(settings.recent, [PathBuf::from("/a.fwsc")]);
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
            ..Settings::new()
        };
        save(&path, &settings).unwrap();
        let (loaded, problems) = load(&path, Settings::new());
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
            Settings::new().startup_firmware(),
            None,
            "nothing opened yet"
        );
        // A missing file is the defaults.
        let (none, problems) = load(&dir.join("absent.toml"), Settings::new());
        assert!(problems.is_empty() && none == Settings::new());
        fs::remove_dir_all(dir).unwrap();
    }
}
