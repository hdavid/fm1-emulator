// SPDX-License-Identifier: GPL-3.0-only
// Peripheral behavior first reached by the stock FM-1_015 and Baud Girl
// FM-1_093 applications.
use fm1_emu::bus::Bus;

fn bus() -> Bus {
    Bus::new(vec![0; 32]).unwrap()
}

#[test]
fn lrc_trim_registers_retain_configuration_and_never_complete() {
    let mut b = bus();
    assert_eq!(b.read(0x13600, 4).unwrap(), 0);
    b.write(0x13600, 0x40, 4).unwrap(); // clear done
    b.write(0x13604, 0, 4).unwrap();
    b.write(0x13600, 0x07, 4).unwrap(); // window 3, enable
    assert_eq!(b.read(0x13600, 4).unwrap(), 0x07);
    for _ in 0..1000 {
        b.devices.advance(1000);
    }
    // Stub: the measurement never sets the done bit or a count.
    assert_eq!(b.read(0x13600, 4).unwrap() & 0x80, 0);
    assert_eq!(b.read(0x13604, 4).unwrap(), 0);
}

#[test]
fn hardware_resampler_accepts_its_disable_and_rejects_unemulated_conversions() {
    // Stock audio init at 0x0203abb2: JL_SRC->CON0 = 0 (WL82.h lsfr 0x4300).
    let mut b = bus();
    b.write(0x14300, 0, 4).unwrap();
    b.write(0x14310, 0x01c1_2340, 4).unwrap(); // IDAT_ADR
    assert_eq!(b.read(0x14310, 4).unwrap(), 0x01c1_2340);
    assert!(b.write(0x14300, 1, 4).is_err());
    assert!(b.read(0x14300, 1).is_err());
}

#[test]
fn random_number_generator_returns_fresh_words_and_is_read_only() {
    // Stock read at 0x02003472 of JL_RAND->R64L (WL82.h lsfr 0x3b00).
    let b = bus();
    let first = b.read(0x13b00, 4).unwrap();
    let second = b.read(0x13b00, 4).unwrap();
    assert_ne!(first, second);
    b.read(0x13b04, 4).unwrap();
    let mut b = bus();
    assert!(b.write(0x13b00, 0, 4).is_err());
    assert!(b.read(0x13b00, 2).is_err());
}
