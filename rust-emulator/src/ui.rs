// SPDX-License-Identifier: GPL-3.0-only
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
#[cfg(test)]
use fm1_emu::bus::Bus;
use fm1_emu::{cpu::Cpu, firmware::Firmware};
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
            if !self.paused && self.fault.is_none() {
                // Keep the UI responsive even if guest code spins forever.
                // This is an instruction budget, not a cycle-accuracy claim.
                let deadline = Instant::now() + Duration::from_millis(12);
                for i in 0..400_000 {
                    if let Err(error) = cpu.step() {
                        self.fault = Some(error.to_string());
                        break;
                    }
                    if i % 1024 == 0 && Instant::now() >= deadline {
                        break;
                    }
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
            self.pulse_steps[id] = self.cpu.as_ref().map_or(0, |cpu| cpu.steps + 72_000_000);
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
            self.pulse_steps[id] = self.cpu.as_ref().map_or(0, |cpu| cpu.steps + 72_000_000);
        }
        if !focused {
            self.pulse[id] = Instant::now();
            self.pulse_steps[id] = 0;
        }
        // A slow host gives the guest at least 100 ms at up to 360 MHz,
        // including two issued core instructions per shared clock step, to scan
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
                ui.label("Hold a key or button to press it · Arrow keys: octave · A W S E D R F G T H Y J K: notes");
                ui.weak("The LCD follows the loaded firmware. Audio playback and rotary input are not implemented.");
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
            Ok(Box::new(Emulator::new(PathBuf::from(path))))
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
        app.cpu.as_mut().unwrap().steps = 72_000_000;
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
}
