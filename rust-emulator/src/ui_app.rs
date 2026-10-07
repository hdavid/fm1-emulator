// SPDX-License-Identifier: GPL-3.0-only
// Where an installed copy of fm1-ui keeps its per-user files. The command line
// and the launchers keep today's places (`state/` beside the emulator, or
// `--state`); only the "app mode" -- a macOS .app bundle, or a bare launch with
// no arguments (a double-click or a desktop entry) -- uses the operating
// system's per-user application-data folder, because the folder the program
// sits in may be read-only (an .app in /Applications, Program Files).
use std::path::{Path, PathBuf};

/// The folder's name where the OS convention allows spaces.
const APP_NAME: &str = "FM-1 Emulator";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub const HOST: Os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
}

/// The environment variables the choice reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Env {
    pub home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
}

impl Env {
    pub fn from_process() -> Self {
        let var = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        Self {
            home: var("HOME").or_else(|| var("USERPROFILE")),
            appdata: var("APPDATA"),
            xdg_data_home: var("XDG_DATA_HOME"),
        }
    }
}

/// The per-user application-data folder: `~/Library/Application Support/FM-1
/// Emulator` on macOS, `%APPDATA%\FM-1 Emulator` on Windows,
/// `$XDG_DATA_HOME/fm1-emulator` (default `~/.local/share/fm1-emulator`) on
/// Linux. None when the environment does not say where the user's home is.
pub(super) fn data_dir(os: Os, env: &Env) -> Option<PathBuf> {
    match os {
        Os::Mac => Some(
            env.home
                .as_ref()?
                .join("Library")
                .join("Application Support")
                .join(APP_NAME),
        ),
        Os::Windows => Some(env.appdata.as_ref()?.join(APP_NAME)),
        Os::Linux => {
            let base = match &env.xdg_data_home {
                Some(xdg) => xdg.clone(),
                None => env.home.as_ref()?.join(".local").join("share"),
            };
            Some(base.join("fm1-emulator"))
        }
    }
}

/// Whether `exe` is the program inside a macOS bundle:
/// `Something.app/Contents/MacOS/fm1-ui`.
fn in_app_bundle(exe: &Path) -> bool {
    let mut ancestors = exe.ancestors().skip(1);
    let is = |dir: Option<&Path>, name: &str| {
        dir.and_then(Path::file_name)
            .is_some_and(|found| found == name)
    };
    is(ancestors.next(), "MacOS")
        && is(ancestors.next(), "Contents")
        && ancestors
            .next()
            .and_then(Path::extension)
            .is_some_and(|extension| extension == "app")
}

/// A build inside a cargo `target/` folder is a development build: it keeps
/// the files beside the checkout, as the launchers and tools expect.
fn in_cargo_target(exe: &Path) -> bool {
    exe.ancestors()
        .skip(1)
        .any(|dir| dir.file_name().is_some_and(|name| name == "target"))
}

/// How the program was started, for `is_app_mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Launch {
    /// No command-line arguments at all.
    pub bare: bool,
    /// `--state` was given: the caller chose where the files go.
    pub state_given: bool,
    /// `--app-data` was given.
    pub forced: bool,
}

/// App mode: `--app-data` forces it; `--state` or a development build
/// (under `target/`) rules it out; otherwise a macOS bundle or a bare launch.
pub(super) fn is_app_mode(exe: &Path, launch: Launch) -> bool {
    if launch.state_given {
        return false;
    }
    launch.forced || (!in_cargo_target(exe) && (in_app_bundle(exe) || launch.bare))
}

/// Where the files go.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Places {
    /// The settings file.
    pub settings: PathBuf,
    /// The flash-state folder (with a trailing separator, as `--state` takes it).
    pub state: PathBuf,
}

/// The places in app mode under `data`.
pub(super) fn places_in(data: &Path) -> Places {
    Places {
        settings: data.join(super::ui_settings::FILE_NAME),
        state: data.join("state").join(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            home: Some("/home/ann".into()),
            appdata: Some(r"C:\Users\ann\AppData\Roaming".into()),
            xdg_data_home: None,
        }
    }

    #[test]
    fn each_os_has_its_own_per_user_folder() {
        assert_eq!(
            data_dir(Os::Mac, &env()),
            Some(PathBuf::from(
                "/home/ann/Library/Application Support/FM-1 Emulator"
            ))
        );
        assert_eq!(
            data_dir(Os::Windows, &env()),
            Some(PathBuf::from(r"C:\Users\ann\AppData\Roaming").join("FM-1 Emulator"))
        );
        assert_eq!(
            data_dir(Os::Linux, &env()),
            Some(PathBuf::from("/home/ann/.local/share/fm1-emulator"))
        );
        let xdg = Env {
            xdg_data_home: Some("/data".into()),
            ..env()
        };
        assert_eq!(
            data_dir(Os::Linux, &xdg),
            Some(PathBuf::from("/data/fm1-emulator"))
        );
        assert_eq!(data_dir(Os::Mac, &Env::default()), None);
        assert_eq!(data_dir(Os::Windows, &Env::default()), None);
    }

    #[test]
    fn a_bundle_or_a_bare_launch_is_app_mode_but_a_checkout_and_state_are_not() {
        let bundle = Path::new("/Applications/FM-1 Emulator.app/Contents/MacOS/fm1-ui");
        let loose = Path::new("/opt/fm1/emulator");
        let dev = Path::new("/src/rust-emulator/target/release/fm1-ui");
        let dev_bundle =
            Path::new("/src/rust-emulator/target/release/FM-1 Emulator.app/Contents/MacOS/fm1-ui");
        let args = Launch::default();
        let bare = Launch { bare: true, ..args };
        assert!(is_app_mode(bundle, args), "a bundle, even given a firmware");
        assert!(is_app_mode(bundle, bare));
        assert!(!is_app_mode(loose, args), "the command line");
        assert!(is_app_mode(loose, bare), "a double-click");
        assert!(!is_app_mode(dev, bare), "a development build");
        assert!(!is_app_mode(dev_bundle, args), "the ./emulator launcher");
        let state = Launch {
            state_given: true,
            ..bare
        };
        assert!(!is_app_mode(bundle, state), "--state wins (tools/emu.py)");
        let forced = Launch {
            forced: true,
            ..args
        };
        assert!(is_app_mode(dev, forced), "--app-data");
        assert!(!is_app_mode(
            dev,
            Launch {
                state_given: true,
                ..forced
            }
        ));
    }

    #[test]
    fn app_mode_files_sit_in_the_data_folder() {
        let places = places_in(Path::new("/data/fm1"));
        assert_eq!(places.settings, Path::new("/data/fm1/fm1-ui.toml"));
        assert!(places.state.to_string_lossy().ends_with('/'));
        assert_eq!(places.state, Path::new("/data/fm1/state"));
        // A folder given with its trailing separator is how --state names one
        // that does not exist yet.
        let store = fm1_emu::flash_state::Store::for_firmware(
            Some(&places.state),
            Path::new("optimist-0.2.fwsc"),
        );
        assert_eq!(store.image, Path::new("/data/fm1/state/optimist.nor"));
    }
}
