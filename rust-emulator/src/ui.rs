// SPDX-License-Identifier: GPL-3.0-only
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
use fm1_emu::encoders::knob;
#[cfg(test)]
use fm1_emu::{bus::Bus, cpu::Cpu, firmware::Firmware};
mod host_audio;
mod ui_theme;
mod web_editor;
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
/// Where to draw the 240x240 LCD inside `area` (points) so that no guest pixel
/// is dropped: never fewer than 240 physical pixels (nearest-neighbour
/// downscaling skips rows and columns, so "TRACK" read "IRALK"), an integer
/// scale when it fills at least 3/4 of the area, and snapped to the pixel grid.
fn lcd_rect(area: Rect, pixels_per_point: f32) -> Rect {
    const LCD: f32 = 240.;
    let available = area.width().min(area.height()) * pixels_per_point;
    let whole = (available / LCD).floor();
    let pixels = if available < LCD {
        LCD
    } else if whole * LCD >= 0.75 * available {
        whole * LCD
    } else {
        available.floor()
    };
    let size = pixels / pixels_per_point;
    let min =
        ((area.center() - vec2(size, size) / 2.) * pixels_per_point).round() / pixels_per_point;
    Rect::from_min_size(min, vec2(size, size))
}
/// Drawn knobs: position, label, matrix encoder (None: the MASTER pot) and
/// the keys that turn them (counter-clockwise / clockwise).
const KNOBS: [(f32, f32, &str, Option<usize>, &str); 8] = [
    (91., 106., "MASTER", None, "N / M"),
    (208., 106., "SELECT", Some(knob::SELECT), "[ / ]"),
    (91., 224., "PRESETS", Some(knob::PRESETS), "9 / 0"),
    (208., 224., "ALGORITHM", Some(knob::ALGORITHM), "- / ="),
    (632., 106., "KNOB1", Some(knob::KNOB1), "1 / 2"),
    (758., 106., "KNOB2", Some(knob::KNOB1 + 1), "3 / 4"),
    (884., 106., "KNOB3", Some(knob::KNOB1 + 2), "5 / 6"),
    (1010., 106., "KNOB4", Some(knob::KNOB1 + 3), "7 / 8"),
];
const KNOB_KEYS: [(egui::Key, egui::Key); 8] = [
    (egui::Key::N, egui::Key::M),
    (egui::Key::OpenBracket, egui::Key::CloseBracket),
    (egui::Key::Num9, egui::Key::Num0),
    (egui::Key::Minus, egui::Key::Equals),
    (egui::Key::Num1, egui::Key::Num2),
    (egui::Key::Num3, egui::Key::Num4),
    (egui::Key::Num5, egui::Key::Num6),
    (egui::Key::Num7, egui::Key::Num8),
];
/// Pointer travel (points of drag, or half-points of scroll) per encoder click.
const DETENT_PX: f32 = 12.;
/// Encoder clicks per turn as drawn (the pointer moves 15 degrees a click).
const DETENT_ANGLE: f32 = std::f32::consts::TAU / 24.;
/// Instruction clock choices: None follows the firmware's system clock (the
/// accurate default); a lower rate gives the guest fewer instructions per
/// second of guest time, so a light firmware can play in real time.
const CLOCKS: [(Option<u32>, &str); 6] = [
    (None, "Firmware clock"),
    (Some(24), "24 MHz"),
    (Some(48), "48 MHz"),
    (Some(96), "96 MHz"),
    (Some(192), "192 MHz"),
    (Some(312), "312 MHz"),
];
/// The ADC model's MASTER reading at reset.
const MASTER_DEFAULT: u16 = 512;
/// MASTER pointer: -135..+135 degrees over the ADC range 0..=1023.
fn master_angle(master: u16) -> f32 {
    (master as f32 / 1023. - 0.5) * 1.5 * std::f32::consts::PI
}
const INK: Color32 = Color32::from_rgb(190, 194, 193);
const ACCENT: Color32 = Color32::from_rgb(231, 193, 91);

