// SPDX-License-Identifier: GPL-3.0-only
// The decoded-instruction cache must never change what executes.
use fm1_emu::{
    bus::Bus,
    cpu::{Cpu, Fault},
    RAM, XIP,
};

fn cpu_with_ram_code(at: u32, words: &[u32]) -> Cpu {
    let mut cpu = Cpu::new(Bus::new(vec![0; 32]).unwrap(), at);
    for (i, &word) in words.iter().enumerate() {
        cpu.bus.write(at + i as u32 * 2, word, 2).unwrap();
    }
    cpu
}

#[test]
fn rewritten_ram_code_executes_its_new_instruction() {
    let at = RAM + 0x200;
    let mut cpu = cpu_with_ram_code(at, &[0x2140]); // r0 = 1
    cpu.step().unwrap();
    assert_eq!(cpu.r[0], 1);
    cpu.bus.write(at, 0x2240, 2).unwrap(); // r0 = 2
    cpu.pc = at;
    cpu.step().unwrap();
    assert_eq!(cpu.r[0], 2);
}

#[test]
fn bundle_primary_reads_operands_after_the_following_slot_stores() {
    // f13e 64fa: rD14 = r6 + (0x4fa | 0xfffff000), bundled with 509c, a
    // byte store of r4 to [r1 - 16] that rewrites the primary's extension
    // word to 0x64d4 before the primary slot reads it.
    let at = RAM + 0x100;
    let mut cpu = cpu_with_ram_code(at, &[0xf13e, 0x64fa, 0x509c]);
    cpu.r[1] = at + 0x12;
    cpu.r[4] = 0xd4;
    cpu.r[6] = 0x01c0_149c;
    cpu.step().unwrap();
    assert_eq!(cpu.r[14], 0x01c0_149c_u32.wrapping_add(0xffff_f4d4));
    assert_eq!(cpu.pc, at + 6);
}

#[test]
fn disabling_xip_faults_code_that_was_cached() {
    let mut cpu = Cpu::new(Bus::new(vec![0; 32]).unwrap(), XIP);
    cpu.step().unwrap(); // nop, now cached
    cpu.pc = XIP;
    cpu.bus.write(0x5101c, 0, 4).unwrap(); // Flash pins no longer routed.
    match cpu.step() {
        Err(Fault::Access { fault, .. }) => assert_eq!(fault.reason, "application XIP is disabled"),
        other => panic!("expected an XIP fault, got {other:?}"),
    }
}
