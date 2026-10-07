// SPDX-License-Identifier: GPL-3.0-only
// The Windows GUI subsystem: double-clicking the program opens the window and
// no console. The command line still works from a terminal: ui_console joins
// the parent's console at start, so messages and stdin behave as before.
#![cfg_attr(windows, windows_subsystem = "windows")]
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
#[cfg(test)]
use fm1_emu::{bus::Bus, cpu::Cpu, firmware::Firmware};
mod ui_app;
mod ui_console;
mod ui_settings;
mod worker;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

// Matrix IDs/wiring from fm1_input.h and the pinned Felucca panel.c defaults.
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments.
const KEYMAP: [[i8; 11]; 4] = [
    [5, 11, 4, 10, 3, 9, 2, 8, -1, -1, -1],
    [34, 35, 36, 37, 38, 40, 39, 13, 7, 6, 12],
    [23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33],
    [0, 1, 15, 14, 17, 16, 19, 18, 20, 21, 22],
];
const NOTE_KEYS: [egui::Key; 13] = [
    egui::Key::A,
    egui::Key::W,
    egui::Key::S,
    egui::Key::E,
    egui::Key::D,
    egui::Key::R,
    egui::Key::F,
    egui::Key::G,
    egui::Key::T,
    egui::Key::H,
    egui::Key::Y,
    egui::Key::J,
    egui::Key::K,
];
const INK: Color32 = Color32::from_rgb(190, 194, 193);
const ACCENT: Color32 = Color32::from_rgb(231, 193, 91);

/// The firmware formats the loader reads: `.fwsc` packages, ELF files and raw
/// application images.
const FIRMWARE_EXTENSIONS: [&str; 3] = ["fwsc", "elf", "bin"];

/// Whether `path` looks like a firmware file (by its extension).
fn is_firmware_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            FIRMWARE_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// The window title for the running firmware.
fn window_title(path: Option<&std::path::Path>) -> String {
    match path.and_then(|path| path.file_name()) {
        Some(name) => format!("{} - FM-1 Emulator", name.to_string_lossy()),
        None => "FM-1 Emulator".into(),
    }
}