struct Emulator {
    path: PathBuf,
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
    /// MASTER potentiometer as the ADC reads it (0..=1023).
    master: u16,
    /// Pointer angle per drawn knob (radians, 0 = up), unspent drag/scroll
    /// and the knob centres of the last drawn frame.
    knob_angle: [f32; 8],
    knob_accum: [f32; 8],
    knob_centre: [egui::Pos2; 8],
    /// Instruction clock in MHz; None follows the firmware.
    clock_mhz: Option<u32>,
    /// Host playback; None in tests or when no device opens (`audio_error`).
    audio: Option<host_audio::HostAudio>,
    audio_error: Option<String>,
    /// Guest audio against real time, sampled about once a second.
    speed: Option<f64>,
    speed_mark: (Instant, u64),
    /// The firmware's web editor and its MIDI bridge (off without one).
    web: web_editor::WebEditor,
    /// The panel's colours (an index into `ui_theme::THEMES`).
    theme: usize,
}
impl Emulator {
    fn new(path: PathBuf) -> Self {
        let mut app = Self {
            path,
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
            master: MASTER_DEFAULT,
            knob_angle: [0.; 8],
            knob_accum: [0.; 8],
            knob_centre: [pos2(0., 0.); 8],
            clock_mhz: None,
            audio: None,
            audio_error: None,
            speed: None,
            speed_mark: (Instant::now(), 0),
            web: web_editor::WebEditor::disabled("No web editor"),
            theme: 0,
        };
        app.knob_angle[0] = master_angle(app.master);
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
        self.worker.restart(self.generation, self.path.clone());
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
        let (since, frames) = self.speed_mark;
        let elapsed = since.elapsed().as_secs_f64();
        if snapshot.frames < frames || self.paused {
            self.speed = None;
            self.speed_mark = (Instant::now(), snapshot.frames);
        } else if elapsed >= 1.0 {
            let guest = (snapshot.frames - frames) as f64 / fm1_emu::audio::SAMPLE_RATE as f64;
            self.speed = (snapshot.frames > 0).then_some(guest / elapsed);
            self.speed_mark = (Instant::now(), snapshot.frames);
        }
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
    /// Steps that make at least 100 ms of guest time for a click: two
    /// issued core instructions per shared clock step at the selected clock,
    /// or 360 MHz (the firmware's own clock is not known here). A fixed
    /// 72 M steps held a click 0.4-0.75 s of guest time at 96 MHz.
    fn pulse_len(&self) -> u64 {
        u64::from(self.clock_mhz.unwrap_or(360)) * 200_000
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
            self.pulse_steps[id] = self.steps + self.pulse_len();
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
            self.pulse_steps[id] = self.steps + self.pulse_len();
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
        let theme = canvas.theme;
        let fill = if pressed {
            theme.key_pressed
        } else if response.hovered() {
            theme.key_hover
        } else {
            theme.key
        };
        let ink = if pressed {
            theme.pressed
        } else {
            theme.key_label
        };
        let radius = if note { 21. } else { 7. } * canvas.scale;
        canvas.painter.rect_filled(
            rect.translate(vec2(0., 3.) * canvas.scale),
            radius,
            theme.key_shadow,
        );
        canvas.painter.rect(
            rect,
            radius,
            fill,
            Stroke::new(canvas.scale, theme.key_edge),
            StrokeKind::Inside,
        );
        let inset = rect.shrink(4. * canvas.scale);
        canvas.painter.rect_stroke(
            inset,
            radius,
            Stroke::new(canvas.scale, theme.key_inset),
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
                if pressed { theme.pressed } else { theme.slot },
            );
            if !label.is_empty() {
                canvas.painter.text(
                    pos2(rect.center().x, rect.bottom() - 19. * canvas.scale),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(11. * canvas.scale),
                    theme.key_label,
                );
            }
        } else {
            canvas.painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(12. * canvas.scale),
                ink,
            );
        }
        response.on_hover_text(if note {
            format!("Note {} · hold to press", id - 14 + 53)
        } else {
            format!("{label} · hold to press")
        });
    }
    /// Pointer movement on drawn knob `index`: whole encoder clicks are queued
    /// for the guest and the rest is kept; MASTER moves the ADC value.
    fn turn_knob(&mut self, index: usize, encoder: Option<usize>, amount: f32) {
        if amount == 0. {
            return;
        }
        match encoder {
            None => {
                // A pot: the pointer follows the turn and stops at both ends.
                let turn = amount / DETENT_PX * DETENT_ANGLE;
                let value = self.master as f32
                    + self.knob_accum[index]
                    + turn / (1.5 * std::f32::consts::PI) * 1023.;
                let value = value.clamp(0., 1023.);
                self.master = value as u16;
                self.knob_accum[index] = value - self.master as f32; // keep slow drags
                self.knob_angle[index] = master_angle(self.master);
                self.worker.master(self.master);
            }
            Some(e) => {
                self.knob_accum[index] += amount;
                let detents = (self.knob_accum[index] / DETENT_PX).trunc();
                if detents != 0. {
                    self.knob_accum[index] -= detents * DETENT_PX;
                    self.worker.turn(e, detents as i32);
                    self.knob_angle[index] += detents * DETENT_ANGLE;
                }
            }
        }
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
            theme: &ui_theme::THEMES[self.theme],
        };
        let c = &canvas;
        let theme = c.theme;
        c.box_at([8., 14., 1120., 662.], 65., theme.shadow);
        c.box_at([0., 0., 1120., 660.], 65., theme.rim);
        c.box_at([3., 3., 1114., 651.], 62., theme.edge);
        c.box_at([7., 6., 1106., 640.], 58., theme.body);
        c.painter.rect_stroke(
            c.rect([11., 10., 1098., 631.]),
            53. * scale,
            Stroke::new(scale, theme.body_line),
            StrokeKind::Inside,
        );
        for (index, (x, y, name, encoder, keys)) in KNOBS.iter().enumerate() {
            let hint = match encoder {
                Some(_) => format!("{name} · drag around or scroll to turn · keys {keys}"),
                None => format!("{name} volume · drag around or scroll · keys {keys}"),
            };
            let response = c.knob(ui, *x, *y, name, self.knob_angle[index], &hint);
            // Circling the knob turns it by the pointer's angle around its
            // centre (clockwise +); scroll up or the right-hand key: clockwise.
            let mut amount = 0.;
            let centre = c.origin + vec2(*x, *y) * c.scale;
            self.knob_centre[index] = centre;
            if let Some(now) = response
                .interact_pointer_pos()
                .filter(|_| response.dragged())
            {
                let (from, to) = (now - response.drag_delta() - centre, now - centre);
                if from.length() > 6. * c.scale && to.length() > 6. * c.scale {
                    let mut turn = to.x.atan2(-to.y) - from.x.atan2(-from.y);
                    if turn > std::f32::consts::PI {
                        turn -= std::f32::consts::TAU;
                    } else if turn < -std::f32::consts::PI {
                        turn += std::f32::consts::TAU;
                    }
                    amount += turn / DETENT_ANGLE * DETENT_PX;
                }
            }
            if response.hovered() {
                amount += ui.input(|i| i.raw_scroll_delta.y) * 0.5;
            }
            let (down, up) = KNOB_KEYS[index];
            amount += ui.input(|i| {
                (i.key_pressed(up) as i32 - i.key_pressed(down) as i32) as f32 * DETENT_PX
            });
            self.turn_knob(index, *encoder, amount);
        }
        c.box_at([290., 52., 270., 272.], 34., theme.bezel);
        c.box_at([310., 72., 230., 230.], 3., theme.glass);
        if let Some(texture) = &self.texture {
            c.painter.image(
                texture.id(),
                lcd_rect(c.rect([315., 77., 220., 220.]), ui.ctx().pixels_per_point()),
                Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                Color32::WHITE,
            );
        }
        c.box_at([65., 286., 181., 56.], 12., theme.oct_recess);
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
        c.box_at([598., 186., 448., 157.], 23., theme.button_recess);
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
        c.box_at([40., 384., 1040., 238.], 40., theme.keybed_rim);
        c.box_at([43., 387., 1034., 232.], 38., theme.keybed);
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
    theme: &'static ui_theme::Theme,
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
    fn knob(
        &self,
        ui: &mut egui::Ui,
        x: f32,
        y: f32,
        name: &str,
        angle: f32,
        hint: &str,
    ) -> egui::Response {
        let p = self.origin + vec2(x, y) * self.scale;
        self.painter.text(
            p - vec2(0., 53.) * self.scale,
            Align2::CENTER_CENTER,
            name,
            FontId::proportional(14. * self.scale),
            self.theme.label,
        );
        self.painter.circle_filled(
            p + vec2(2., 4.) * self.scale,
            28. * self.scale,
            self.theme.knob_shadow,
        );
        self.painter
            .circle_filled(p, 26. * self.scale, self.theme.knob_rim);
        self.painter
            .circle_filled(p, 22. * self.scale, self.theme.knob_skirt);
        for i in 0..24 {
            let angle = i as f32 * std::f32::consts::TAU / 24.;
            let d = vec2(angle.sin(), angle.cos()) * self.scale;
            self.painter.line_segment(
                [p + d * 22., p + d * 25.],
                Stroke::new(self.scale, self.theme.knob_tick),
            );
        }
        self.painter
            .circle_filled(p, 19. * self.scale, self.theme.knob_cap);
        let pointer = vec2(angle.sin(), -angle.cos()) * self.scale;
        self.painter.line_segment(
            [p + pointer * 11., p + pointer * 18.],
            Stroke::new(3. * self.scale, self.theme.knob_pointer),
        );
        ui.interact(
            Rect::from_center_size(p, vec2(56., 56.) * self.scale),
            egui::Id::new(name),
            Sense::drag(),
        )
        .on_hover_text(hint)
    }
}

