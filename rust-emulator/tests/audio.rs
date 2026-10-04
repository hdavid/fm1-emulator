// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    audio::BASE,
    bus::Bus,
    devices::{IRQ_CONFIG, TIMER5},
    RAM,
};

fn playing() -> Bus {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    for (i, value) in [10, -20, 30, -40, 50, -60, 70, -80].iter().enumerate() {
        bus.write(RAM + i as u32 * 4, *value as u32, 4).unwrap();
    }
    bus.write(BASE + 0x1c, RAM, 4).unwrap();
    bus.write(BASE + 0x20, 4, 2).unwrap();
    bus.write(BASE + 4, 0x4000, 2).unwrap();
    bus.write(BASE, 0x800, 2).unwrap();
    bus
}

#[test]
fn dma_reads_stereo_frames_switches_halves_and_acknowledges_pending() {
    let mut bus = playing();
    bus.advance_audio(1089).unwrap(); // Exactly two 44.1 kHz frames.
    assert_eq!(bus.audio.frames, 2);
    assert_eq!(bus.audio.samples, [[10, -20], [30, -40]]);
    assert_eq!(bus.read(BASE, 2).unwrap() & 0x8000, 0x8000);
    assert_eq!(bus.read(BASE + 8, 1).unwrap(), 0x80);
    bus.write(BASE + 8, 8, 1).unwrap();
    assert_eq!(bus.read(BASE + 8, 1).unwrap(), 0);
    bus.advance_audio(1088).unwrap();
    assert_eq!(
        bus.audio.samples,
        [[10, -20], [30, -40], [50, -60], [70, -80]]
    );
    assert_eq!(bus.read(BASE, 2).unwrap() & 0x8000, 0);
    bus.write(BASE, 0, 2).unwrap();
    bus.advance_audio(24000).unwrap();
    assert_eq!(bus.audio.frames, 4);
}

#[test]
fn audio_pending_competes_with_timer_by_guest_priority_and_mask() {
    let mut bus = playing();
    bus.write(IRQ_CONFIG + 4, 7 << 12, 4).unwrap(); // ALNK11, priority3.
    bus.write(IRQ_CONFIG + 28, 3 << 28, 4).unwrap(); // TIMER63, priority1.
    bus.write(TIMER5 + 8, 1, 4).unwrap();
    bus.write(TIMER5, 0x4009, 4).unwrap();
    bus.devices.advance(1);
    bus.advance_audio(1089).unwrap();
    assert_eq!(bus.pending_irq(0x100), Some(11));
    assert_eq!(bus.read(0x01eef180, 4).unwrap(), 1 << 11);
    assert_eq!(bus.pending_irq(0), None);
    bus.write(BASE + 8, 8, 1).unwrap();
    assert_eq!(bus.pending_irq(0x100), Some(63));
}

#[test]
fn dma_rejects_a_buffer_outside_ram() {
    let mut bus = playing();
    bus.write(BASE + 0x1c, 0x02000120, 4).unwrap();
    assert!(bus.advance_audio(1).is_err());
}
