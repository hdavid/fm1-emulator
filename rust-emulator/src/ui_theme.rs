// SPDX-License-Identifier: GPL-3.0-only
// Colours of the drawn FM-1: the emulator's original look and the device's
// colour editions, sampled from product photos and kept flat like the panel.
use eframe::egui::Color32;

/// Every colour the panel is painted with.
pub(super) struct Theme {
    pub name: &'static str,
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
    name: "Classic",
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

pub(super) const THEMES: [Theme; 7] = [
    CLASSIC,
    // Grey case, black keys and buttons, white slots and labels.
    Theme {
        name: "Black",
        shadow: Color32::from_black_alpha(90),
        rim: hex(0x7a7a7a),
        edge: hex(0x3a3a3a),
        body: hex(0x5d5d5d),
        body_line: hex(0x686868),
        label: hex(0xeaeaea),
        knob_shadow: hex(0x262626),
        knob_rim: hex(0x303030),
        knob_skirt: hex(0x141414),
        knob_tick: hex(0x444444),
        knob_cap: hex(0x1c1c1c),
        knob_pointer: hex(0xf2f2f2),
        bezel: hex(0x0a0a0a),
        glass: Color32::BLACK,
        oct_recess: hex(0x232323),
        button_recess: hex(0x232323),
        keybed_rim: hex(0x2a2a2a),
        keybed: hex(0x383838),
        key: hex(0x2a2a2a),
        key_hover: hex(0x363636),
        key_pressed: hex(0x4b412a),
        key_shadow: hex(0x101010),
        key_edge: hex(0x484848),
        key_inset: hex(0x202020),
        slot: hex(0xf2f2f2),
        key_label: hex(0xdcdcdc),
        pressed: hex(0xe7c15b),
    },
    // Lilac case, purple keys and buttons; dark slots and labels.
    Theme {
        name: "Lilac",
        shadow: Color32::from_black_alpha(90),
        rim: hex(0xd8b8e6),
        edge: hex(0xa27cb6),
        body: hex(0xc298d4),
        body_line: hex(0xcda7de),
        label: hex(0x3a2c4c),
        knob_shadow: hex(0x8e6ca2),
        knob_rim: hex(0x2c2a30),
        knob_skirt: hex(0x141416),
        knob_tick: hex(0x3e3c44),
        knob_cap: hex(0x1c1c1f),
        knob_pointer: hex(0xf4f4f4),
        bezel: hex(0x0a0a0b),
        glass: Color32::BLACK,
        oct_recess: hex(0x6a5ea2),
        button_recess: hex(0x6a5ea6),
        keybed_rim: hex(0x5c5094),
        keybed: hex(0x7a6fb5),
        key: hex(0x6e62a8),
        key_hover: hex(0x7d72b8),
        key_pressed: hex(0x52478a),
        key_shadow: hex(0x40366c),
        key_edge: hex(0x8c82c4),
        key_inset: hex(0x62569c),
        slot: hex(0x2d2a46),
        key_label: hex(0x1a1628),
        pressed: hex(0xf6d77a),
    },
    // Terracotta all over, black knobs; light labels.
    Theme {
        name: "Orange",
        shadow: Color32::from_black_alpha(90),
        rim: hex(0xd2845f),
        edge: hex(0x9a5136),
        body: hex(0xbe6d49),
        body_line: hex(0xc87a56),
        label: hex(0xffffff),
        knob_shadow: hex(0x86462e),
        knob_rim: hex(0x2c2624),
        knob_skirt: hex(0x161212),
        knob_tick: hex(0x40383a),
        knob_cap: hex(0x1c1717),
        knob_pointer: hex(0xf4f4f4),
        bezel: hex(0x0a0a0a),
        glass: Color32::BLACK,
        oct_recess: hex(0x7c4838),
        button_recess: hex(0x854e42),
        keybed_rim: hex(0x6c3c32),
        keybed: hex(0x8e5448),
        key: hex(0x844d43),
        key_hover: hex(0x935a4f),
        key_pressed: hex(0x63362c),
        key_shadow: hex(0x4c281f),
        key_edge: hex(0xa06456),
        key_inset: hex(0x76423a),
        slot: hex(0x33292a),
        key_label: hex(0xf6e4dc),
        pressed: hex(0xffd27a),
    },
    // Dark grey case, mint keys and buttons; dark slots and labels.
    Theme {
        name: "Mint",
        shadow: Color32::from_black_alpha(90),
        rim: hex(0x666668),
        edge: hex(0x2e2e30),
        body: hex(0x4b4b4d),
        body_line: hex(0x575759),
        label: hex(0xe6e6e8),
        knob_shadow: hex(0x1e1e1f),
        knob_rim: hex(0x303032),
        knob_skirt: hex(0x141415),
        knob_tick: hex(0x444447),
        knob_cap: hex(0x1d1d1f),
        knob_pointer: hex(0xf2f2f2),
        bezel: hex(0x0a0a0a),
        glass: Color32::BLACK,
        oct_recess: hex(0x62b7aa),
        button_recess: hex(0x68c0b3),
        keybed_rim: hex(0x4f9e92),
        keybed: hex(0x79d1c6),
        key: hex(0x6fcabd),
        key_hover: hex(0x85d8cd),
        key_pressed: hex(0x4b9f93),
        key_shadow: hex(0x3a8278),
        key_edge: hex(0x96e2d8),
        key_inset: hex(0x60b9ac),
        slot: hex(0x2e3d44),
        key_label: hex(0x1d3633),
        pressed: hex(0xffffff),
    },
    // Cream case, dark grey keys and buttons; dark labels on the case.
    Theme {
        name: "Cream",
        shadow: Color32::from_black_alpha(70),
        rim: hex(0xf0e8df),
        edge: hex(0xc2b7ab),
        body: hex(0xe1d7cc),
        body_line: hex(0xd7ccc1),
        label: hex(0x544e4c),
        knob_shadow: hex(0xb4a99f),
        knob_rim: hex(0x2c2c2c),
        knob_skirt: hex(0x141414),
        knob_tick: hex(0x424242),
        knob_cap: hex(0x1c1c1c),
        knob_pointer: hex(0xf2f2f2),
        bezel: hex(0x0a0a0d),
        glass: Color32::BLACK,
        oct_recess: hex(0x3a3d40),
        button_recess: hex(0x3d4044),
        keybed_rim: hex(0x313538),
        keybed: hex(0x40464a),
        key: hex(0x36393b),
        key_hover: hex(0x44484b),
        key_pressed: hex(0x4b412a),
        key_shadow: hex(0x202224),
        key_edge: hex(0x52575b),
        key_inset: hex(0x2c2f31),
        slot: hex(0x1c1f22),
        key_label: hex(0xa4aaae),
        pressed: hex(0xe7c15b),
    },
    // White case, blue keys and buttons; blue-grey labels on the case.
    Theme {
        name: "Blue",
        shadow: Color32::from_black_alpha(70),
        rim: hex(0xfbf9f5),
        edge: hex(0xd2ccc3),
        body: hex(0xf0ece6),
        body_line: hex(0xe5e0d8),
        label: hex(0x5f7290),
        knob_shadow: hex(0xc8c1b6),
        knob_rim: hex(0x2c2c2c),
        knob_skirt: hex(0x141414),
        knob_tick: hex(0x424242),
        knob_cap: hex(0x1c1c1c),
        knob_pointer: hex(0xf2f2f2),
        bezel: hex(0x0a0a0b),
        glass: Color32::BLACK,
        oct_recess: hex(0x4e6884),
        button_recess: hex(0x557290),
        keybed_rim: hex(0x3e5874),
        keybed: hex(0x557392),
        key: hex(0x4a6888),
        key_hover: hex(0x58789a),
        key_pressed: hex(0x39516c),
        key_shadow: hex(0x2c4056),
        key_edge: hex(0x6a88a8),
        key_inset: hex(0x415e7c),
        slot: hex(0x232e3c),
        key_label: hex(0x0e1520),
        pressed: hex(0xffe08a),
    },
];

/// The theme called `name` (any case).
pub(super) fn find(name: &str) -> Option<usize> {
    THEMES
        .iter()
        .position(|theme| theme.name.eq_ignore_ascii_case(name.trim()))
}

/// `"Classic, Black, ..."` for messages.
pub(super) fn names() -> String {
    THEMES.map(|theme| theme.name).join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio between two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let channel = |v: u8| {
                let v = v as f32 / 255.;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn themes_are_found_by_name_and_classic_comes_first() {
        assert_eq!(THEMES[0].name, "Classic");
        for (index, theme) in THEMES.iter().enumerate() {
            assert_eq!(find(theme.name), Some(index));
            assert_eq!(find(&theme.name.to_lowercase()), Some(index));
        }
        assert_eq!(find("Purple"), None);
        assert!(names().starts_with("Classic, Black"));
    }

    #[test]
    fn every_theme_keeps_its_labels_readable() {
        for theme in &THEMES {
            let name = theme.name;
            // Knob names: 3:1 (white on the terracotta case, as printed, is 3.8:1).
            assert!(
                contrast(theme.label, theme.body) >= 3.,
                "{name} case labels"
            );
            for face in [theme.key, theme.key_hover] {
                assert!(contrast(theme.key_label, face) >= 3., "{name} key labels");
            }
            // Slots only have to show (the cream edition's barely do).
            assert!(contrast(theme.slot, theme.key) >= 1.4, "{name} key slots");
            assert!(
                contrast(theme.pressed, theme.key_pressed) >= 2.,
                "{name} pressed"
            );
            // The guest's pixels keep a dark surround, as on every edition.
            for surround in [theme.bezel, theme.glass] {
                assert!(
                    contrast(surround, Color32::WHITE) >= 15.,
                    "{name} LCD bezel"
                );
            }
        }
    }
}