struct Emulator {
    /// The firmware that runs; None until one is loaded.
    path: Option<PathBuf>,
    /// Firmware files opened, the latest first, and whether to reopen the
    /// latest at start (both kept in the settings).
    recent: Vec<PathBuf>,
    reopen_last: bool,
    /// Where the settings are saved; None: not saved.
    settings_path: Option<PathBuf>,
    /// The settings last written, to save only on a change.
    saved: ui_settings::Settings,
    /// "Load firmware…" clicked: open the file dialog at the next frame.
    pick_requested: bool,
    /// The last thing that went wrong with a firmware file, for the status line.
    notice: Option<String>,
    /// The window title last sent to the window system.
    title: String,
    worker: worker::Worker,
    generation: u64,
    loaded: bool,
    steps: u64,
    fault: Option<String>,
    paused: bool,
    texture: Option<egui::TextureHandle>,
    pressed: [bool; 41],
    pulse: [Instant; 41],
    pulse_steps: [u64; 41],
}
impl Emulator {
    fn new(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            title: window_title(path.as_deref()),
            path,
            recent: Vec::new(),
            reopen_last: true,
            settings_path: None,
            saved: ui_settings::Settings::new(),
            pick_requested: false,
            notice: None,
            worker: worker::Worker::new(),
            generation: 0,
            loaded: false,
            steps: 0,
            fault: None,
            paused: false,
            texture: None,
            pressed: [false; 41],
            pulse: [Instant::now(); 41],
            pulse_steps: [0; 41],
        };
        app.reset();
        app
    }
    fn reset(&mut self) {
        self.pressed.fill(false);
        self.pulse.fill(Instant::now());
        self.pulse_steps.fill(0);
        self.paused = false;
        self.texture = None;
        self.generation += 1;
        self.loaded = false;
        self.steps = 0;
        self.fault = None;
        if let Some(path) = &self.path {
            self.worker.restart(self.generation, path.clone());
        }
    }
    /// Take the remembered settings, and save later changes to `path`.
    fn apply_settings(&mut self, settings: ui_settings::Settings, path: PathBuf) {
        self.recent = settings.recent.clone();
        self.reopen_last = settings.reopen_last;
        self.saved = settings;
        self.settings_path = Some(path);
    }
    /// Write the settings when they changed (no-op without a settings path).
    fn keep_settings(&mut self) {
        let Some(path) = &self.settings_path else {
            return;
        };
        let now = ui_settings::Settings {
            recent: self.recent.clone(),
            reopen_last: self.reopen_last,
        };
        if now == self.saved {
            return;
        }
        if let Err(error) = ui_settings::save(path, &now) {
            eprintln!("window settings: {error}");
        }
        self.saved = now;
    }
    /// Boot `path` as a power cycle: the CPU, RAM and devices are rebuilt from
    /// the new package.
    fn load_firmware(&mut self, path: PathBuf) {
        self.notice = None;
        // Remembered paths must still name the file from another folder.
        let path = std::path::absolute(&path).unwrap_or(path);
        ui_settings::remember(&mut self.recent, &path);
        self.path = Some(path);
        self.reset();
    }
    /// Load a firmware file the user named (dialog, recent list, drop).
    fn open_firmware(&mut self, path: PathBuf) {
        if is_firmware_file(&path) {
            self.load_firmware(path);
        } else {
            self.notice = Some(format!(
                "{} is not a firmware file (.fwsc, .elf or .bin)",
                path.display()
            ));
        }
    }
    /// Ask for a firmware file with the system's own dialog (blocks the
    /// window while it is open; it is modal anyway) and load it.
    fn pick_firmware(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Load firmware")
            .add_filter("FM-1 firmware", &FIRMWARE_EXTENSIONS)
            .add_filter("All files", &["*"]);
        if let Some(folder) = self
            .path
            .as_deref()
            .or(self.recent.first().map(PathBuf::as_path))
            .and_then(std::path::Path::parent)
            .filter(|folder| folder.is_dir())
        {
            dialog = dialog.set_directory(folder);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_firmware(path);
        }
    }
    /// The toolbar's firmware controls: the load button and the recent list.
    fn firmware_controls(&mut self, ui: &mut egui::Ui) {
        if ui
            .button("Load firmware…")
            .on_hover_text("Pick a .fwsc, .elf or .bin file. The emulator reboots into it. You can also drop a file on the window.")
            .clicked()
        {
            self.pick_requested = true;
        }
        let mut chosen = None;
        ui.menu_button("Recent", |ui| {
            if self.recent.is_empty() {
                ui.weak("No firmware opened yet");
            }
            for path in &self.recent {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if ui
                    .button(name)
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    chosen = Some(path.clone());
                    ui.close_menu();
                }
            }
            ui.separator();
            ui.checkbox(&mut self.reopen_last, "Reopen the last firmware at start")
                .on_hover_text("Without a firmware on the command line, start with the latest one if it still exists");
        });
        if let Some(path) = chosen {
            self.open_firmware(path);
        }
    }
    /// What the window shows before any firmware is loaded.
    fn idle_prompt(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("open_firmware_prompt"))
            .anchor(Align2::CENTER_CENTER, vec2(0., 0.))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(24.)
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("No firmware loaded");
                            ui.label(
                                "Load a .fwsc package (or an .elf / .bin image) to start the FM-1.",
                            );
                            ui.add_space(8.);
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("Load firmware…").size(18.),
                                ))
                                .clicked()
                            {
                                self.pick_requested = true;
                            }
                            ui.weak("or drop a firmware file on this window");
                        });
                    });
            });
    }
    /// Firmware files dropped on the window: the first one is loaded.
    fn dropped_firmware(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.into_iter().find_map(|file| file.path) {
            self.open_firmware(path);
        }
    }
    fn refresh(&mut self, ctx: &egui::Context) {
        self.worker.input(self.pressed);
        self.worker.pause(self.paused);
        let Some(snapshot) = self.worker.snapshot() else {
            return;
        };
        if snapshot.generation != self.generation {
            return;
        }
        self.loaded = snapshot.loaded;
        self.steps = snapshot.steps;
        self.fault = snapshot.fault;
        let Some(pixels) = snapshot.pixels else {
            return;
        };
        let image = egui::ColorImage {
            size: [240, 240],
            pixels: pixels
                .into_iter()
                .map(|rgb| Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
                .collect(),
        };
        if let Some(texture) = &mut self.texture {
            texture.set(image, egui::TextureOptions::NEAREST);
        } else {
            self.texture =
                Some(ctx.load_texture("Guest LCD", image, egui::TextureOptions::NEAREST));
        }
    }
    fn key(
        &mut self,
        ui: &mut egui::Ui,
        canvas: &Canvas,
        id: usize,
        rect: Rect,
        label: &str,
        note: bool,
    ) {
        let response = ui.interact(rect, egui::Id::new(("key", id)), Sense::click_and_drag());
        let down = response.is_pointer_button_down_on();
        if down || response.clicked() {
            self.pulse[id] = Instant::now() + Duration::from_millis(100);
            self.pulse_steps[id] = self.steps + 72_000_000;
        }
        let focused = ui.input(|i| i.focused);
        let binding = match id {
            0 => Some(egui::Key::ArrowLeft),
            1 => Some(egui::Key::ArrowRight),
            14..=26 => Some(NOTE_KEYS[id - 14]),
            _ => None,
        };
        let keyboard = binding.is_some_and(|key| ui.input(|i| i.key_down(key)));
        if binding.is_some_and(|key| ui.input(|i| i.key_pressed(key))) {
            self.pulse[id] = Instant::now() + Duration::from_millis(100);
            self.pulse_steps[id] = self.steps + 72_000_000;
        }
        if !focused {
            self.pulse[id] = Instant::now();
            self.pulse_steps[id] = 0;
        }
        // A slow host gives the guest at least 100 ms at up to 360 MHz,
        // including two issued core instructions per shared clock step, to scan
        // and debounce a click; the wall-clock pulse keeps visual feedback.
        let guest_pulse = self.steps < self.pulse_steps[id];
        let pressed =
            focused && (down || keyboard || Instant::now() < self.pulse[id] || guest_pulse);
        self.pressed[id] = pressed;
        let fill = if pressed {
            Color32::from_rgb(75, 65, 42)
        } else if response.hovered() {
            Color32::from_gray(53)
        } else {
            Color32::from_gray(38)
        };
        let radius = if note { 21. } else { 7. } * canvas.scale;
        canvas.painter.rect_filled(
            rect.translate(vec2(0., 3.) * canvas.scale),
            radius,
            Color32::BLACK,
        );
        canvas.painter.rect(
            rect,
            radius,
            fill,
            Stroke::new(canvas.scale, Color32::from_gray(86)),
            StrokeKind::Inside,
        );
        let inset = rect.shrink(4. * canvas.scale);
        canvas.painter.rect_stroke(
            inset,
            radius,
            Stroke::new(canvas.scale, Color32::from_gray(24)),
            StrokeKind::Inside,
        );
        if note {
            let y = if label.is_empty() {
                rect.center().y
            } else {
                rect.center().y - 9. * canvas.scale
            };
            canvas.painter.rect_filled(
                Rect::from_center_size(pos2(rect.center().x, y), vec2(6., 31.) * canvas.scale),
                3.,
                if pressed { ACCENT } else { INK },
            );
            if !label.is_empty() {
                canvas.painter.text(
                    pos2(rect.center().x, rect.bottom() - 19. * canvas.scale),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(11. * canvas.scale),
                    INK,
                );
            }
        } else {
            canvas.painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(12. * canvas.scale),
                if pressed { ACCENT } else { INK },
            );
        }
        response.on_hover_text(if note {
            format!("Note {} · hold to press", id - 14 + 53)
        } else {
            format!("{label} · hold to press")
        });
    }
    fn panel(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let scale = (available.x / 1160.).min(available.y / 700.).max(0.1);
        let size = vec2(1160., 700.) * scale;
        let (area, _) = ui.allocate_exact_size(vec2(available.x, size.y), Sense::hover());
        let canvas = Canvas {
            painter: ui.painter().clone(),
            origin: pos2(
                area.center().x - size.x / 2. + 20. * scale,
                area.top() + 12. * scale,
            ),
            scale,
        };
        let c = &canvas;
        c.box_at([8., 14., 1120., 662.], 65., Color32::from_black_alpha(90));
        c.box_at([0., 0., 1120., 660.], 65., Color32::from_gray(85));
        c.box_at([3., 3., 1114., 651.], 62., Color32::from_gray(23));
        c.box_at([7., 6., 1106., 640.], 58., Color32::from_gray(34));
        c.painter.rect_stroke(
            c.rect([11., 10., 1098., 631.]),
            53. * scale,
            Stroke::new(scale, Color32::from_gray(48)),
            StrokeKind::Inside,
        );
        for (x, y, name) in [
            (91., 106., "MASTER"),
            (208., 106., "SELECT"),
            (91., 224., "PRESETS"),
            (208., 224., "ALGORITHM"),
            (632., 106., "KNOB1"),
            (758., 106., "KNOB2"),
            (884., 106., "KNOB3"),
            (1010., 106., "KNOB4"),
        ] {
            c.knob(ui, x, y, name);
        }
        c.box_at([290., 52., 270., 272.], 34., Color32::from_gray(8));
        c.box_at([310., 72., 230., 230.], 3., Color32::BLACK);
        if let Some(texture) = &self.texture {
            c.painter.image(
                texture.id(),
                c.rect([315., 77., 220., 220.]),
                Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                Color32::WHITE,
            );
        }
        c.box_at([65., 286., 181., 56.], 12., Color32::from_gray(17));
        for (id, label) in [(0, "OCT−"), (1, "OCT+")] {
            self.key(
                ui,
                c,
                id,
                c.rect([75. + id as f32 * 82., 296., 70., 35.]),
                label,
                false,
            );
        }
        c.box_at([598., 186., 448., 157.], 23., Color32::from_gray(16));
        for (index, label) in [
            "FX",
            "SEL",
            "ENV",
            "LFO",
            "EDIT",
            "GLO",
            "HOME",
            "SAVE",
            "ARP",
            "SEQ",
            "PLAY\nSTOP",
            "REC",
        ]
        .iter()
        .enumerate()
        {
            self.key(
                ui,
                c,
                index + 2,
                c.rect([
                    615. + (index % 6) as f32 * 71.,
                    201. + (index / 6) as f32 * 70.,
                    57.,
                    55.,
                ]),
                label,
                false,
            );
        }
        c.box_at([40., 384., 1040., 238.], 40., Color32::from_gray(12));
        c.box_at([43., 387., 1034., 232.], 38., Color32::from_gray(40));
        let mut white_index = 0;
        let mut black_index = 0;
        let labels = [
            "OP1", "OP2", "OP3", "OP4", "OP5", "OP6", "PIT", "GLO", "MONO", "POLY", "",
        ];
        for note in 0..27 {
            let black = matches!(note, 1 | 3 | 5 | 8 | 10 | 13 | 15 | 17 | 20 | 22 | 25);
            let (x, y, label) = if black {
                let x = 58. + (white_index as f32 - 0.5) * 62.5;
                let label = labels[black_index];
                black_index += 1;
                (x, 405., label)
            } else {
                let x = 58. + white_index as f32 * 62.5;
                white_index += 1;
                (x, 510., "")
            };
            self.key(ui, c, note + 14, c.rect([x, y, 49., 94.]), label, true);
        }
    }
}