impl eframe::App for Emulator {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                    ui.label(
                        egui::RichText::new(
                            self.path.file_name().unwrap_or_default().to_string_lossy(),
                        )
                        .color(Color32::from_gray(150)),
                    )
                    .on_hover_text(self.path.display().to_string());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Restart").clicked() {
                            self.reset();
                        }
                        let editor =
                            ui.add_enabled(self.web.active(), egui::Button::new("Open editor"));
                        let editor = match (self.web.url(), self.web.unavailable()) {
                            (Some(url), _) => editor
                                .on_hover_text(format!("Open {url} in the default browser")),
                            (None, reason) => {
                                editor.on_disabled_hover_text(reason.unwrap_or_default())
                            }
                        };
                        if editor.clicked() {
                            if let Err(error) = self.web.open_in_browser() {
                                eprintln!("{error}");
                            }
                        }
                        let before = self.clock_mhz;
                        let selected = CLOCKS
                            .iter()
                            .find(|(mhz, _)| *mhz == self.clock_mhz)
                            .map_or("Custom clock".into(), |(_, label)| label.to_string());
                        egui::ComboBox::from_id_salt("clock")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for (mhz, label) in CLOCKS {
                                    ui.selectable_value(&mut self.clock_mhz, mhz, label);
                                }
                            })
                            .response
                            .on_hover_text("Instructions per second of guest time. Lower rates let light firmware play in real time; timers, audio and USB keep their own clocks.");
                        if self.clock_mhz != before {
                            self.worker.clock(self.clock_mhz.map(|mhz| mhz * 1_000_000));
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
                        let (label, color) = if self.fault.is_some() {
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
            if let Some(error) = &self.fault {
                ui.colored_label(Color32::LIGHT_RED, error);
                ui.label("This firmware needs additional emulation support. The LCD retains its last guest-written pixels.");
            } else {
                ui.label("Hold a key or button to press it · Arrow keys: octave · A W S E D R F G T H Y J K: notes · Knobs: drag around, scroll, or 1–0 - = [ ] (MASTER: N / M)");
                let speed = self
                    .speed
                    .map_or(String::from("no guest audio yet"), |s| {
                        format!("guest audio at {:.0}% of real time", s * 100.)
                    });
                let audio = match (&self.audio, &self.audio_error) {
                    (Some(audio), _) => format!(
                        "output {} Hz, {} dropouts",
                        audio.device_rate,
                        audio.underruns()
                    ),
                    (None, Some(error)) => format!("no audio output: {error}"),
                    (None, None) => String::from("no audio output"),
                };
                ui.weak(format!("{speed} · {audio} · below 100% the sound breaks up"));
                self.web.refresh();
                ui.horizontal(|ui| {
                    if self.web.active() {
                        let lit = if self.web.busy() {
                            ACCENT
                        } else {
                            Color32::from_gray(70)
                        };
                        ui.colored_label(lit, "●").on_hover_text("MIDI traffic");
                    }
                    ui.weak(self.web.status());
                });
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
        self.worker.input(self.pressed);
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
/// The command line.
#[derive(Debug, PartialEq)]
struct Args {
    path: PathBuf,
    clock_mhz: Option<u32>,
    /// Web editor files to serve instead of FIRMWARE-ui.zip.
    ui: Option<PathBuf>,
}
const USAGE: &str = "usage: emulator [--cpu-mhz N] [--ui DIR] <firmware>";
/// `[--cpu-mhz N] [--ui DIR] FIRMWARE`.
fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Args, String> {
    let mut path = None;
    let mut clock = None;
    let mut ui = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--ui" {
            ui = Some(PathBuf::from(args.next().ok_or("--ui needs a directory")?));
        } else if arg == "--cpu-mhz" || arg.to_str().is_some_and(|a| a.starts_with("--cpu-mhz=")) {
            // `--cpu-mhz N`, or `--cpu-mhz=N` as the fork's launcher passes it.
            let value = match arg.to_str().and_then(|a| a.strip_prefix("--cpu-mhz=")) {
                Some(value) => value.into(),
                None => args.next().ok_or("--cpu-mhz needs a value")?,
            };
            let mhz = value
                .to_str()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|mhz| (1..=1000).contains(mhz))
                .ok_or("--cpu-mhz takes 1..1000")?;
            clock = Some(mhz);
        } else if path.replace(PathBuf::from(arg)).is_some() {
            return Err("expected one firmware path".into());
        }
    }
    Ok(Args {
        path: path.ok_or(USAGE)?,
        clock_mhz: clock,
        ui,
    })
}
fn main() -> eframe::Result {
    let args = parse_args(std::env::args_os().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
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
            let mut app = Emulator::new(args.path.clone());
            app.clock_mhz = args.clock_mhz;
            app.worker.clock(args.clock_mhz.map(|mhz| mhz * 1_000_000));
            app.web =
                web_editor::WebEditor::new(&args.path, args.ui.as_deref(), fm1_emu::web::ADDRESS);
            match app.web.url() {
                Some(url) => eprintln!("web editor at {url}"),
                None => eprintln!("{}", app.web.unavailable().unwrap_or_default()),
            }
            app.worker.web(app.web.hub());
            match host_audio::HostAudio::open() {
                Ok(audio) => {
                    app.worker.audio(Some(audio.queue.clone()));
                    app.audio = Some(audio);
                }
                Err(error) => app.audio_error = Some(error),
            }
            app.reset(); // Apply the clock, audio and editor from the first instruction.
            app.worker.read_stdin();
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo() -> Emulator {
        Emulator::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures/display/firmware.elf"),
        )
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
        let stop = Firmware::load(&app.path).unwrap().symbols["display_frame_done"];
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
    fn circling_a_knob_clicks_its_encoder_and_master_stops_at_its_ends() {
        let mut app = demo();
        let ctx = egui::Context::default();
        // Paused, the guest cannot play the queued clicks out before the check.
        app.paused = true;
        app.refresh(&ctx);
        draw(&mut app, &ctx, vec![], true);
        let drag = |app: &mut Emulator, index: usize, degrees: i32| {
            let centre = app.knob_centre[index];
            let at = |d: i32| {
                let a = (d as f32).to_radians();
                centre + vec2(a.sin(), -a.cos()) * 22.
            };
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            draw(app, &ctx, vec![egui::Event::PointerMoved(at(0))], true);
            draw(app, &ctx, vec![button(at(0), true)], true);
            let step = degrees.signum() * 5;
            for d in (step..=degrees)
                .step_by(5)
                .chain((degrees..=step).rev().step_by(5))
            {
                draw(app, &ctx, vec![egui::Event::PointerMoved(at(d))], true);
            }
            draw(app, &ctx, vec![button(at(degrees), false)], true);
        };
        // A quarter turn clockwise around KNOB1 is six 15-degree clicks.
        drag(&mut app, 4, 90);
        assert!(
            (app.knob_angle[4] - 6. * DETENT_ANGLE).abs() < 1e-4,
            "{}",
            app.knob_angle[4]
        );
        // Endless: two full turns back keep counting.
        drag(&mut app, 4, -720);
        assert!(
            (app.knob_angle[4] + 42. * DETENT_ANGLE).abs() < 1e-4,
            "{}",
            app.knob_angle[4]
        );
        let pending = app.worker.inspect(|machine| machine.encoders.clone());
        assert!(pending.busy(), "the worker received the clicks");
        // MASTER is a pot: a big clockwise turn pins it at full scale.
        drag(&mut app, 0, 300);
        assert_eq!(app.master, 1023);
        let master = app
            .worker
            .inspect(|machine| machine.cpu.as_ref().unwrap().bus.devices.adc.master);
        assert_eq!(master, 1023);
    }
    #[test]
    fn the_command_line_takes_a_firmware_and_an_instruction_clock() {
        let parse = |args: &[&str]| parse_args(args.iter().map(std::ffi::OsString::from));
        let args = |clock_mhz, ui: Option<&str>| Args {
            path: PathBuf::from("a.fwsc"),
            clock_mhz,
            ui: ui.map(PathBuf::from),
        };
        assert_eq!(parse(&["a.fwsc"]), Ok(args(None, None)));
        assert_eq!(
            parse(&["--cpu-mhz", "24", "a.fwsc"]),
            Ok(args(Some(24), None))
        );
        assert_eq!(
            parse(&["a.fwsc", "--ui", "web"]),
            Ok(args(None, Some("web")))
        );
        assert!(parse(&["a.fwsc", "--ui"]).is_err());
        assert!(parse(&["--cpu-mhz", "0", "a.fwsc"]).is_err());
        assert_eq!(parse(&["--cpu-mhz=96", "a.fwsc"]), Ok(args(Some(96), None)));
        assert!(parse(&["a.fwsc", "b.fwsc"]).is_err());
        assert!(parse(&[]).is_err());
    }
    #[test]
    fn the_worker_paces_the_guest_by_the_audio_queue() {
        let mut app = demo();
        let queue = host_audio::AudioQueue::default();
        app.worker.audio(Some(queue.clone()));
        app.reset();
        app.worker.inspect(|machine| {
            let cpu = machine.cpu.as_mut().unwrap();
            cpu.bus.audio.frames = 1; // The guest has started streaming.
        });
        queue.push(std::iter::repeat_n([0, 0], host_audio::TARGET_FRAMES));
        let steps = app
            .worker
            .inspect(|machine| machine.cpu.as_ref().unwrap().steps);
        std::thread::sleep(Duration::from_millis(30));
        let paced = app
            .worker
            .inspect(|machine| machine.cpu.as_ref().unwrap().steps);
        assert_eq!(paced, steps, "a full queue holds the guest");
        queue.clear();
        std::thread::sleep(Duration::from_millis(30));
        let resumed = app
            .worker
            .inspect(|machine| machine.cpu.as_ref().unwrap().steps);
        assert!(resumed > paced, "an emptied queue lets it run");
    }
    #[test]
    fn the_lcd_is_never_downscaled_and_sits_on_the_pixel_grid() {
        let area = |side: f32| Rect::from_min_size(pos2(10.3, 20.7), vec2(side, side));
        // 1x display, 220-point box: drawn at 240 pixels, not squeezed to 220.
        assert_eq!(lcd_rect(area(220.), 1.).width(), 240.);
        // Retina, 220 points = 440 px: 1x would fill only 55%, so use all 440.
        assert_eq!(lcd_rect(area(220.), 2.).width(), 220.);
        // Retina, 250 points = 500 px: 2x (480 px) fills 96%: integer scale.
        assert_eq!(lcd_rect(area(250.), 2.).width(), 240.);
        for ppp in [1., 1.5, 2.] {
            let r = lcd_rect(area(233.), ppp);
            assert!(r.width() * ppp >= 240.);
            assert_eq!((r.min.x * ppp).fract(), 0.);
            assert_eq!((r.min.y * ppp).fract(), 0.);
        }
    }
    #[test]
    #[ignore = "requires FM1_STOCK_FWSC; measures GUI worker latency in release mode"]
    fn stock_click_reaches_a_new_guest_frame_through_the_gui_worker() {
        let path = std::env::var_os("FM1_STOCK_FWSC").expect("set FM1_STOCK_FWSC");
        let mut app = Emulator::new(PathBuf::from(path));
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
}
