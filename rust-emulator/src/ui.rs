// SPDX-License-Identifier: GPL-3.0-only
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
#[cfg(test)]
use fm1_emu::bus::Bus;
use fm1_emu::{cpu::Cpu, firmware::Firmware};
mod ui_audio;
mod ui_knobs;
use std::{
    io::{self, Write},
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
/// Drawn knobs: position, label, matrix encoder (None: the MASTER pot) and
/// the keys that turn them (counter-clockwise / clockwise).
const KNOBS: [(f32, f32, &str, Option<usize>, &str); 8] = [
    (91., 106., "MASTER", None, "N / M"),
    (208., 106., "SELECT", Some(ui_knobs::knob::SELECT), "[ / ]"),
    (91., 224., "PRESETS", Some(ui_knobs::knob::PRESETS), "9 / 0"),
    (208., 224., "ALGORITHM", Some(ui_knobs::knob::ALGORITHM), "- / ="),
    (632., 106., "KNOB1", Some(ui_knobs::knob::KNOB1), "1 / 2"),
    (758., 106., "KNOB2", Some(ui_knobs::knob::KNOB1 + 1), "3 / 4"),
    (884., 106., "KNOB3", Some(ui_knobs::knob::KNOB1 + 2), "5 / 6"),
    (1010., 106., "KNOB4", Some(ui_knobs::knob::KNOB1 + 3), "7 / 8"),
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
/// MASTER pointer: -135..+135 degrees over the ADC range 0..=1023.
fn master_angle(master: u16) -> f32 {
    (master as f32 / 1023. - 0.5) * 1.5 * std::f32::consts::PI
}
const INK: Color32 = Color32::from_rgb(190, 194, 193);
const ACCENT: Color32 = Color32::from_rgb(231, 193, 91);

struct Emulator {
    path: PathBuf,
    cpu: Option<Cpu>,
    fault: Option<String>,
    paused: bool,
    texture: Option<egui::TextureHandle>,
    pressed: [bool; 41],
    pulse: [Instant; 41],
    pulse_steps: [u64; 41],
    /// Host playback; None in tests or when no device opens (`audio_error`).
    audio: Option<ui_audio::HostAudio>,
    audio_error: Option<String>,
    encoders: ui_knobs::Encoders,
    /// MASTER potentiometer as the ADC reads it (0..=1023).
    master: u16,
    /// Pointer angle per drawn knob (radians, 0 = up) and unspent drag/scroll.
    knob_angle: [f32; 8],
    knob_accum: [f32; 8],
    /// Where each knob was last drawn (screen points).
    knob_centre: [egui::Pos2; 8],
    /// Guest speed against real time (24 M instructions/s), sampled each second.
    speed: Option<f64>,
    speed_mark: (Instant, u64),
}
impl Emulator {
    fn new(path: PathBuf) -> Self {
        let mut app = Self {
            path,
            cpu: None,
            fault: None,
            paused: false,
            texture: None,
            pressed: [false; 41],
            pulse: [Instant::now(); 41],
            pulse_steps: [0; 41],
            audio: None,
            audio_error: None,
            encoders: ui_knobs::Encoders::default(),
            master: 768,
            knob_angle: [0.; 8],
            knob_accum: [0.; 8],
            knob_centre: [egui::Pos2::ZERO; 8],
            speed: None,
            speed_mark: (Instant::now(), 0),
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
        self.encoders = ui_knobs::Encoders::default();
        self.speed = None;
        self.speed_mark = (Instant::now(), 0);
        if let Some(audio) = &self.audio {
            audio.clear();
        }
        match Firmware::load(&self.path).and_then(|firmware| {
            let mut cpu = Cpu::new(firmware.bus()?, firmware.entry);
            cpu.r[0] = 0x01c7fe08;
            Ok(cpu)
        }) {
            Ok(cpu) => {
                self.cpu = Some(cpu);
                self.fault = None;
            }
            Err(error) => {
                self.cpu = None;
                self.fault = Some(error);
            }
        }
    }
    fn run_slice(&mut self, ctx: &egui::Context) {
        if let Some(cpu) = &mut self.cpu {
            for (row, ids) in KEYMAP.iter().enumerate() {
                for (column, &id) in ids.iter().enumerate() {
                    if id >= 0 {
                        cpu.bus
                            .devices
                            .gpio
                            .press(column, row + 1, self.pressed[id as usize])
                            .unwrap();
                    }
                }
            }
            cpu.bus.devices.adc.master = self.master;
            if !self.paused && self.fault.is_none() {
                // Keep the UI responsive even if guest code spins forever.
                // While the guest streams audio to a host device, its pace is
                // the playback queue (real time when the host keeps up);
                // otherwise an instruction budget of about one frame of guest
                // time. Neither is a cycle-accuracy claim.
                let deadline = Instant::now() + Duration::from_millis(14);
                let paced = self.audio.is_some() && cpu.bus.audio.frames > 0;
                let budget = if paced { u64::MAX } else { 400_000 };
                let mut i = 0u64;
                while i < budget {
                    if i % 1024 == 0 {
                        let contacts = self
                            .encoders
                            .update(|column| cpu.bus.devices.gpio.column_scans(column));
                        for (e, (a, b)) in contacts.into_iter().enumerate() {
                            let [ac, ar, bc, br] = ui_knobs::CONTACTS[e];
                            let gpio = &mut cpu.bus.devices.gpio;
                            gpio.press(ac, ar, a).unwrap();
                            gpio.press(bc, br, b).unwrap();
                        }
                        if let Some(audio) = &self.audio {
                            audio.push(cpu.bus.audio.samples.drain(..));
                            if paced && audio.queued() >= ui_audio::TARGET_FRAMES {
                                break;
                            }
                        }
                        if Instant::now() >= deadline {
                            break;
                        }
                    }
                    if let Err(error) = cpu.step() {
                        self.fault = Some(error.to_string());
                        break;
                    }
                    i += 1;
                }
                if let Some(audio) = &self.audio {
                    audio.push(cpu.bus.audio.samples.drain(..));
                }
                let (since, steps) = self.speed_mark;
                let elapsed = since.elapsed().as_secs_f64();
                if elapsed >= 1.0 {
                    self.speed = Some((cpu.steps - steps) as f64 / 24e6 / elapsed);
                    self.speed_mark = (Instant::now(), cpu.steps);
                }
            }
            if !cpu.bus.usb.serial.is_empty() {
                let bytes: Vec<u8> = cpu.bus.usb.serial.drain(..).collect();
                let mut stdout = io::stdout().lock();
                if let Err(error) = stdout.write_all(&bytes).and_then(|_| stdout.flush()) {
                    self.fault = Some(format!("USB serial stdout: {error}"));
                }
            }
            let visible = cpu.bus.screen_visible();
            let pixels = cpu
                .bus
                .lcd
                .pixels
                .iter()
                .map(|&rgb| {
                    if visible {
                        Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
                    } else {
                        Color32::BLACK
                    }
                })
                .collect();
            let image = egui::ColorImage {
                size: [240, 240],
                pixels,
            };
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::NEAREST);
            } else {
                self.texture =
                    Some(ctx.load_texture("Guest LCD", image, egui::TextureOptions::NEAREST));
            }
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
            self.pulse_steps[id] = self.cpu.as_ref().map_or(0, |cpu| cpu.steps + 2_400_000);
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
            self.pulse_steps[id] = self.cpu.as_ref().map_or(0, |cpu| cpu.steps + 2_400_000);
        }
        if !focused {
            self.pulse[id] = Instant::now();
            self.pulse_steps[id] = 0;
        }
        // A slow host still gives the guest 100 ms of oscillator time to scan
        // and debounce a click; the wall-clock pulse keeps visual feedback.
        let guest_pulse = self
            .cpu
            .as_ref()
            .is_some_and(|cpu| cpu.steps < self.pulse_steps[id]);
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
    /// Pointer movement on drawn knob `index`: whole encoder clicks are queued
    /// for the guest, the rest is kept; MASTER moves the ADC value directly.
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
            }
            Some(e) => {
                self.knob_accum[index] += amount;
                let detents = (self.knob_accum[index] / DETENT_PX).trunc();
                if detents != 0. {
                    self.knob_accum[index] -= detents * DETENT_PX;
                    self.encoders.turn(e, detents as i32);
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
        for (index, (x, y, name, encoder, keys)) in KNOBS.iter().enumerate() {
            let hint = match encoder {
                Some(_) => format!("{name} · drag or scroll to turn · keys {keys}"),
                None => format!("{name} volume · drag or scroll · keys {keys}"),
            };
            let response = c.knob(ui, *x, *y, name, self.knob_angle[index], &hint);
            // Circling the knob turns it by the pointer's angle around its
            // centre (clockwise +); scroll up or the right-hand key: clockwise.
            let mut amount = 0.;
            let centre = c.origin + vec2(*x, *y) * c.scale;
            self.knob_centre[index] = centre;
            if let Some(now) = response.interact_pointer_pos().filter(|_| response.dragged()) {
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
    fn knob(&self, ui: &mut egui::Ui, x: f32, y: f32, name: &str, angle: f32, hint: &str) -> egui::Response {
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
        let pointer = vec2(angle.sin(), -angle.cos()) * self.scale;
        self.painter
            .line_segment([p + pointer * 11., p + pointer * 18.], Stroke::new(3. * self.scale, INK));
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
                        if ui
                            .add_enabled(
                                self.cpu.is_some() && self.fault.is_none(),
                                egui::Button::new(if self.paused { "Resume" } else { "Pause" }),
                            )
                            .clicked()
                        {
                            self.paused = !self.paused;
                            let steps = self.cpu.as_ref().map_or(0, |cpu| cpu.steps);
                            self.speed_mark = (Instant::now(), steps);
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
                ui.label("Hold a key or button to press it · Arrow keys: octave · A W S E D R F G T H Y J K: notes · Knobs: drag, scroll, or 1–0 - = [ ] (MASTER: N / M)");
                let speed = self.speed.map_or(String::from("measuring…"), |s| format!("{:.0}% of real time", s * 100.));
                let audio = match (&self.audio, &self.audio_error) {
                    (Some(audio), _) => format!("audio {} Hz · {} dropouts", audio.device_rate, audio.underruns()),
                    (None, Some(error)) => format!("no audio: {error}"),
                    (None, None) => String::from("no audio"),
                };
                ui.weak(format!("Guest speed {speed} · {audio} · below 100% the sound breaks up"));
            }
        });
        // Use input from the previous rendered frame, then collect this frame's
        // input below. Losing focus immediately releases every matrix contact.
        if !ctx.input(|i| i.focused) {
            self.pressed.fill(false);
            self.pulse.fill(Instant::now());
            self.pulse_steps.fill(0);
        }
        self.run_slice(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(31, 35, 37))
                    .inner_margin(10.),
            )
            .show(ctx, |ui| self.panel(ui));
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
fn main() -> eframe::Result {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: emulator <application.elf|application.bin>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("expected one firmware path");
        std::process::exit(2);
    }
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
            let mut app = Emulator::new(PathBuf::from(path));
            match ui_audio::HostAudio::open() {
                Ok(audio) => app.audio = Some(audio),
                Err(error) => app.audio_error = Some(error),
            }
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
        app.run_slice(&ctx);
        let stop = Firmware::load(&app.path).unwrap().symbols["display_frame_done"];
        let cpu = app.cpu.as_mut().unwrap();
        cpu.run(Some(stop), cpu.steps + 200_000, None).unwrap();
        assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0xf7cb00);
        draw(&mut app, &ctx, vec![], false);
        assert!(app.pressed.iter().all(|&value| !value));
        app.run_slice(&ctx);
        let cpu = app.cpu.as_mut().unwrap();
        cpu.run(Some(stop), cpu.steps + 200_000, None).unwrap();
        assert_eq!(cpu.bus.lcd.pixels[202 * 240 + 24], 0x313031);
    }
    #[test]
    fn pause_and_restart_control_the_actual_cpu() {
        let mut app = demo();
        let ctx = egui::Context::default();
        app.run_slice(&ctx);
        let steps = app.cpu.as_ref().unwrap().steps;
        assert!(steps > 0);
        app.paused = true;
        app.run_slice(&ctx);
        assert_eq!(app.cpu.as_ref().unwrap().steps, steps);
        app.reset();
        assert_eq!(app.cpu.as_ref().unwrap().steps, 0);
        assert!(!app.paused);
        assert!(app.texture.is_none());
        assert!(app
            .cpu
            .as_ref()
            .unwrap()
            .bus
            .lcd
            .pixels
            .iter()
            .all(|&pixel| pixel == 0));
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
        app.cpu.as_mut().unwrap().steps = 2_400_000;
        draw(&mut app, &ctx, vec![], true);
        assert!(!app.pressed[0]);
    }
    #[test]
    fn unsupported_firmware_stops_without_fabricating_a_screen() {
        let mut app = demo();
        app.cpu = Some(Cpu::new(Bus::new(vec![0xff, 0x00]).unwrap(), 0x02000120));
        let ctx = egui::Context::default();
        app.run_slice(&ctx);
        assert!(app
            .fault
            .as_ref()
            .unwrap()
            .contains("unsupported instruction"));
        let cpu = app.cpu.as_ref().unwrap();
        let steps = cpu.steps;
        assert!(cpu.bus.lcd.pixels.iter().all(|&pixel| pixel == 0));
        app.run_slice(&ctx);
        assert_eq!(app.cpu.as_ref().unwrap().steps, steps);
    }
    #[test]
    fn circling_a_knob_clicks_its_encoder_and_master_stops_at_its_ends() {
        let mut app = demo();
        let ctx = egui::Context::default();
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
            for d in (step..=degrees).step_by(5).chain((degrees..=step).rev().step_by(5)) {
                draw(app, &ctx, vec![egui::Event::PointerMoved(at(d))], true);
            }
            draw(app, &ctx, vec![button(at(degrees), false)], true);
        };
        // A quarter turn clockwise around KNOB1 is six 15-degree clicks.
        drag(&mut app, 4, 90);
        assert!((app.knob_angle[4] - 6. * DETENT_ANGLE).abs() < 1e-4, "{}", app.knob_angle[4]);
        // Endless: two full turns back keep counting.
        drag(&mut app, 4, -720);
        assert!((app.knob_angle[4] + 42. * DETENT_ANGLE).abs() < 1e-4, "{}", app.knob_angle[4]);
        // MASTER is a pot: a big clockwise turn pins it at full scale.
        drag(&mut app, 0, 300);
        assert_eq!(app.master, 1023);
    }
}
