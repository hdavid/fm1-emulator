// SPDX-License-Identifier: GPL-3.0-only
pub mod adc;
pub mod audio;
pub mod bus;
mod cache;
mod clock;
mod code_cache;
pub mod cpu;
mod crc;
mod decode;
pub mod devices;
pub mod firmware;
pub mod gpio;
pub mod husb;
pub mod lcd;
mod package;
pub mod player;
pub mod png;
pub mod radio;
mod resample;
mod rng;
pub mod spi2;
pub mod uart1;
pub mod ui_knobs;

pub const XIP: u32 = 0x0200_0120;
pub const XIP_END: u32 = 0x0210_0000;
pub const RAM: u32 = 0x01c0_0000;
pub const RAM_SIZE: usize = 512 * 1024;
pub const RESULT: u32 = 0x01c0_8000;
pub const USER_STACK: u32 = 0x01c7_a000;
pub const SYSTEM_STACK: u32 = 0x01c7_c000;
pub const PROBE_RETURN: u32 = 0xffff_fffe;
pub const NAMES: [&str; 12] = [
    "constant",
    "add_wrap",
    "subtract",
    "xor",
    "and",
    "or",
    "not",
    "shift_left",
    "shift_right",
    "load_add",
    "loop_sum",
    "stack",
];

mod extended;

pub mod system;

mod guards;

mod nor;

pub mod usb;
pub mod usb_audio;
pub mod usb_audio_host;
pub mod usb_midi;
pub mod web;
