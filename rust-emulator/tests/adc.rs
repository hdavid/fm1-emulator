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
fn stock_pmu_calibration_samples_the_mux_and_restarts_with_completion_clear() {
    use fm1_emu::devices::{IRQ_CONFIG, IRQ_PENDING};
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    // Set P33 ANA_CON4 through the guest's serial bridge, then configure ADC
    // channel 15. Exercise repeated conversion acknowledgements and IRQ 24.
    bus.write(IRQ_CONFIG + 12, 1, 4).unwrap();
    let mut samples = Vec::new();
    for (source, millivolts) in [(5, 1050), (0, 800)] {
        bus.write(0x13e08, 0, 4).unwrap();
        bus.write(0x13e08, 1, 4).unwrap();
        for byte in [0, 4, (source << 1) | 1] {
            bus.write(0x13e0c, byte, 4).unwrap();
            bus.write(0x13e08, 0x11, 4).unwrap();
        }
        bus.write(0x13e08, 0, 4).unwrap();
        bus.write(CONTROL, 0, 4).unwrap();
        bus.write(CONTROL, 0xff7f, 4).unwrap();
        for _ in 0..20 {
            assert_eq!(bus.pending_irq(0x100), None);
            bus.devices.advance(31);
            assert_eq!(bus.read(CONTROL, 4).unwrap() & 128, 0);
            bus.devices.advance(1);
            assert_eq!(bus.pending_irq(0x100), Some(24));
            assert_eq!(bus.read(IRQ_PENDING, 4).unwrap(), 1 << 24);
            let measured_mv = bus.read(RESULT, 4).unwrap() * 3300 / 1023;
            assert!((millivolts - 3..=millivolts).contains(&measured_mv));
            samples.push(bus.read(RESULT, 4).unwrap());
            let control = bus.read(CONTROL, 4).unwrap();
            bus.write(CONTROL, control | 64, 4).unwrap();
        }
    }
    // Exercise the SDK's nominal calibration formula, not just ADC codes:
    // adc_get_voltage(VBAT) * 4 must report a full battery, not ~2.8 V.
    let battery_mv = samples[0] * 800 / samples[20] * 4;
    assert!((4180..=4220).contains(&battery_mv));
    assert_eq!(bus.devices.adc.conversions, 40);
    bus.write(CONTROL, 64, 4).unwrap();
    assert_eq!(bus.pending_irq(0x100), None);
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

#[test]
fn temperature_conversion_latches_the_pmu_mux_before_a_later_selection() {
    use fm1_emu::devices::IRQ_CONFIG;
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    bus.write(IRQ_CONFIG + 12, 1, 4).unwrap();
    let select = |bus: &mut Bus, source: u32| {
        bus.write(0x13e08, 1, 4).unwrap();
        for byte in [0, 4, (source << 1) | 1] {
            bus.write(0x13e0c, byte, 4).unwrap();
            bus.write(0x13e08, 0x11, 4).unwrap();
        }
        bus.write(0x13e08, 0, 4).unwrap();
    };
    select(&mut bus, 3);
    bus.write(CONTROL, 0xff7f, 4).unwrap();
    select(&mut bus, 0);
    assert_eq!(bus.pending_irq(0x100), None);
    bus.devices.advance(31);
    assert_eq!(bus.read(CONTROL, 4).unwrap() & 128, 0);
    bus.devices.advance(1);
    assert_eq!(bus.pending_irq(0x100), Some(24));
    assert!((349..=351).contains(&bus.read(RESULT, 4).unwrap()));
    // Acknowledge and restart: the next sample must use the new mux source.
    bus.write(CONTROL, 0xff7f, 4).unwrap();
    assert_eq!(bus.pending_irq(0x100), None);
    bus.devices.advance(32);
    assert_eq!(bus.read(RESULT, 4).unwrap(), 1023 * 800 / 3300);
    assert_eq!(bus.devices.adc.conversions, 2);
    select(&mut bus, 1);
    assert!(bus.write(CONTROL, 0xff7f, 4).is_err());
}