struct Canvas {
    painter: egui::Painter,
    origin: egui::Pos2,
    scale: f32,
}
impl Canvas {
    fn rect(&self, [x, y, w, h]: [f32; 4]) -> Rect {
        Rect::from_min_size(
            self.origin + vec2(x, y) * self.scale,
            vec2(w, h) * self.scale,
        )
    }
    fn box_at(&self, rect: [f32; 4], radius: f32, color: Color32) {
        self.painter
            .rect_filled(self.rect(rect), radius * self.scale, color);
    }
    fn knob(&self, ui: &mut egui::Ui, x: f32, y: f32, name: &str) {
        let p = self.origin + vec2(x, y) * self.scale;
        self.painter.text(
            p - vec2(0., 53.) * self.scale,
            Align2::CENTER_CENTER,
            name,
            FontId::proportional(14. * self.scale),
            INK,
        );
        self.painter.circle_filled(
            p + vec2(2., 4.) * self.scale,
            28. * self.scale,
            Color32::from_gray(12),
        );
        self.painter
            .circle_filled(p, 26. * self.scale, Color32::from_gray(66));
        self.painter
            .circle_filled(p, 22. * self.scale, Color32::from_gray(18));
        for i in 0..24 {
            let angle = i as f32 * std::f32::consts::TAU / 24.;
            let d = vec2(angle.sin(), angle.cos()) * self.scale;
            self.painter.line_segment(
                [p + d * 22., p + d * 25.],
                Stroke::new(self.scale, Color32::from_gray(135)),
            );
        }
        self.painter
            .circle_filled(p, 19. * self.scale, Color32::from_gray(35));
        self.painter.line_segment(
            [
                p + vec2(-5., -11.) * self.scale,
                p + vec2(-8., -18.) * self.scale,
            ],
            Stroke::new(3. * self.scale, INK),
        );
        ui.interact(
            Rect::from_center_size(p, vec2(56., 56.) * self.scale),
            egui::Id::new(name),
            Sense::hover(),
        )
        .on_hover_text("Rotary input is not implemented yet");
    }
}

