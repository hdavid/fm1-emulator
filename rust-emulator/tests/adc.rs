// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{
    adc::{CONTROL, RESULT},
    bus::Bus,
};

#[test]
fn polled_conversion_samples_the_selected_input_and_completes_later() {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    bus.devices.adc.battery = 777;
    bus.devices.adc.master = 1023;
    for (channel, sample) in [(3, 777), (4, 1023)] {
        bus.write(CONTROL, 0, 4).unwrap();
        bus.write(CONTROL, 0xf04e | channel << 8, 4).unwrap();
        bus.write(CONTROL, 0xf05e | channel << 8, 4).unwrap();
        assert_eq!(bus.read(CONTROL, 4).unwrap() & 0x80, 0);
        bus.devices.adc.master = 100;
        bus.devices.advance(31);
        assert_eq!(bus.read(CONTROL, 4).unwrap() & 0x80, 0);
        bus.devices.advance(1);
        assert_eq!(bus.read(CONTROL, 4).unwrap() & 0x80, 0x80);
        assert_eq!(bus.read(RESULT, 4).unwrap(), sample);
        bus.write(CONTROL, 0x40, 4).unwrap();
        assert_eq!(bus.read(CONTROL, 4).unwrap() & 0x80, 0);
        bus.devices.adc.master = 1023;
    }
    assert_eq!(bus.devices.adc.conversions, 2);
}

#[test]
fn disable_cancels_conversion_and_unknown_channels_fault() {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    bus.write(CONTROL, 0xf45e, 4).unwrap();
    bus.write(CONTROL, 0, 4).unwrap();
    bus.devices.advance(100);
    assert_eq!(bus.read(CONTROL, 4).unwrap(), 0);
    assert_eq!(bus.devices.adc.conversions, 0);
    assert!(bus.write(CONTROL, 0xf15e, 4).is_err());
    assert!(bus.write(RESULT, 123, 4).is_err());
    assert!(bus.read(CONTROL, 2).is_err());
}
