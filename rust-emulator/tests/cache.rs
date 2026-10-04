// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::bus::Bus;

#[test]
fn cache_configuration_preserves_idle_and_tag_ram_is_bounded() {
    let mut bus = Bus::new(vec![0, 0]).unwrap();
    bus.write(0x1eee008, 0x100, 4).unwrap();
    assert_eq!(bus.read(0x1eee008, 4).unwrap(), 0x4100);
    bus.write(0x1eee00c, 0x808080, 4).unwrap();
    assert_eq!(bus.read(0x1eee00c, 4).unwrap(), 0x808080);
    for address in [0x1f00000, 0x1f0bffc, 0x1f28000, 0x1f2fffc] {
        bus.write(address, 0x12345678, 4).unwrap();
        assert_eq!(bus.read(address, 4).unwrap(), 0x12345678);
        assert_eq!(bus.fetch(address).unwrap(), 0x5678);
    }
    assert!(bus.read(0x1f0c000, 4).is_err());
    assert!(bus.write(0x1f30000, 0, 4).is_err());
    for address in [0x40400, 0x40500] {
        assert_eq!(bus.read(address, 4).unwrap(), 0);
        assert!(bus.write(address, 1, 4).is_err());
    }
}