impl eframe::App for Emulator {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // File dialogs and drops are handled outside any menu callback, so the
        // menu has closed.
        if std::mem::take(&mut self.pick_requested) {
            self.pick_firmware();
        }
        self.dropped_firmware(ctx);
        let title = window_title(self.path.as_deref());
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 25, 27))
                    .inner_margin(16.),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("FM-1").size(22.).strong().color(INK));
                    ui.add_space(12.);
                    let (name, hover) = match &self.path {
                        Some(path) => (
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            path.display().to_string(),
                        ),
                        None => ("no firmware".into(), "Load a firmware to start".into()),
                    };
                    ui.label(egui::RichText::new(name).color(Color32::from_gray(150)))
                        .on_hover_text(hover);
                    ui.add_space(8.);
                    self.firmware_controls(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(self.path.is_some(), egui::Button::new("Restart"))
                            .clicked()
                        {
                            self.reset();
                        }
                        if ui
                            .add_enabled(
                                self.loaded && self.fault.is_none(),
                                egui::Button::new(if self.paused { "Resume" } else { "Pause" }),
                            )
                            .clicked()
                        {
                            self.paused = !self.paused;
                        }
                        let (label, color) = if self.path.is_none() {
                            ("No firmware", Color32::from_gray(150))
                        } else if self.fault.is_some() {
                            ("Stopped", Color32::LIGHT_RED)
                        } else if self.paused {
                            ("Paused", ACCENT)
                        } else {
                            ("Running", Color32::from_rgb(122, 192, 159))
                        };
                        ui.colored_label(color, label);
                    });
                });
            });
        egui::TopBottomPanel::bottom("status").frame(egui::Frame::new().fill(Color32::from_rgb(22, 25, 27)).inner_margin(16.)).show(ctx, |ui| {
            if let Some(notice) = &self.notice {
                ui.colored_label(Color32::LIGHT_RED, notice);
            }
            if let Some(error) = &self.fault {
                ui.colored_label(Color32::LIGHT_RED, error);
                ui.label("This firmware needs additional emulation support. The LCD retains its last guest-written pixels.");
            } else {
                ui.label("Hold a key or button to press it · Arrow keys: octave · A W S E D R F G T H Y J K: notes");
                ui.weak("The LCD follows the loaded firmware. Audio playback and rotary input are not implemented.");
            }
        });
        // Receive worker snapshots, then send this frame's input below.
        // Losing focus immediately releases every matrix contact.
        if !ctx.input(|i| i.focused) {
            self.pressed.fill(false);
            self.pulse.fill(Instant::now());
            self.pulse_steps.fill(0);
        }
        self.refresh(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(31, 35, 37))
                    .inner_margin(10.),
            )
            .show(ctx, |ui| self.panel(ui));
        if self.path.is_none() {
            self.idle_prompt(ctx);
        }
        self.worker.input(self.pressed);
        self.keep_settings();
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
/// The command line.
#[derive(Debug, PartialEq)]
struct Args {
    /// The firmware on the command line; None: the saved one, or the picker.
    path: Option<PathBuf>,
    /// `--app-data`: keep the settings in the per-user application-data
    /// folder, as an installed app does.
    app_data: bool,
}
const USAGE: &str = "usage: emulator [--app-data] [firmware]";
fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Args, String> {
    let mut path = None;
    let mut app_data = false;
    for arg in args {
        if arg == "--app-data" {
            app_data = true;
        } else if path.replace(PathBuf::from(arg)).is_some() {
            return Err("expected one firmware path".into());
        }
    }
    Ok(Args { path, app_data })
}
fn main() -> eframe::Result {
    ui_console::attach_parent();
    if std::env::args_os().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}\nWithout a firmware the window opens with a Load firmware button.");
        return Ok(());
    }
    let bare = std::env::args_os().len() == 1;
    let args = parse_args(std::env::args_os().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\n{USAGE}");
        std::process::exit(2);
    });
    // The settings sit beside the emulator, except for an installed app.
    let exe = std::env::current_exe().unwrap_or_default();
    let launch = ui_app::Launch {
        bare,
        forced: args.app_data,
    };
    let settings_path = if ui_app::is_app_mode(&exe, launch) {
        match ui_app::data_dir(ui_app::Os::HOST, &ui_app::Env::from_process()) {
            Some(data) => ui_app::settings_in(&data),
            None => {
                eprintln!(
                    "no per-user data folder (HOME or APPDATA unset): using the emulator's folder"
                );
                ui_settings::default_path()
            }
        }
    } else {
        ui_settings::default_path()
    };
    let (settings, problems) = ui_settings::load(&settings_path, ui_settings::Settings::new());
    for problem in problems {
        eprintln!("window settings: {problem}");
    }
    // The command line wins; else the latest firmware, if it is still there.
    let first_firmware = args
        .path
        .clone()
        .or_else(|| settings.startup_firmware().map(PathBuf::from));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180., 830.])
            .with_min_inner_size([800., 600.]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "FM-1 Emulator",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut app = Emulator::new(None);
            app.apply_settings(settings, settings_path);
            if let Some(path) = first_firmware {
                app.load_firmware(path);
            }
            app.keep_settings();
            app.worker.read_stdin();
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo() -> Emulator {
        Emulator::new(Some(fixture()))
    }
    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/display/firmware.elf")
    }
    fn draw(app: &mut Emulator, ctx: &egui::Context, events: Vec<egui::Event>, focused: bool) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0., 0.), vec2(1180., 830.))),
                focused,
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.panel(ui));
            },
        );
        app.worker.input(app.pressed);
    }
    #[test]
    fn keyboard_events_reach_guest_pixels_and_focus_loss_releases_keys() {
        let mut app = demo();
        let ctx = egui::Context::default();
        draw(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::ArrowLeft,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            true,
        );
        assert!(app.pressed[0]);
        app.paused = true;
        app.refresh(&ctx);
        let stop =
            Firmware::load(app.path.as_ref().unwrap()).unwrap().symbols["display_frame_done"];
        app.worker.inspect(move |machine| {
            let cpu = machine.cpu.as_mut().unwrap();
            cpu.run(Some(stop), cpu.steps + 200_000, None).unwrap();
            assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0xf7cb00);
        });
        draw(&mut app, &ctx, vec![], false);
        assert!(app.pressed.iter().all(|&value| !value));
        app.refresh(&ctx);
        app.worker.inspect(move |machine| {
            let cpu = machine.cpu.as_mut().unwrap();
            cpu.step().unwrap(); // Leave the previous frame's stop address.
            cpu.run(Some(stop), cpu.steps + 200_000, None).unwrap();
            assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0x313031);
        });
    }
    #[test]
    fn pause_and_restart_control_the_actual_cpu() {
        let mut app = demo();
        let ctx = egui::Context::default();
        app.worker.inspect(|_| ()); // Wait for loading without advancing on the UI.
        std::thread::sleep(Duration::from_millis(20));
        let steps = app
            .worker
            .inspect(|machine| machine.cpu.as_ref().unwrap().steps);
        assert!(
            steps > 0,
            "guest execution must continue without drawing frames"
        );
        app.paused = true;
        app.refresh(&ctx);
        let paused = app.worker.inspect(|machine| {
            assert!(machine.paused);
            machine.cpu.as_ref().unwrap().steps
        });
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            app.worker
                .inspect(|machine| machine.cpu.as_ref().unwrap().steps),
            paused
        );
        app.reset();
        assert_eq!(app.steps, 0);
        assert!(!app.paused);
        assert!(!app.loaded);
        assert!(app.texture.is_none());
        app.worker.inspect(|machine| {
            assert!(!machine.paused);
            assert!(machine.fault.is_none());
            assert!(machine.cpu.is_some());
        });
    }
    #[test]
    fn a_short_keypress_survives_a_slow_host_until_guest_debounce_can_run() {
        let mut app = demo();
        let ctx = egui::Context::default();
        let key = |pressed| egui::Event::Key {
            key: egui::Key::ArrowLeft,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        draw(&mut app, &ctx, vec![key(true)], true);
        app.pulse[0] = Instant::now(); // Wall-clock pulse expired on a slow host.
        draw(&mut app, &ctx, vec![key(false)], true);
        assert!(app.pressed[0]);
        app.worker
            .inspect(|machine| machine.cpu.as_mut().unwrap().steps = 72_000_000);
        app.refresh(&ctx);
        draw(&mut app, &ctx, vec![], true);
        assert!(!app.pressed[0]);
    }
    #[test]
    fn unsupported_firmware_stops_without_fabricating_a_screen() {
        let mut app = demo();
        app.worker.inspect(|machine| {
            machine.cpu = Some(Cpu::new(Bus::new(vec![0xff, 0x00]).unwrap(), 0x02000120));
            machine.fault = None;
        });
        // Wait for execution rather than relying on host scheduling speed.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.worker.inspect(|machine| machine.fault.is_some()) {
            assert!(Instant::now() < deadline, "worker did not report the fault");
            std::thread::sleep(Duration::from_millis(1));
        }
        let ctx = egui::Context::default();
        app.refresh(&ctx);
        assert!(app
            .fault
            .as_ref()
            .unwrap()
            .contains("unsupported instruction"));
        let steps = app.worker.inspect(|machine| {
            let cpu = machine.cpu.as_ref().unwrap();
            assert!(cpu.bus.lcd.pixels.iter().all(|&pixel| pixel == 0));
            cpu.steps
        });
        app.refresh(&ctx);
        assert_eq!(
            app.worker
                .inspect(|machine| machine.cpu.as_ref().unwrap().steps),
            steps
        );
    }
    #[test]
    #[ignore = "requires FM1_STOCK_FWSC; measures GUI worker latency in release mode"]
    fn stock_click_reaches_a_new_guest_frame_through_the_gui_worker() {
        let path = std::env::var_os("FM1_STOCK_FWSC").expect("set FM1_STOCK_FWSC");
        let mut app = Emulator::new(Some(PathBuf::from(path)));
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(240);
        while app.steps < 3_000_000_000 {
            app.refresh(&ctx);
            assert!(app.fault.is_none(), "{:?}", app.fault);
            assert!(
                Instant::now() < deadline,
                "stock boot exceeded the host-time budget"
            );
            std::thread::sleep(Duration::from_millis(16));
        }
        app.paused = true;
        app.refresh(&ctx);
        let before = app.worker.inspect(|machine| {
            let cpu = machine.cpu.as_ref().unwrap();
            assert!(cpu.bus.screen_visible());
            cpu.bus.lcd.pixels.clone()
        });
        app.paused = false;
        app.pressed[2] = true; // FX, sent through the GUI's matrix mapping.
        let release = app.steps + 72_000_000;
        let start = Instant::now();
        app.refresh(&ctx);
        loop {
            std::thread::sleep(Duration::from_millis(16));
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "FX redraw exceeded ten seconds"
            );
            let Some(snapshot) = app.worker.snapshot() else {
                continue;
            };
            assert!(snapshot.fault.is_none(), "{:?}", snapshot.fault);
            app.steps = snapshot.steps;
            app.pressed[2] = app.steps < release;
            app.worker.input(app.pressed);
            if snapshot.pixels.is_some_and(|pixels| pixels != before) {
                eprintln!(
                    "Stock FX click to guest LCD snapshot: {:.3} seconds",
                    start.elapsed().as_secs_f64()
                );
                break;
            }
        }
    }
    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(std::ffi::OsString::from))
    }
    #[test]
    fn the_command_line_takes_an_optional_firmware() {
        assert_eq!(
            parse(&["a.fwsc"]).unwrap(),
            Args {
                path: Some(PathBuf::from("a.fwsc")),
                app_data: false
            }
        );
        assert!(parse(&["a.fwsc", "b.fwsc"]).is_err());
        // No firmware is fine: the window opens with the picker.
        assert_eq!(parse(&[]).unwrap().path, None);
        assert!(parse(&["--app-data"]).unwrap().app_data);
    }
    #[test]
    fn without_a_firmware_the_panel_idles_and_offers_to_load_one() {
        let mut app = Emulator::new(None);
        assert!(app.path.is_none() && !app.loaded);
        let ctx = egui::Context::default();
        let frame = |app: &mut Emulator| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0., 0.), vec2(1180., 830.))),
                    focused: true,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.panel(ui));
                    if app.path.is_none() {
                        app.idle_prompt(ctx);
                    }
                },
            );
        };
        for _ in 0..3 {
            frame(&mut app);
        }
        let prompt = egui::Id::new("open_firmware_prompt");
        assert!(
            ctx.memory(|memory| memory.area_rect(prompt)).is_some(),
            "the open-firmware prompt is drawn"
        );
        // Restart and the worker are harmless with nothing to run.
        app.reset();
        app.refresh(&ctx);
        assert!(!app.loaded && app.fault.is_none() && app.texture.is_none());
        assert_eq!(window_title(None), "FM-1 Emulator");
        // A non-firmware file is refused with a message, not loaded.
        app.open_firmware(PathBuf::from("/tmp/notes.txt"));
        assert!(
            app.path.is_none()
                && app
                    .notice
                    .as_deref()
                    .is_some_and(|n| n.contains("notes.txt"))
        );
    }
    #[test]
    fn firmware_files_are_recognised_by_extension() {
        for name in ["a.fwsc", "b.ELF", "c.bin", "/x y/z.Fwsc"] {
            assert!(is_firmware_file(std::path::Path::new(name)), "{name}");
        }
        for name in ["a.txt", "fwsc", "a.fwsc.zip", ""] {
            assert!(!is_firmware_file(std::path::Path::new(name)), "{name}");
        }
        assert_eq!(
            window_title(Some(std::path::Path::new("/fw/optimist-0.2.fwsc"))),
            "optimist-0.2.fwsc - FM-1 Emulator"
        );
    }
    #[test]
    fn loading_a_firmware_reboots_into_it_and_remembers_it() {
        let dir = std::env::temp_dir().join(format!("fm1-ui-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (alpha, beta) = (dir.join("alpha-1.0.elf"), dir.join("beta-1.0.elf"));
        std::fs::copy(fixture(), &alpha).unwrap();
        std::fs::copy(fixture(), &beta).unwrap();
        let settings = dir.join(ui_settings::FILE_NAME);
        let mut app = Emulator::new(None);
        app.apply_settings(ui_settings::Settings::new(), settings.clone());
        let ctx = egui::Context::default();
        let wait_loaded = |app: &mut Emulator| {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !app.loaded {
                app.refresh(&ctx);
                assert!(Instant::now() < deadline, "timed out waiting for a boot");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        app.load_firmware(alpha.clone());
        assert_eq!(
            window_title(app.path.as_deref()),
            "alpha-1.0.elf - FM-1 Emulator"
        );
        wait_loaded(&mut app);
        let generation = app.generation;
        // A power cycle into the other firmware: a new machine, from the start.
        app.load_firmware(beta.clone());
        assert!(app.generation > generation && !app.loaded);
        wait_loaded(&mut app);
        assert_eq!(app.path.as_deref(), Some(beta.as_path()));
        let booted = app.worker.inspect(|machine| machine.cpu.is_some());
        assert!(booted && app.fault.is_none());
        assert_eq!(app.recent, [beta.clone(), alpha.clone()]);
        // The latest firmware is on disk, for the next start.
        app.keep_settings();
        let (saved, problems) = ui_settings::load(&settings, ui_settings::Settings::new());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(saved.startup_firmware(), Some(beta.as_path()));
        app.load_firmware(alpha.clone());
        assert_eq!(app.recent, [alpha, beta]);
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
