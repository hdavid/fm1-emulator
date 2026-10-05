// SPDX-License-Identifier: GPL-3.0-only
// USB0 endpoint 4. The controller has five device endpoints; EP4's count and
// DMA addresses have their own registers after EP3's (AC79 SDK WL82.h
// JL_USB: EP4_CNT 0x11834, EP4_TADR 0x11838, EP4_RADR 0x1183c; Felucca 1.0
// fm1_usb.h uses them for its isochronous UAC IN stream and selects INDEX 4
// at boot).
use fm1_emu::{usb::Usb, RAM, RAM_SIZE};

const EP0_TADR: u32 = 0x11818;
const EP4_CNT: u32 = 0x11834;
const EP4_TADR: u32 = 0x11838;
const EP4_RADR: u32 = 0x1183c;

struct Controller {
    usb: Usb,
    ram: Vec<u8>,
}

impl Controller {
    fn new() -> Self {
        let mut c = Self {
            usb: Usb::default(),
            ram: vec![0; RAM_SIZE],
        };
        c.reg(0x51000, 0x40);
        c.reg(EP0_TADR, RAM + 0x100);
        c.reg(0x11800, 4);
        c
    }
    fn try_reg(&mut self, a: u32, v: u32) -> Result<(), &'static str> {
        self.usb.write(a, v, &mut self.ram).unwrap()
    }
    fn reg(&mut self, a: u32, v: u32) {
        self.try_reg(a, v).unwrap();
    }
    fn wr(&mut self, r: u32, v: u32) -> Result<(), &'static str> {
        self.try_reg(0x11804, (r << 8) | v)
    }
    fn rd(&mut self, r: u32) -> u32 {
        self.reg(0x11804, (r << 8) | 0x4000);
        self.usb.read(0x11804).unwrap() & 0xff
    }
}

#[test]
fn endpoint_4_registers_hold_their_values() {
    let mut c = Controller::new();
    for (a, v) in [
        (EP4_CNT, 0x60),
        (EP4_TADR, RAM + 0x800),
        (EP4_RADR, RAM + 0x900),
    ] {
        c.reg(a, v);
        assert_eq!(c.usb.read(a), Some(v));
    }
}

#[test]
fn index_4_selects_endpoint_4_and_its_own_csr() {
    let mut c = Controller::new();
    c.wr(14, 4).unwrap();
    assert_eq!(c.rd(14), 4);
    c.wr(18, 0x40).unwrap(); // TXCSR2: isochronous
    c.wr(14, 1).unwrap();
    assert_eq!(c.rd(18), 0, "EP1 keeps its own CSR");
    c.wr(14, 4).unwrap();
    assert_eq!(c.rd(18), 0x40);
}

#[test]
fn endpoint_4_sends_from_its_own_dma_address_and_count() {
    // Felucca's fm1_usb_ep4_send: TADR, then CNT, then TxPktRdy. With EP1's
    // registers zero, reading them for EP4 would DMA from address 0.
    let mut c = Controller::new();
    c.reg(EP4_TADR, RAM + 0x800);
    c.reg(EP4_CNT, 64);
    c.wr(14, 4).unwrap();
    c.wr(17, 1).unwrap();
    assert_eq!(c.rd(2) & 0x10, 0x10, "EP4's TX interrupt flag");
    // A count above the full-speed bulk limit is still refused.
    c.reg(EP4_CNT, 65);
    assert!(c.wr(17, 1).is_err());
}

#[test]
fn indexes_above_4_are_not_modeled() {
    let mut c = Controller::new();
    assert_eq!(
        c.wr(14, 5),
        Err("USB endpoint index exceeds modeled controller")
    );
}
