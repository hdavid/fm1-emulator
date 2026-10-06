// SPDX-License-Identifier: GPL-3.0-only
// Colours of the drawn FM-1.
use eframe::egui::Color32;

/// Every colour the panel is painted with.
pub(super) struct Theme {
    /// The case: drop shadow, outer rim, edge, face and the line inside it.
    pub shadow: Color32,
    pub rim: Color32,
    pub edge: Color32,
    pub body: Color32,
    pub body_line: Color32,
    /// Knob names printed on the case.
    pub label: Color32,
    /// Knobs: shadow, knurled rim, skirt, ticks, cap and pointer.
    pub knob_shadow: Color32,
    pub knob_rim: Color32,
    pub knob_skirt: Color32,
    pub knob_tick: Color32,
    pub knob_cap: Color32,
    pub knob_pointer: Color32,
    /// The LCD window: bezel and the glass around the guest pixels.
    pub bezel: Color32,
    pub glass: Color32,
    /// Recesses: around OCT-/OCT+, around the function buttons, and the key
    /// bed (its rim, then its floor).
    pub oct_recess: Color32,
    pub button_recess: Color32,
    pub keybed_rim: Color32,
    pub keybed: Color32,
    /// Keys and buttons: face at rest, hovered and pressed, then the shadow
    /// under them, their edge and the line inside it.
    pub key: Color32,
    pub key_hover: Color32,
    pub key_pressed: Color32,
    pub key_shadow: Color32,
    pub key_edge: Color32,
    pub key_inset: Color32,
    /// The slot line on each key, and the labels on keys and buttons.
    pub slot: Color32,
    pub key_label: Color32,
    /// Slot and label of a pressed key or button.
    pub pressed: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The emulator's original look (the default).
const CLASSIC: Theme = Theme {
    shadow: Color32::from_black_alpha(90),
    rim: Color32::from_gray(85),
    edge: Color32::from_gray(23),
    body: Color32::from_gray(34),
    body_line: Color32::from_gray(48),
    label: hex(0xbec2c1),
    knob_shadow: Color32::from_gray(12),
    knob_rim: Color32::from_gray(66),
    knob_skirt: Color32::from_gray(18),
    knob_tick: Color32::from_gray(135),
    knob_cap: Color32::from_gray(35),
    knob_pointer: hex(0xbec2c1),
    bezel: Color32::from_gray(8),
    glass: Color32::BLACK,
    oct_recess: Color32::from_gray(17),
    button_recess: Color32::from_gray(16),
    keybed_rim: Color32::from_gray(12),
    keybed: Color32::from_gray(40),
    key: Color32::from_gray(38),
    key_hover: Color32::from_gray(53),
    key_pressed: hex(0x4b412a),
    key_shadow: Color32::BLACK,
    key_edge: Color32::from_gray(86),
    key_inset: Color32::from_gray(24),
    slot: hex(0xbec2c1),
    key_label: hex(0xbec2c1),
    pressed: hex(0xe7c15b),
};

pub(super) const THEMES: [Theme; 1] = [CLASSIC];
