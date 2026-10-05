// SPDX-License-Identifier: GPL-3.0-only
// The GUI side of the web editor bridge (fm1_emu::web): which editor this
// firmware has (X-ui.zip next to X.fwsc, or --ui DIR), the loopback server
// for it, the MIDI pump between the server and the emulated USB device, and
// what the toolbar button and the status line show.
use fm1_emu::{
    bus::Bus,
    usb_midi::Decoder,
    web::{self, Hub, Server, Source},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// How long the traffic indicator stays lit after a message.
const TRAFFIC_GLOW: Duration = Duration::from_millis(300);

pub struct WebEditor {
    hub: Arc<Hub>,
    server: Option<Server>,
    /// Why there is no editor (shown on the disabled button and status line).
    unavailable: Option<String>,
    /// Where the editor's files come from.
    origin: Option<PathBuf>,
    decoder: Decoder,
    counted: (u64, u64),
    last_traffic: Option<Instant>,
}

impl WebEditor {
    /// No editor, no server, and the USB host stays a CDC-only host.
    pub fn disabled(reason: &str) -> Self {
        Self {
            hub: Arc::new(Hub::default()),
            server: None,
            unavailable: Some(reason.to_owned()),
            origin: None,
            decoder: Decoder::default(),
            counted: (0, 0),
            last_traffic: None,
        }
    }

    /// The editor of `firmware` (`ui_dir` overrides the sidecar), served on
    /// `address` when there is one.
    pub fn new(firmware: &Path, ui_dir: Option<&Path>, address: &str) -> Self {
        let Some(origin) = ui_dir
            .map(Path::to_path_buf)
            .or_else(|| web::sidecar_for(firmware))
        else {
            let stem = firmware.file_stem().unwrap_or_default().to_string_lossy();
            return Self::disabled(&format!(
                "No web editor for this firmware: put {stem}-ui.zip next to it, or start with --ui DIR"
            ));
        };
        let source = match Source::open(&origin) {
            Ok(source) if source.has_index() => source,
            Ok(_) => {
                return Self::disabled(&format!(
                    "{} has no index.html or editor.html",
                    origin.display()
                ))
            }
            Err(error) => return Self::disabled(&format!("{}: {error}", origin.display())),
        };
        let hub = Arc::new(Hub::default());
        match Server::start(address, Some(source), hub.clone()) {
            Ok(server) => Self {
                hub,
                server: Some(server),
                unavailable: None,
                origin: Some(origin),
                ..Self::disabled("")
            },
            Err(error) => Self::disabled(&format!("editor server on {address}: {error}")),
        }
    }

    pub fn active(&self) -> bool {
        self.server.is_some()
    }

    pub fn url(&self) -> Option<String> {
        self.server.as_ref().map(Server::url)
    }

    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    /// A fresh device (restart): forget a half-received message.
    pub fn attach(&mut self, bus: &mut Bus) {
        self.decoder = Decoder::default();
        if self.active() {
            bus.usb.enable_midi_host();
        }
    }

    /// Move MIDI between the browser clients and the device.
    pub fn pump(&mut self, bus: &mut Bus) {
        if !self.active() {
            return;
        }
        web::pump(&self.hub, bus, &mut self.decoder);
        let stats = self.hub.stats();
        let counted = (stats.to_device, stats.from_device);
        if counted != self.counted {
            self.counted = counted;
            self.last_traffic = Some(Instant::now());
        }
    }

    /// Whether MIDI moved in the last moment (the status line's indicator).
    pub fn busy(&self) -> bool {
        self.last_traffic
            .is_some_and(|at| at.elapsed() < TRAFFIC_GLOW)
    }

    /// The status line: URL, clients, traffic and what the device reported.
    pub fn status(&self) -> String {
        let Some(url) = self.url() else {
            return self.unavailable.clone().unwrap_or_default();
        };
        let stats = self.hub.stats();
        let device = if self.hub.ready() {
            format!("port \"{}\"", self.hub.port_name())
        } else {
            String::from("device not enumerated yet")
        };
        let clients = match stats.clients {
            1 => String::from("1 page connected"),
            n => format!("{n} pages connected"),
        };
        let from = self
            .origin
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or(String::new(), |n| format!(" ({})", n.to_string_lossy()));
        format!(
            "Editor {url}{from} · {clients} · MIDI to device {} / from device {} · {device}",
            stats.to_device, stats.from_device
        )
    }

    /// Open the editor in the default browser.
    pub fn open_in_browser(&self) -> Result<(), String> {
        let url = self.url().ok_or("no editor")?;
        let mut command = if cfg!(target_os = "macos") {
            std::process::Command::new("open")
        } else if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        } else {
            std::process::Command::new("xdg-open")
        };
        command
            .arg(&url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("could not open {url}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fm1-ui-web-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn without_a_sidecar_the_editor_is_off_and_says_why() {
        let dir = scratch("none");
        let editor = WebEditor::new(&dir.join("stock.fwsc"), None, "127.0.0.1:0");
        assert!(!editor.active());
        assert_eq!(editor.url(), None);
        let reason = editor.unavailable().unwrap();
        assert!(reason.contains("stock-ui.zip"), "{reason}");
        assert_eq!(editor.status(), reason);
    }

    #[test]
    fn a_ui_directory_starts_the_server_and_enables_the_midi_host() {
        let dir = scratch("dir");
        std::fs::write(dir.join("index.html"), "<html><head></head></html>").unwrap();
        let mut editor = WebEditor::new(&dir.join("x.fwsc"), Some(&dir), "127.0.0.1:0");
        assert!(editor.active(), "{:?}", editor.unavailable());
        assert!(editor.url().unwrap().starts_with("http://127.0.0.1:"));
        assert!(
            editor.status().contains("0 pages connected"),
            "{}",
            editor.status()
        );
        let mut bus = Bus::new(vec![0; 64]).unwrap();
        editor.attach(&mut bus);
        editor.pump(&mut bus);
        assert!(editor.status().contains("device not enumerated yet"));
        assert!(!editor.busy());
    }

    #[test]
    fn a_directory_without_index_html_is_refused() {
        let dir = scratch("empty");
        let editor = WebEditor::new(&dir.join("x.fwsc"), Some(&dir), "127.0.0.1:0");
        assert!(!editor.active());
        assert!(editor.unavailable().unwrap().contains("index.html"));
    }
}
