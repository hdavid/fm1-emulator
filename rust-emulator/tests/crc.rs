// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::bus::Bus;
#[test]
fn crc_fifo_computes_xmodem_check_and_honors_seed() {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    for (seed, expected) in [(0, 0x31c3), (0xffff, 0x29b1)] {
        bus.write(0x13504, seed, 4).unwrap();
        for byte in b"123456789" {
            bus.write(0x13500, *byte as u32, 4).unwrap();
        }
        assert_eq!(bus.read(0x13504, 4).unwrap(), expected);
    }
    assert!(bus.read(0x13500, 4).is_err()); // FIFO is write-only.
}
