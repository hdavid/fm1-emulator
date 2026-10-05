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

#[test]
fn radio_stub_retains_registers_and_completes_rf_port_writes() {
    // STUB (radio.rs): JL_WL, JL_ANA WLA_CON1.. and the BT core window keep
    // written values; the RF port at 0x3101c drops its bit-17 busy flag.
    let mut b = bus();
    b.write(0x14040, 0x1234, 4).unwrap();
    assert_eq!(b.read(0x14040, 4).unwrap(), 0x1234);
    assert_eq!(b.read(0x11930, 4).unwrap(), 0);
    b.write(0x30f04, 7, 4).unwrap();
    assert_eq!(b.read(0x30f04, 4).unwrap(), 7);
    b.write(0x3101c, 0x000a_1600, 4).unwrap();
    assert_eq!(b.read(0x3101c, 4).unwrap(), 0x0008_1600);
    b.write(0x11978, 0, 4).unwrap(); // WLA_CON30: bit 5 reads as ready (inferred)
    assert_eq!(b.read(0x11978, 4).unwrap(), 0x20);
    assert!(b.radio.accesses.get() >= 6);
}

#[test]
fn spi2_stub_completes_after_the_shift_time_and_interrupts_when_enabled() {
    // STUB (spi2.rs): stock CON = 0x6020, BAUD 29, 2-byte DMA, then CON |= 1.
    // 16 bits * 30 lsb ticks at the assumed 4 lsb ticks per oscillator tick.
    let mut b = bus();
    b.write(0x11e00, 0x6020, 4).unwrap();
    b.write(0x11e04, 29, 4).unwrap();
    b.write(0x11e10, 2, 4).unwrap();
    b.write(0x11e00, 0x2021, 4).unwrap();
    for _ in 0..119 {
        b.spi2.advance(1);
    }
    assert!(!b.spi2.pending_irq());
    b.spi2.advance(1);
    assert!(b.spi2.pending_irq());
    assert_eq!(b.read(0x11e00, 4).unwrap() & 0x8000, 0x8000);
    b.write(0x11e00, 0x6021, 4).unwrap(); // clear pending
    assert!(!b.spi2.pending_irq());
}
