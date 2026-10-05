// SPDX-License-Identifier: GPL-3.0-only
use fm1_emu::{bus::Bus, cpu::Cpu, RAM, XIP};

fn cpu(words: &[u16]) -> Cpu {
    Cpu::new(
        Bus::new(words.iter().flat_map(|w| w.to_le_bytes()).collect()).unwrap(),
        XIP,
    )
}
#[test]
fn scheduler_restores_the_task_frame_and_stack_pointer_banks() {
    let mut c = cpu(&[0x04e8, 0x04a8, 0x1442, 0x1443, 0x1440, 0x1441]);
    c.sr[14] = RAM + 64;
    c.sr[5] = 0x12345678;
    c.sr[3] = XIP + 20;
    c.sr[0] = XIP + 24;
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 56);
    assert_eq!(c.bus.read(RAM + 56, 4).unwrap(), XIP + 20);
    assert_eq!(c.bus.read(RAM + 60, 4).unwrap(), 0x12345678);
    c.sr[3] = 0;
    c.sr[5] = 0;
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 64);
    assert_eq!(c.sr[3], XIP + 20);
    assert_eq!(c.sr[5], 0x12345678);
    assert_eq!(c.sr[0], XIP + 24);
    c.step().unwrap();
    c.sr[14] = RAM + 128;
    c.step().unwrap();
    c.sr[14] = 0;
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 64);
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 128);
}

#[test]
fn leading_zero_count_selects_the_highest_ready_task_priority() {
    for (value, expected) in [
        (0, 32),
        (1, 31),
        (0x2000c001, 2),
        (0x80000000, 0),
        (u32::MAX, 0),
    ] {
        let mut c = cpu(&[0xe180, 0x4200]); // r4 = clz(r2)
        c.r[2] = value;
        c.r[0] = 0x55555555;
        c.step().unwrap();
        assert_eq!(c.r[4], expected);
        assert_eq!(c.r[2], value);
        assert_eq!(c.r[0], 0x55555555);
    }
}

#[test]
fn stock_can_disable_the_unused_high_speed_usb_controller() {
    let mut c = cpu(&[0]);
    c.bus.write(0x16800, 0, 4).unwrap();
    assert_eq!(c.bus.read(0x16800, 4).unwrap(), 0);
    // STUB (husb.rs): enabling (bits 0-1) reports ready in bit 4, as the
    // stock usb id 1 init at 0x02006f64 polls for.
    c.bus.write(0x16800, 3, 4).unwrap();
    assert_eq!(c.bus.read(0x16800, 4).unwrap(), 0x13);
}

#[test]
fn register_list_stores_linked_list_fields_without_advancing_the_base() {
    let mut c = cpu(&[0xeb20, 6, 0xeb21, 0x101]);
    c.r[0] = RAM;
    c.r[1] = RAM + 64;
    c.r[2] = RAM + 128;
    c.r[8] = 0x12345678;
    // Lowest register at the lowest address (see stock_isa.rs register_list_*).
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM, 4).unwrap(), RAM + 64);
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), RAM + 128);
    assert_eq!(c.r[0], RAM);
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 64, 4).unwrap(), RAM);
    assert_eq!(c.bus.read(RAM + 68, 4).unwrap(), 0x12345678);
    assert_eq!(c.r[1], RAM + 64);
}

#[test]
fn flash_identification_shifts_the_jedec_word_in_memory() {
    let mut c = cpu(&[0xe86c, 0x581e]); // [r5+28] >>= 8
    c.r[5] = RAM;
    c.bus.write(RAM + 28, 0x85601400, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 28, 4).unwrap(), 0x00856014);
    assert_eq!(c.r[5], RAM);
}

#[test]
fn packed_unsigned_less_equal_checks_the_stock_ram_code_size() {
    for (value, expected) in [(0, 0), (0xd80, 0), (4096, 0), (4097, 1), (u32::MAX, 1)] {
        let mut c = cpu(&[0xeca1, 0x0d80, 0x2040, 0]);
        c.r[1] = value;
        c.r[0] = 1;
        while c.pc < XIP + 6 {
            c.step().unwrap();
        }
        assert_eq!(c.r[0], expected);
    }
}

#[test]
fn interrupt_enable_instructions_expose_the_global_icfg_bit() {
    let mut c = cpu(&[0x0061, 0x0060, 0x0061, 0xe064, 0x0b00, 0xe064, 0x0b80]);
    c.sr[11] = 0x100;
    c.step().unwrap();
    assert_eq!(c.sr[11], 0x300);
    c.step().unwrap();
    assert!(!c.interrupts_enabled);
    assert_eq!(c.sr[11], 0x100);
    c.step().unwrap();
    c.step().unwrap();
    assert_eq!(c.r[0], 0x300);
    c.r[0] = 0x100;
    c.step().unwrap();
    assert!(!c.interrupts_enabled);
}

#[test]
fn stock_startup_long_calls_return_after_six_bytes() {
    // Vendor disassembly: call 176, and nested call -10 to an rts.
    let mut c = cpu(&[0xff80, 0x00b0, 0x0000]);
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 6 + 176);
    assert_eq!(c.sr[3], XIP + 6);
    let mut c = cpu(&[0x0080, 0xff80, 0xfff8, 0xffff]);
    c.pc = XIP + 2;
    c.step().unwrap();
    assert_eq!(c.pc, XIP);
    assert_eq!(c.sr[3], XIP + 8);
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 8);
}

#[test]
fn core_tick_timer_wraps_acknowledges_and_obeys_irq_priority() {
    use fm1_emu::devices::{IRQ_CONFIG, IRQ_PENDING, TICK_IRQ, TICK_TIMER};
    let mut c = cpu(&[0; 16]);
    c.bus.write(TICK_TIMER + 8, 29, 4).unwrap();
    c.bus.write(TICK_TIMER, 1, 1).unwrap();
    c.bus.devices.advance(1);
    assert_eq!(c.bus.read(TICK_TIMER + 4, 4).unwrap(), 15);
    assert_eq!(c.bus.pending_irq(0x100), None);
    c.bus.devices.advance(1);
    assert_eq!(c.bus.read(TICK_TIMER + 4, 4).unwrap(), 0);
    assert_eq!(c.bus.read(TICK_TIMER, 1).unwrap(), 129);
    assert_eq!(c.bus.read(IRQ_PENDING, 4).unwrap(), 8);
    c.bus.write(IRQ_CONFIG, 3 << 12, 4).unwrap();
    assert_eq!(c.bus.pending_irq(0x100), Some(TICK_IRQ));
    c.bus.write(TICK_TIMER, 65, 1).unwrap();
    assert_eq!(c.bus.pending_irq(0x100), None);
    assert_eq!(c.bus.read(TICK_TIMER, 1).unwrap(), 1);
    c.bus.write(TICK_TIMER, 0, 1).unwrap();
    c.bus.devices.advance(100);
    assert_eq!(c.bus.read(TICK_TIMER + 4, 4).unwrap(), 0);
}

#[test]
fn startup_timer_banks_count_and_signal_their_sdk_interrupts() {
    use fm1_emu::devices::{IRQ_CONFIG, IRQ_PENDING};
    for index in 0..4 {
        let mut c = cpu(&[0]);
        let base = 0x10400 + index * 256;
        c.bus.write(base, 0x4000, 4).unwrap();
        c.bus.write(base + 8, 3, 4).unwrap();
        c.bus.write(base, 9, 4).unwrap();
        c.bus.write(IRQ_CONFIG, 3 << ((4 + index) * 4), 4).unwrap();
        c.bus.devices.advance(3);
        assert_eq!(c.bus.pending_irq(0x100), Some((4 + index) as usize));
        assert_eq!(c.bus.read(IRQ_PENDING, 4).unwrap(), 1 << (4 + index));
        c.bus.write(base, 0x4009, 4).unwrap();
        assert_eq!(c.bus.pending_irq(0x100), None);
    }
}

#[test]
fn startup_repeat_clears_exactly_the_requested_words() {
    // Stock startup: rep 2 r2 { [r3++=4] = r1 }; if (r2 != 0) goto rep.
    for count in [0, 1, 3] {
        let mut c = cpu(&[0x0302, 0x05b1, 0x5df2, 0x0000]);
        c.r[1] = 0x11223344;
        c.r[2] = count;
        c.r[3] = RAM;
        while c.pc != XIP + 6 {
            c.step().unwrap();
            assert!(c.steps <= 10);
        }
        assert_eq!(c.r[2], 0);
        assert_eq!(c.r[3], RAM + count * 4);
        for i in 0..count {
            assert_eq!(c.bus.read(RAM + i * 4, 4).unwrap(), 0x11223344);
        }
        assert_eq!(c.bus.read(RAM + count * 4, 4).unwrap(), 0);
    }
}

#[test]
fn packed_memory_and_preserves_the_high_cache_way_bits() {
    // Stock cache setup: [r0+4] &= 0xff000000.
    let mut c = cpu(&[0xef81, 0x047f]);
    c.r[0] = RAM;
    c.bus.write(RAM + 4, 0x87654321, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), 0x87000000);
    assert_eq!(c.r[0], RAM);
}

#[test]
fn word_postincrement_store_uses_the_old_base_and_preserves_the_gap() {
    let mut c = cpu(&[0xecd8, 0x1009]); // [r0++=8] = r1
    c.r[0] = RAM;
    c.r[1] = u32::MAX;
    c.bus.write(RAM + 4, 0x12345678, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM, 4).unwrap(), u32::MAX);
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), 0x12345678);
    assert_eq!(c.r[0], RAM + 8);
}

#[test]
fn packed_inequality_selects_the_stock_oscillator_frequency() {
    // Vendor stock disassembly: if (r7 != 0x08000000) { 24 MHz } else { 40 MHz }.
    for (value, expected) in [(0x04000000, 24_000_000), (0x08000000, 40_000_000)] {
        let mut c = cpu(&[
            0xe8a7, 0x1600, 0xffc7, 0x3600, 0x016e, 0xffc7, 0x5a00, 0x0262, 0,
        ]);
        c.r[7] = value;
        while c.pc < XIP + 16 {
            c.step().unwrap();
        }
        assert_eq!(c.r[7], expected);
    }
}

#[test]
fn immediate_repeat_clears_twenty_words_and_copies_multiword_blocks() {
    let mut c = cpu(&[0x9300, 0x0592, 0x0000]);
    c.r[1] = RAM;
    c.r[2] = 0x12345678;
    while c.pc != XIP + 4 {
        c.step().unwrap();
        assert!(c.steps <= 21);
    }
    assert_eq!(c.r[1], RAM + 80);
    for i in 0..20 {
        assert_eq!(c.bus.read(RAM + i * 4, 4).unwrap(), 0x12345678);
    }
    assert_eq!(c.bus.read(RAM + 80, 4).unwrap(), 0);

    let mut c = cpu(&[0x8210, 0x0513, 0x05c3, 0x0000]);
    c.r[1] = RAM;
    c.r[4] = RAM + 32;
    for i in 0..3 {
        c.bus.write(RAM + i * 4, 0x12340000 + i, 4).unwrap();
    }
    while c.pc != XIP + 6 {
        c.step().unwrap();
        assert!(c.steps <= 7);
    }
    for i in 0..3 {
        assert_eq!(c.bus.read(RAM + 32 + i * 4, 4).unwrap(), 0x12340000 + i);
    }
}

#[test]
fn an_interrupt_waits_for_the_unfinished_repeat() {
    // The stock RTOS context switch saves only r0-r15 and {psr, rets, reti},
    // so a pending interrupt is taken once the repeat block has finished.
    use fm1_emu::devices::{IRQ_CONFIG, TIMER5};
    let mut c = cpu(&[0x8200, 0x0592, 0x0000, 0x0081]);
    c.r[1] = RAM;
    c.r[2] = 42;
    c.sr[14] = RAM + 256;
    c.sr[13] = RAM + 512;
    c.sr[11] = 0x100;
    c.bus.write(0x01c7fe00 + 63 * 4, XIP + 6, 4).unwrap();
    c.bus.write(IRQ_CONFIG + 7 * 4, 1 << 28, 4).unwrap();
    c.bus.write(TIMER5 + 8, 1, 4).unwrap();
    c.bus.write(TIMER5, 9, 4).unwrap();
    c.interrupts_enabled = true;
    c.step().unwrap(); // rep 3
    for _ in 0..3 {
        assert_ne!(c.pc, XIP + 6);
        c.step().unwrap();
    }
    assert_eq!(c.r[1], RAM + 12);
    assert_eq!(c.bus.read(RAM + 8, 4).unwrap(), 42);
    c.step().unwrap(); // the interrupt is delivered after the block
    assert_eq!(c.sr[0], XIP + 4);
}

#[test]
fn atomic_lock_flags_match_twelve_physical_fm1_measurements() {
    for (byte, flags, taken) in [
        (0, 0, false),
        (1, 1, false),
        (0x80, 0, false),
        (0xff, 15, true),
        (2, 2, false),
        (4, 4, true),
        (8, 8, false),
        (15, 15, true),
        (0x7f, 15, true),
        (0xfe, 14, true),
        (0xf0, 0, false),
        (0x10, 0, false),
    ] {
        let mut c = cpu(&[0x00b1, 0xe840, 0x0002, 0x0000, 0x0000, 0x0000]);
        c.r[1] = RAM + 1;
        c.bus.write(RAM, 0x44332211, 4).unwrap();
        c.bus.write(RAM + 1, byte, 1).unwrap();
        c.step().unwrap();
        assert_eq!(c.sr[5], flags);
        assert_eq!(c.bus.read(RAM, 4).unwrap(), 0x4433ff11);
        assert_eq!(c.r[1], RAM + 1);
        c.step().unwrap();
        assert_eq!(c.pc, if taken { XIP + 10 } else { XIP + 6 });
    }
}

#[test]
fn conditional_skip_counts_the_whole_long_call() {
    // if (r5 < 5) { call ... } else { r0=22 }.
    let mut c = cpu(&[0xe9b5, 0x1005, 0xff80, 0x00b0, 0x0000, 0x3640]);
    c.r[5] = 5;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 10);
    c.step().unwrap();
    assert_eq!(c.r[0], 22);
    assert_eq!(c.sr[3], 0);
}
#[test]
fn compiler_parallel_store_uses_the_previous_register_value() {
    // Vendor compiler startup: r0 = 0x4009 # [r1+4] = r0.
    let mut c = cpu(&[0xf040, 0x4009, 0x6190]);
    c.r[0] = 17;
    c.r[1] = RAM;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), 17);
    assert_eq!(c.r[0], 0x4009);
    assert_eq!(c.pc, XIP + 6);
}

#[test]
fn parallel_add_reads_the_old_value_of_the_register_loaded_by_the_other_slot() {
    // cv_text: r1 += r0 # r0 = [sp].
    let mut c = cpu(&[0xd801, 0x2000]);
    c.r[0] = 0x3900;
    c.r[1] = 0x0205c9f0;
    c.sr[14] = RAM;
    c.bus.write(RAM, 0x0205c930, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], 0x020602f0);
    assert_eq!(c.r[0], 0x0205c930);
    assert_eq!(c.pc, XIP + 4);
}
#[test]
fn compiler_signed_immediates_terminate_decimal_printing() {
    let mut c = cpu(&[0xe040, 0xa240, 0xf8f5, 0xfffe]);
    c.step().unwrap();
    assert_eq!(c.r[0], (-24000i32) as u32);
    c.r[5] = u32::MAX;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 8);
}
#[test]
fn conditional_block_executes_only_the_selected_arm() {
    // if ((r3 & 0x8000)==0) { r0=1 } else { r0=2 }
    for (condition, expected) in [(0, 1), (0x8000, 2)] {
        let mut c = cpu(&[0xea23, 0x1c00, 0x2140, 0x2240, 0x0000]);
        c.r[3] = condition;
        c.step().unwrap();
        c.step().unwrap();
        c.step().unwrap();
        assert_eq!(c.r[0], expected);
        assert_eq!(c.pc, XIP + 10);
    }
}

#[test]
fn conditional_memset_alignment_counts_a_parallel_pair_once() {
    for offset in [0, 1, 2, 3] {
        // Stock memset alignment loop; subtract and store form one bundle.
        let mut c = cpu(&[
            0x5202, 0xea33, 0x4003, 0xf0f2, 0x2001, 0x07b1, 0x99f7, 0x2144,
        ]);
        c.r[2] = 16;
        c.r[3] = RAM + offset;
        c.r[1] = 0xa5;
        while c.pc < XIP + 14 {
            c.step().unwrap();
            assert!(c.steps < 20);
        }
        let filled = (4 - offset) % 4;
        assert_eq!(c.r[2], 16 - filled);
        assert_eq!(c.r[3], RAM + offset + filled);
        for i in 0..filled {
            assert_eq!(c.bus.read(RAM + offset + i, 1).unwrap(), 0xa5);
        }
        assert_eq!(c.bus.read(RAM + offset + filled, 1).unwrap(), 0);
    }
}

#[test]
fn unsigned_immediate_conditional_selects_storage_header_region() {
    // Vendor form from st_head: if (r5 < 5) { r0=11 } else { r0=22 }.
    for (value, expected) in [(0, 11), (4, 11), (5, 22), (u32::MAX, 22)] {
        let mut c = cpu(&[0xe9b5, 0x1005, 0x2b40, 0x3640, 0x0000]);
        c.r[5] = value;
        for _ in 0..3 {
            c.step().unwrap();
        }
        assert_eq!(c.r[0], expected);
        assert_eq!(c.pc, XIP + 10);
    }
}

#[test]
fn register_list_loads_ascend_without_changing_the_base() {
    // Vendor form: 04 eb 04 01 => {r8, r2} = [r4+]; r2 comes from [r4]
    // (Felucca fm1_fault_c reads fm1_crash.magic into r1 of {r5, r1}).
    for upper in [4, 8, 15] {
        let mut c = cpu(&[0xeb04, (1 << upper) | (1 << 2)]);
        c.r[4] = RAM;
        c.bus.write(RAM, 0x11223344, 4).unwrap();
        c.bus.write(RAM + 4, 0xaabbccdd, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[2], 0x11223344);
        assert_eq!(c.r[upper], 0xaabbccdd);
        if upper != 4 {
            assert_eq!(c.r[4], RAM);
        }
    }
}

#[test]
fn register_pairs_move_both_words_without_clobbering_the_source() {
    for destination in (0..16).step_by(2) {
        for source in (0..16).step_by(2) {
            let mut c = cpu(&[0x1500 | (source as u16) << 4 | destination as u16]);
            c.r = std::array::from_fn(|i| 0x12345600 + i as u32);
            let before = c.r;
            c.step().unwrap();
            assert_eq!(
                c.r[destination..destination + 2],
                before[source..source + 2]
            );
            assert_eq!(c.pc, XIP + 2);
        }
    }
}

#[test]
fn signed_division_truncates_toward_zero_and_rejects_undefined_cases() {
    for (left, right, expected) in [(128, 2, 64), (-9, 2, -4), (9, -2, -4), (-9, -2, 4)] {
        let mut c = cpu(&[0xe1f4, 0x0101]); // r0 = r0 / r1 (s)
        c.r[0] = left as u32;
        c.r[1] = right as u32;
        c.step().unwrap();
        assert_eq!(c.r[0], expected as u32);
    }
    for (left, right) in [(1, 0), (i32::MIN, -1)] {
        let mut c = cpu(&[0xe1f4, 0x0101]);
        c.r[0] = left as u32;
        c.r[1] = right as u32;
        assert!(c.step().is_err());
    }
}

#[test]
fn three_operand_subtraction_preserves_the_text_canvas_address() {
    for (left, right, expected) in [(RAM + 128, 4, RAM + 124), (0, 1, u32::MAX)] {
        let mut c = cpu(&[0xe0b4, 0x00c2]); // r0 = r12 - r0
        c.r[12] = left;
        c.r[0] = right;
        c.step().unwrap();
        assert_eq!(c.r[0], expected);
        assert_eq!(c.r[12], left);
    }
}

#[test]
fn halfword_postincrement_reads_before_advancing_parameter_pointer() {
    let mut c = cpu(&[0xedd0, 0x2104]); // r2 = h[r0++=20] (u)
    c.r[0] = RAM;
    c.bus.write(RAM, 0xfedc, 2).unwrap();
    c.bus.write(RAM + 20, 0x1234, 2).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[2], 0xfedc);
    assert_eq!(c.r[0], RAM + 20);
}

#[test]
fn fx_signed_halfword_load_reads_before_advancing_the_parameter_pointer() {
    for (value, expected) in [
        (0, 0),
        (0x7fff, 0x7fff),
        (0x8000, 0xffff8000),
        (0xffff, u32::MAX),
    ] {
        let mut c = cpu(&[0xedd4, 0x4062]); // r4 = h[r6++=2] (s), Felucca graph_fx
        c.r[6] = RAM;
        c.bus.write(RAM, value, 2).unwrap();
        c.bus.write(RAM + 2, 0x1234, 2).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[4], expected);
        assert_eq!(c.r[6], RAM + 2);
        assert_eq!(c.pc, XIP + 4);
        assert_eq!(c.bus.read(RAM, 2).unwrap(), value);
        assert_eq!(c.bus.read(RAM + 2, 2).unwrap(), 0x1234);
    }
}

#[test]
fn single_return_address_push_and_pop_restore_the_call_target() {
    let mut c = cpu(&[0x0410, 0x0400]);
    c.sr[14] = RAM + 16;
    c.sr[3] = XIP + 0x200;
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 12);
    assert_eq!(c.bus.read(RAM + 12, 4).unwrap(), XIP + 0x200);
    assert_eq!(c.sr[3], XIP + 0x200);
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 0x200);
    assert_eq!(c.sr[14], RAM + 16);
}

#[test]
fn register_preincrement_store_advances_the_mixer_buffer_pointer() {
    let mut c = cpu(&[0xecdc, 0x5013]); // [++r1=r0] = r5
    c.r[0] = 4;
    c.r[1] = RAM;
    c.r[5] = 0x11223344;
    c.step().unwrap();
    assert_eq!(c.r[1], RAM + 4);
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), 0x11223344);
    assert_eq!(c.bus.read(RAM, 4).unwrap(), 0);
}

#[test]
fn byte_postincrement_store_updates_the_event_cursor() {
    let mut c = cpu(&[0xeed2, 0x2011]); // b[r1++=1] = r2
    c.r[1] = RAM;
    c.r[2] = 0x12345678;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM, 1).unwrap(), 0x78);
    assert_eq!(c.bus.read(RAM + 1, 1).unwrap(), 0);
    assert_eq!(c.r[1], RAM + 1);
}
#[test]
fn immediate_arithmetic_shift_extends_the_sign_in_mixer_interpolation() {
    for (value, expected) in [(65536, 2), (-65536, -2), (-1, -1)] {
        let mut c = cpu(&[0xaf88]); // r0 = r0 >>> 15
        c.r[0] = value as u32;
        c.step().unwrap();
        assert_eq!(c.r[0], expected as u32);
    }
}

#[test]
fn extended_halfword_load_separates_sign_extension_from_the_offset() {
    for (h, expected) in [(0xed51, 0xfedc), (0xed55, 0xfffffedc)] {
        let mut c = cpu(&[h, 0x120c]); // r1 = h[r0+300] (u/s)
        c.r[0] = RAM + 1024;
        c.bus.write(RAM + 1324, 0xfedc, 2).unwrap();
        c.bus.write(RAM + 300, 0x1234, 2).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[1], expected);
        assert_eq!(c.r[0], RAM + 1024);
    }
    let mut c = cpu(&[0xed51, 0x1e85]); // h[r8+484] = r1
    c.r[8] = RAM;
    c.r[1] = 0xabcdef12;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 484, 2).unwrap(), 0xef12);
}

#[test]
fn signed_minimum_clips_audio_with_signed_comparison() {
    for (left, right, expected) in [(-10, 32767, -10), (40000, 32767, 32767), (-10, -20, -20)] {
        let mut c = cpu(&[0xe435, 0x0031]); // r0 = smin(r3, r0)
        c.r[3] = left as u32;
        c.r[0] = right as u32;
        c.step().unwrap();
        assert_eq!(c.r[0], expected as u32);
    }
}

#[test]
fn absolute_and_maximum_compute_the_audio_peak() {
    for (value, expected) in [(-123, 123), (123, 123), (i32::MIN, i32::MIN)] {
        let mut c = cpu(&[0xe430, 0x1500]); // r1 = abs(r5)
        c.r[5] = value as u32;
        c.step().unwrap();
        assert_eq!(c.r[1], expected as u32);
    }
    for (mode, expected) in [(0, u32::MAX), (1, 123)] {
        let mut c = cpu(&[0xe434, 0x1130 | mode]); // r1 = u/smax(r3, r1)
        c.r[3] = u32::MAX;
        c.r[1] = 123;
        c.step().unwrap();
        assert_eq!(c.r[1], expected);
    }
}

#[test]
fn halfword_register_preincrement_reads_signed_lookup_values() {
    let mut c = cpu(&[0xeddc, 0x3312]); // r3 = h[++r1=r3] (s)
    c.r[1] = 4;
    c.r[3] = RAM;
    c.bus.write(RAM + 4, 0xfedc, 2).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[1], RAM + 4);
    assert_eq!(c.r[3], 0xfffffedc);
}

#[test]
fn halfword_register_preincrement_stores_the_low_or_high_half() {
    // Vendor objdump: eddc 4651 is h[++r5=r6] = r4, eddc 4653 is h[++r5=r6] = r4.h
    // (SLOOP's sequencer event ring, PC 0x02002e86 of sloop-plus).
    for (x, expected) in [(0x4651u16, 0x5678u32), (0x4653, 0x1234)] {
        let mut c = cpu(&[0xeddc, x]);
        c.r[5] = 6;
        c.r[6] = RAM;
        c.r[4] = 0x1234_5678;
        c.bus.write(RAM + 4, 0xaaaa_aaaa, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[5], RAM + 6);
        assert_eq!(c.bus.read(RAM + 6, 2).unwrap(), expected);
        assert_eq!(c.bus.read(RAM + 4, 2).unwrap(), 0xaaaa); // the neighbour untouched
    }
}

#[test]
fn memory_shift_scales_the_stereo_output_word() {
    let mut c = cpu(&[0xe86c, 0x3704]); // [r3+4] <<= 7
    c.r[3] = RAM;
    c.bus.write(RAM, 123, 4).unwrap();
    c.bus.write(RAM + 4, (-100i32) as u32, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 4, 4).unwrap(), (-12800i32) as u32);
    assert_eq!(c.bus.read(RAM, 4).unwrap(), 123);
    assert_eq!(c.r[3], RAM);
}

#[test]
fn byte_and_halfword_stack_accesses_preserve_adjacent_fields() {
    let mut c = cpu(&[
        0xe9de, 0x814a, 0xe9d8, 0x8149, 0xe9dd, 0x114a, 0xe9d9, 0x2148,
    ]);
    c.sr[14] = RAM;
    c.r[8] = 0xabcdfedc;
    c.bus.write(RAM + 328, 0x11223344, 4).unwrap();
    c.step().unwrap(); // b[sp+330] = r8
    c.step().unwrap(); // h[sp+328] = r8
    assert_eq!(c.bus.read(RAM + 328, 4).unwrap(), 0x11dcfedc);
    c.step().unwrap(); // r1 = b[sp+330] (s)
    c.step().unwrap(); // r2 = h[sp+328] (s)
    assert_eq!(c.r[1], 0xffffffdc);
    assert_eq!(c.r[2], 0xfffffedc);
    assert_eq!(c.sr[14], RAM);
}

#[test]
fn register_pair_clear_uses_the_encoded_even_register() {
    for destination in (0..16).step_by(2) {
        let mut c = cpu(&[0x1480 | destination as u16]);
        c.r = std::array::from_fn(|i| 100 + i as u32);
        let mut expected = c.r;
        expected[destination..destination + 2].fill(0);
        c.step().unwrap();
        assert_eq!(c.r, expected);
    }
}

#[test]
fn extended_byte_postincrement_reads_before_advancing_the_string() {
    for (h, expected) in [(0xeed0, 0xdc), (0xeed4, 0xffffffdc)] {
        let mut c = cpu(&[h, 0x9001]); // r9 = b[r0++=1] (u/s)
        c.r[0] = RAM;
        c.bus.write(RAM, 0xdc, 1).unwrap();
        c.bus.write(RAM + 1, 0x12, 1).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[9], expected);
        assert_eq!(c.r[0], RAM + 1);
    }
}

#[test]
fn signed_immediate_conditional_adds_a_minus_only_for_negative_numbers() {
    for (value, expected_pc) in [
        (-1, XIP + 4),
        (-120, XIP + 4),
        (0, XIP + 10),
        (120, XIP + 10),
    ] {
        let mut c = cpu(&[0xeeb1, 0x8fff, 0x2140, 0, 0, 0]); // ifs (r1 <= -1) { 3 instructions }
        c.r[1] = value as u32;
        c.step().unwrap();
        assert_eq!(c.pc, expected_pc);
    }
}

#[test]
fn packed_subtraction_centers_the_oscillator_waveform() {
    for (value, expected) in [(32768, 0), (0, 0xffff8000), (65535, 32767)] {
        let mut c = cpu(&[0xe0f4, 0x4c00]); // r4 = r4 - 0x8000
        c.r[4] = value;
        c.step().unwrap();
        assert_eq!(c.r[4], expected);
    }
}

fn p33(c: &mut Bus, command: u8, address: u8, value: u8) {
    c.write(0x13e08, 1, 4).unwrap();
    for byte in [command, address, value] {
        c.write(0x13e0c, byte as u32, 4).unwrap();
        c.write(0x13e08, 17, 4).unwrap();
    }
    c.write(0x13e08, 0, 4).unwrap();
}
#[test]
fn watchdog_is_fed_via_serial_p33_and_really_expires() {
    let mut b = Bus::new(vec![0, 0]).unwrap();
    p33(&mut b, 0, 0x80, 0x1a);
    b.system.advance(23_999_999).unwrap();
    p33(&mut b, 0x20, 0x80, 0x40);
    assert_eq!(b.system.watchdog_feeds, 1);
    b.system.advance(23_999_999).unwrap();
    assert!(b.system.advance(1).is_err());
}

#[test]
fn p33_rtc_registers_are_separate_from_the_watchdog_domain() {
    let mut b = Bus::new(vec![0, 0]).unwrap();
    p33(&mut b, 0, 0x80, 0x1a);
    for (command, value) in [(0, 0x40), (0x80, 0)] {
        b.write(0x13e08, 0x101, 4).unwrap();
        for byte in [command, 0x80, value] {
            b.write(0x13e0c, byte as u32, 4).unwrap();
            b.write(0x13e08, 0x111, 4).unwrap();
        }
        b.write(0x13e08, 0, 4).unwrap();
    }
    assert_eq!(b.read(0x13e0c, 4).unwrap(), 0x40);
    assert_eq!(b.system.watchdog_feeds, 0);
    p33(&mut b, 0x80, 0x80, 0);
    assert_eq!(b.read(0x13e0c, 4).unwrap(), 0x1a);
}
#[test]
fn guarded_ram_rejects_writes_and_usb_dma_checks_its_address() {
    let mut b = Bus::new(vec![0, 0]).unwrap();
    b.write(0x1eee2c0, RAM, 4).unwrap();
    b.write(0x1eee280, RAM + 255, 4).unwrap();
    b.write(0x1eee348, 1, 4).unwrap();
    assert!(b.write(RAM, 0, 4).is_err());
    b.write(RAM + 256, 0, 4).unwrap();
    b.write(0x11800, 4, 4).unwrap();
    b.write(0x51000, 0x40, 4).unwrap();
    b.advance_usb(1).unwrap();
    assert!(b.advance_usb(120000).is_err());
}

#[test]
fn stock_protection_setup_acknowledges_events_without_enabling_sdram() {
    let mut b = Bus::new(vec![0, 0]).unwrap();
    b.write(0x13400, 0x6d, 4).unwrap();
    assert_eq!(b.read(0x13400, 4).unwrap(), 0x2d);
    b.write(0x40438, 0, 4).unwrap();
    assert!(b.write(0x40438, 1, 4).is_err());
    b.write(0x1eef2d4, u32::MAX, 4).unwrap();
    assert_eq!(b.read(0x1eef2d4, 4).unwrap(), 0);
}

// Forms found by scripts/op-scan.sh (examples/op_scan) in Felucca, Jangada
// and SLOOP code; texts are the JieLi objdump's.

#[test]
fn special_register_push_and_pop_cover_every_mask() {
    // Vendor objdump 0x04c0-0x04ff: [--sp] = {psr, sr4, rets, retx, rete,
    // reti} for bits 5..0; 0x0480-0x04bf pop the same sets. Every interrupt
    // stub of fm1_vec.S (Felucca/Jangada/SLOOP) starts with 04c8,
    // [--sp] = {rets}. The lowest register is at the lowest address, as the
    // irq frame pushes (04e9) and pops (04a9) the firmware relies on.
    for mask in 0..64u16 {
        let mut c = cpu(&[0x04c0 | mask, 0x0480 | mask]);
        c.sr[14] = RAM + 0x100;
        for i in 0..6 {
            c.sr[i] = 0x1000 + i as u32;
        }
        c.step().unwrap();
        let count = mask.count_ones();
        assert_eq!(c.sr[14], RAM + 0x100 - 4 * count, "mask {mask:#x}");
        let mut address = c.sr[14];
        for i in 0..6 {
            if mask & (1 << i) != 0 {
                assert_eq!(c.bus.read(address, 4).unwrap(), 0x1000 + i as u32);
                address += 4;
            }
        }
        for i in 0..6 {
            c.sr[i] = 0;
        }
        c.step().unwrap();
        assert_eq!(c.sr[14], RAM + 0x100);
        for i in 0..6 {
            let expected = if mask & (1 << i) != 0 {
                0x1000 + i as u32
            } else {
                0
            };
            assert_eq!(c.sr[i], expected, "mask {mask:#x} sr{i}");
        }
        assert_eq!(c.pc, XIP + 4);
    }
}

#[test]
fn special_register_mask_push_saves_the_fatal_frame() {
    // fm1_vec.S fm1_fatal_common: e958 782f is
    // [--sp] = {sp, ssp, usp, icfg, psr, rets, retx, rete, reti}; fm1_fault_c
    // reads it back as f[16] reti .. f[24] sp, so the lowest special register
    // is at the lowest address. e950 382f pops the same set without sp.
    let mut c = cpu(&[0xe958, 0x782f, 0xe950, 0x382f]);
    c.sr = std::array::from_fn(|i| 0x100 + i as u32);
    c.sr[14] = RAM + 0x100;
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 0x100 - 36);
    let saved: Vec<u32> = (0..9)
        .map(|i| c.bus.read(RAM + 0x100 - 36 + 4 * i, 4).unwrap())
        .collect();
    // sp is saved as it was before the push (UNCERTAIN: objdump cannot say).
    assert_eq!(
        saved,
        [
            0x100,
            0x101,
            0x102,
            0x103,
            0x105,
            0x10b,
            0x10c,
            0x10d,
            RAM + 0x100
        ]
    );
    for i in [0, 1, 2, 3, 5, 11, 12, 13] {
        c.sr[i] = 0;
    }
    c.step().unwrap();
    assert_eq!(c.sr[14], RAM + 0x100 - 4);
    for i in [0, 1, 2, 3, 5, 11, 12, 13] {
        assert_eq!(c.sr[i], 0x100 + i as u32, "sr{i}");
    }
    assert_eq!(c.pc, XIP + 8);
}

#[test]
fn special_register_mask_pop_of_pc_returns() {
    // Stock FM-1 0x02043854: e950 8000, {pc} = [sp++].
    let mut c = cpu(&[0xe950, 0x8000]);
    c.sr[14] = RAM + 0x100;
    c.bus.write(RAM + 0x100, XIP + 0x40, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 0x40);
    assert_eq!(c.sr[14], RAM + 0x104);
}

#[test]
fn trigger_is_a_debug_event_that_continues() {
    // e870 0000 `trigger`: the SDK's ___trig (jl_fft.c) is followed by a
    // printf, so execution continues; fm1_fatal_common starts with it.
    let mut c = cpu(&[0xe870, 0x0000]);
    c.r = std::array::from_fn(|i| i as u32);
    let (r, sr) = (c.r, c.sr);
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 4);
    assert_eq!((c.r, c.sr), (r, sr));
}

/// Where a one-instruction conditional block `[h, x, r1 = 1]` continues.
fn conditional_target(h: u16, x: u16, register: usize, value: u32) -> u32 {
    let mut c = cpu(&[h, x, 0x2141, 0x2140]);
    c.r[register] = value;
    c.step().unwrap();
    c.pc - XIP
}

#[test]
fn conditional_blocks_compare_with_packed_immediates() {
    // ota_session (Felucca/Jangada/SLOOP): ec23 0ba0 `if (r3 > 81920) {`.
    for (value, enters) in [(81921, true), (81920, false), (0xffff_ffff, true)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xec23, 0x0ba0, 3, value), expected);
    }
    // e9a3 0ba0 `if (r3 < 81920) {` (unsigned).
    for (value, enters) in [(81919, true), (81920, false), (0xffff_ffff, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xe9a3, 0x0ba0, 3, value), expected);
    }
    // FM-1_093 0x020a3008: ee21 0e5e `ifs (r1 > 3552) {` (signed).
    for (value, enters) in [(3553, true), (3552, false), (-5i32 as u32, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xee21, 0x0e5e, 1, value), expected);
    }
}

#[test]
fn unsigned_conditional_immediates_are_not_sign_extended() {
    // clang -target pi32v2 -O2 for `if (x < 3000u) y = y * 3 + 1;`:
    // r2 = r1 * 0x3; r2 += 1; if (r0 >= 3000) { r2 = r1 }; r0 = r2; rts.
    // 3000 is 0xbb8: bit 11 set, still an unsigned 3000 (objdump agrees for
    // e93X and e9bX alike).
    for (x, expected) in [(100, 16), (2999, 16), (3000, 5), (4000, 5)] {
        let mut c = cpu(&[
            0xe1e2, 0x1003, 0x21c2, 0xe930, 0x0bb8, 0x1612, 0x1620, 0x0080,
        ]);
        c.r[0] = x;
        c.r[1] = 5;
        c.sr[3] = XIP + 0x40;
        while c.pc != XIP + 0x40 {
            c.step().unwrap();
        }
        assert_eq!(c.r[0], expected, "x = {x}");
    }
    for (value, enters) in [(2999, true), (3000, false), (0xffff_ffff, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xe9b0, 0x0bb8, 0, value), expected); // if (r0 < 3000) {
    }
    // ec33 0ba5 `if (r3 > 2981) {`: imm12 too, not packed.
    for (value, enters) in [(2982, true), (2981, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xec33, 0x0ba5, 3, value), expected);
    }
    // e8b3 0ba5 `if (r3 != -1115) {` and ea33 0ba5
    // `if ((r3 & 0x14A00) != 0) {` (a packed mask).
    for (value, enters) in [(-1115i32 as u32, false), (0, true)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xe8b3, 0x0ba5, 3, value), expected);
    }
    for (value, enters) in [(0x200, true), (0x1_0000, true), (0x1ff, false)] {
        let expected = if enters { 4 } else { 6 };
        assert_eq!(conditional_target(0xea33, 0x0ba5, 3, value), expected);
    }
}

#[test]
fn conditional_blocks_skip_six_byte_compare_branches_whole() {
    // e820 0001 `if (r0 == 1) {` around ff00 0000 0002
    // `if (r0 == 0) goto 4` (6 bytes in objdump), then r1 = 1; r0 = 1.
    let mut c = cpu(&[0xe820, 0x0001, 0xff00, 0x0000, 0x0002, 0x2141, 0x2140]);
    c.r[0] = 5;
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 10);
    c.step().unwrap();
    assert_eq!(c.r[1], 1);
}

// Forms op-scan found in reachable stock FM-1 / FM-1_093 (Baud Girl) code.

#[test]
fn mask_move_uses_the_packed_immediate_forms() {
    // Stock 0x020259ac: e060 3264, r3 = 0x64006400 (was rejected); e060
    // 3164 is r3 = 0x640064 in objdump (was 0x64006400).
    for (x, expected) in [
        (0x3264, 0x6400_6400),
        (0x3164, 0x0064_0064),
        (0x3364, 0x6464_6464),
        (0x3064, 0x64),
    ] {
        let mut c = cpu(&[0xe060, x]);
        c.step().unwrap();
        assert_eq!(c.r[3], expected, "x {x:04x}");
        assert_eq!(c.pc, XIP + 4);
    }
}

#[test]
fn word_postincrement_immediates_are_signed_eleven_bit() {
    // ecd8-ecdf: offset (h & 7) : x[11:8] : x[3:2], signed; x bit 0 store.
    // Stock 0x02071dae: ecdf 4f00 r4 = [r0++=-16]; 0x020407fc: ecdf 3f29
    // [r2++=-8] = r3; ecda 1238 r1 = [r3++=552].
    let mut c = cpu(&[0xecdf, 0x4f00, 0xecdf, 0x3f29, 0xecda, 0x1238]);
    c.r[0] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x1111_2222, 4).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[4], c.r[0]), (0x1111_2222, RAM + 0x100 - 16));
    c.r[2] = RAM + 0x200;
    c.r[3] = 0xabcd_0123;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x200, 4).unwrap(), 0xabcd_0123);
    assert_eq!(c.r[2], RAM + 0x200 - 8);
    c.r[3] = RAM + 0x300;
    c.bus.write(RAM + 0x300, 77, 4).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[1], c.r[3]), (77, RAM + 0x300 + 552));
}

#[test]
fn halfword_postincrement_immediates_are_signed_ten_bit() {
    // edd0-edd7: offset (h & 3) : x[11:8] : x[3:1], signed; h bit 2 is a
    // signed load or a store of the high half. Stock 0x0204068c: edd3 3f0d
    // h[r0++=-4] = r3; edd4 1231 h[r3++=32] = r1.h; edd6 1232
    // r1 = h[r3++=-478] (s).
    let mut c = cpu(&[0xedd3, 0x3f0d, 0xedd4, 0x1231, 0xedd6, 0x1232]);
    c.r[0] = RAM + 0x100;
    c.r[3] = 0x1234_5678;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100, 2).unwrap(), 0x5678);
    assert_eq!(c.r[0], RAM + 0x100 - 4);
    c.r[1] = 0xbeef_cafe;
    c.r[3] = RAM + 0x200;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x200, 2).unwrap(), 0xbeef);
    assert_eq!(c.r[3], RAM + 0x200 + 32);
    c.r[3] = RAM + 0x400;
    c.bus.write(RAM + 0x400, 0xfedc, 2).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[1], c.r[3]), (0xffff_fedc, RAM + 0x400 - 478));
}

#[test]
fn byte_postincrement_immediates_are_signed_nine_bit() {
    // eed0-eed5: offset (h & 1) : x[11:8] : x[3:0], signed. FM-1_093
    // 0x020a7de6: eed1 3f28 r3 = b[r2++=-8] (u); eed3 1230 b[r3++=-224] = r1;
    // eed5 3f28 r3 = b[r2++=-8] (s).
    let mut c = cpu(&[0xeed1, 0x3f28, 0xeed3, 0x1230, 0xeed5, 0x3f28]);
    c.r[2] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0xf0, 1).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[3], c.r[2]), (0xf0, RAM + 0x100 - 8));
    c.r[1] = 0x1234_5678;
    c.r[3] = RAM + 0x200;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x200, 1).unwrap(), 0x78);
    assert_eq!(c.r[3], RAM + 0x200 - 224);
    c.r[2] = RAM + 0x100;
    c.step().unwrap();
    assert_eq!((c.r[3], c.r[2]), (0xffff_fff0, RAM + 0x100 - 8));
}

#[test]
fn register_postincrement_accesses_advance_by_a_register() {
    // x bits 0-1 of ecde/edde/eede: word 2 load, 3 store; halfword 0 load,
    // 1 store, 2 signed load, 3 store of the high half; byte 0 load,
    // 1 store, 2 signed load. Stock 0x0204335a: edde 6120
    // r6 = h[r2++=r1] (u); 0x0200f442: edde 0251 h[r5++=r2] = r0;
    // 0x02021508: eede 2731 b[r3++=r7] = r2.
    let mut c = cpu(&[0xecde, 0x1232, 0xecde, 0x1233]);
    c.r[2] = 12;
    c.r[3] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x5555_aaaa, 4).unwrap();
    c.step().unwrap(); // r1 = [r3++=r2]
    assert_eq!((c.r[1], c.r[3]), (0x5555_aaaa, RAM + 0x10c));
    c.r[1] = 99;
    c.step().unwrap(); // [r3++=r2] = r1
    assert_eq!(c.bus.read(RAM + 0x10c, 4).unwrap(), 99);
    assert_eq!(c.r[3], RAM + 0x118);

    let mut c = cpu(&[
        0xedde, 0x6120, 0xedde, 0x0251, 0xedde, 0x1232, 0xedde, 0x1233,
    ]);
    c.r[1] = 6;
    c.r[2] = RAM + 0x100;
    c.bus.write(RAM + 0x100, 0x8001, 2).unwrap();
    c.step().unwrap(); // r6 = h[r2++=r1] (u)
    assert_eq!((c.r[6], c.r[2]), (0x8001, RAM + 0x106));
    c.r[0] = 0x1234_5678;
    c.r[2] = 2;
    c.r[5] = RAM + 0x200;
    c.step().unwrap(); // h[r5++=r2] = r0
    assert_eq!(c.bus.read(RAM + 0x200, 2).unwrap(), 0x5678);
    assert_eq!(c.r[5], RAM + 0x202);
    c.r[3] = RAM + 0x100;
    c.r[2] = 4;
    c.step().unwrap(); // r1 = h[r3++=r2] (s)
    assert_eq!((c.r[1], c.r[3]), (0xffff_8001, RAM + 0x104));
    c.r[1] = 0xbeef_cafe;
    c.step().unwrap(); // h[r3++=r2] = r1.h
    assert_eq!(c.bus.read(RAM + 0x104, 2).unwrap(), 0xbeef);
    assert_eq!(c.r[3], RAM + 0x108);

    let mut c = cpu(&[0xeede, 0x2731, 0xeede, 0x1232]);
    c.r[2] = 0x1234_56f0;
    c.r[3] = RAM + 0x100;
    c.r[7] = 3;
    c.step().unwrap(); // b[r3++=r7] = r2
    assert_eq!(c.bus.read(RAM + 0x100, 1).unwrap(), 0xf0);
    assert_eq!(c.r[3], RAM + 0x103);
    c.r[3] = RAM + 0x100;
    c.r[2] = 1;
    c.step().unwrap(); // r1 = b[r3++=r2] (s)
    assert_eq!((c.r[1], c.r[3]), (0xffff_fff0, RAM + 0x101));
}

#[test]
fn signed_byte_preincrement_immediate_writes_the_address_back() {
    // Stock 0x020015ee: ee5c 2051, r2 = b[++r5=1] (s); ee5d 2051 is
    // r2 = b[++r5=-255] (s).
    let mut c = cpu(&[0xee5c, 0x2051, 0xee5d, 0x2051]);
    c.r[5] = RAM + 0x100;
    c.bus.write(RAM + 0x101, 0x80, 1).unwrap();
    c.bus.write(RAM + 0x2, 0x7f, 1).unwrap();
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[5]), (0xffff_ff80, RAM + 0x101));
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[5]), (0x7f, RAM + 2));
}

#[test]
fn register_pair_preincrement_by_a_register() {
    // Stock 0x0200940e: ec5c 8012, r9_r8 = d[++r1=r0]; ec5c 8013 stores.
    let mut c = cpu(&[0xec5c, 0x8012, 0xec5c, 0x8013]);
    c.r[0] = 8;
    c.r[1] = RAM + 0x100;
    c.bus.write(RAM + 0x108, 0x1111_1111, 4).unwrap();
    c.bus.write(RAM + 0x10c, 0x2222_2222, 4).unwrap();
    c.step().unwrap();
    assert_eq!(
        (c.r[8], c.r[9], c.r[1]),
        (0x1111_1111, 0x2222_2222, RAM + 0x108)
    );
    c.r[8] = 3;
    c.r[9] = 4;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x110, 4).unwrap(), 3);
    assert_eq!(c.bus.read(RAM + 0x114, 4).unwrap(), 4);
    assert_eq!(c.r[1], RAM + 0x110);
}

#[test]
fn register_pair_postincrement_by_an_immediate() {
    // Felucca 1.0 0x02024c88: ec58 2009, d[r0++=8] = r3_r2 (objdump). The
    // offset is signed(h & 7) << 8 | x[11:8] << 4 | x & 12: ec5f 2f09 is
    // d[r0++=-8]; ec58 2008 the load r3_r2 = d[r0++=8].
    let mut c = cpu(&[0xec58, 0x2009, 0xec5f, 0x2f08, 0xec5c, 0x2001]);
    c.r[0] = RAM + 0x100;
    c.r[2] = 0x1111_1111;
    c.r[3] = 0x2222_2222;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100, 4).unwrap(), 0x1111_1111);
    assert_eq!(c.bus.read(RAM + 0x104, 4).unwrap(), 0x2222_2222);
    assert_eq!(c.r[0], RAM + 0x108);
    c.r[0] = RAM + 0x100;
    c.r[2] = 0;
    c.r[3] = 0;
    c.step().unwrap();
    assert_eq!((c.r[2], c.r[3], c.r[0]), (0x1111_1111, 0x2222_2222, RAM + 0xf8));
    c.r[0] = RAM + 0x800;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x800, 4).unwrap(), 0x1111_1111);
    assert_eq!(c.r[0], RAM + 0x400);
}

#[test]
fn memory_mask_offsets_are_signed_and_xor_is_an_operation() {
    // ef00-efff: bits 7-6 or / xor / and / and-not, bits 5-0 a signed word
    // offset. Stock 0x0208110e: ef3f 0400, [r0+-4] |= 0x80000000.
    for (h, expected) in [
        (0xef3f, 0x8000_1234u32),
        (0xef7f, 0x8000_1234),
        (0xefbf, 0),
        (0xefff, 0x1234),
    ] {
        let mut c = cpu(&[h, 0x0400]);
        c.r[0] = RAM + 0x100;
        c.bus.write(RAM + 0xfc, 0x1234, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.bus.read(RAM + 0xfc, 4).unwrap(), expected, "{h:04x}");
    }
    let mut c = cpu(&[0xef60, 0x0c41]); // [r0+-128] ^= 0xC100
    c.r[0] = RAM + 0x100;
    c.bus.write(RAM + 0x80, 0xff00, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x80, 4).unwrap(), 0x3e00);
}

#[test]
fn signed_sixteen_bit_saturation() {
    // Stock 0x0203f82c: e078 2201, r2 = sat16(r2) (s); e078 2101 reads r1.
    for (value, expected) in [(40000, 32767), (-40000, -32768), (-5, -5), (32767, 32767)] {
        let mut c = cpu(&[0xe078, 0x2101]);
        c.r[1] = value as u32;
        c.step().unwrap();
        assert_eq!(c.r[2], expected as u32);
        assert_eq!(c.r[1], value as u32);
    }
}

/// Where a one-instruction register conditional block `[h, x, r1 = 1]`
/// continues with rN (h & 15) = `lhs` and rC (x >> 8 & 15) = `rhs`: 4 when
/// it enters the block, 6 when it skips it.
fn register_conditional_target(h: u16, x: u16, lhs: u32, rhs: u32) -> u32 {
    let mut c = cpu(&[h, x, 0x2141, 0x2140]);
    c.r[((x >> 8) & 15) as usize] = rhs;
    c.r[(h & 15) as usize] = lhs;
    c.step().unwrap();
    c.pc - XIP
}

#[test]
fn register_conditional_blocks_with_x_bit_7_compare_floats() {
    // JieLi objdump -mattr=+fprev1 (the FM-1's FPU): register conditional
    // blocks whose x has bit 7 set (bit 6 clear) are `iff`, IEEE single
    // compares; `u` also enters when unordered (a NaN operand). Stock RAM
    // code: ed11 0080 iff (r1 >= r0) {, ed92 8b80 iff (r2 u< r11) {.
    let f = |v: f32| v.to_bits();
    let nan = f32::NAN.to_bits();
    let cases: [(u16, f32, f32, bool, bool); 10] = [
        // (h with rN = r3, lhs, rhs, enters, enters when unordered)
        (0xe813, -0.0, 0.0, true, false),  // iff (r3 == r0)
        (0xe893, 1.0, 1.0, false, true),   // iff (r3 u!= r0)
        (0xe913, -1.0, -2.0, true, true),  // iff (r3 u>= r0)
        (0xe993, -2.0, -1.0, true, false), // iff (r3 < r0)
        (0xec13, -1.0, -2.0, true, true),  // iff (r3 u> r0)
        (0xec93, -2.0, -2.0, true, false), // iff (r3 <= r0)
        (0xed13, -1.0, -2.0, true, false), // iff (r3 >= r0)
        (0xed93, -2.0, -1.0, true, true),  // iff (r3 u< r0)
        (0xee13, -1.0, -2.0, true, false), // iff (r3 > r0)
        (0xee93, -2.0, -1.0, true, true),  // iff (r3 u<= r0)
    ];
    for (h, lhs, rhs, enters, unordered) in cases {
        let target = |entered: bool| if entered { 4 } else { 6 };
        assert_eq!(
            register_conditional_target(h, 0x0080, f(lhs), f(rhs)),
            target(enters),
            "{h:04x} {lhs} {rhs}"
        );
        assert_eq!(
            register_conditional_target(h, 0x0080, nan, f(rhs)),
            target(unordered),
            "{h:04x} NaN {rhs}"
        );
    }
    // The low bits of x do not matter (objdump: 0081, 0090 and 00a0 too).
    assert_eq!(
        register_conditional_target(0xed13, 0x0090, f(-1.0), f(-2.0)),
        4
    );
    // ed92 8b80: iff (r2 u< r11) { compares r2 with r11.
    assert_eq!(
        register_conditional_target(0xed92, 0x0b80, f(-3.0), f(-1.0)),
        4
    );
    assert_eq!(
        register_conditional_target(0xed92, 0x0b80, f(1.0), f(-1.0)),
        6
    );
}

#[test]
fn register_bit_test_block_with_x_low_bits_is_a_float_not_equal() {
    // objdump -mattr=+fprev1, ea13 x: 0000 if ((r3 & r0) == 0) {,
    // 0080 if ((r3 & r0) != 0) {, 0081-00bf iff (r3 != r0) { (ordered).
    let f = |v: f32| v.to_bits();
    assert_eq!(register_conditional_target(0xea13, 0x0000, 2, 1), 4);
    assert_eq!(register_conditional_target(0xea13, 0x0080, 3, 1), 4);
    assert_eq!(register_conditional_target(0xea13, 0x0080, 2, 1), 6);
    assert_eq!(
        register_conditional_target(0xea13, 0x0081, f(1.0), f(2.0)),
        4
    );
    assert_eq!(
        register_conditional_target(0xea13, 0x0081, f(-0.0), f(0.0)),
        6
    );
    assert_eq!(
        register_conditional_target(0xea13, 0x00bf, f32::NAN.to_bits(), f(0.0)),
        6
    );
}

#[test]
fn register_conditional_blocks_with_x_bits_7_and_6_are_unsupported() {
    // objdump (with or without the FPU): e813 00c0 is <unknown instruction>.
    let mut c = cpu(&[0xe813, 0x00c0, 0x2141, 0x2140]);
    assert!(c.step().is_err());
}

// Forms op-scan found in the stock RAM code (the copy startup makes to
// 0x01c00000, which the scan now follows; objdump -mattr=+fprev1).

#[test]
fn stack_word_accesses_reach_the_high_registers() {
    // Stock 0x01c075a6: 2709 r9 = [sp+28]; 0x01c07636: 2808 r8 = [sp+32].
    // Bit 3 selects r8-r15: 2889 [sp+32] = r9, 20af [sp+128] = r15.
    let mut c = cpu(&[0x2709, 0x2808, 0x2889, 0x20af]);
    c.sr[14] = RAM + 0x100;
    c.bus.write(RAM + 0x100 + 28, 0x1111_2222, 4).unwrap();
    c.bus.write(RAM + 0x100 + 32, 0x3333_4444, 4).unwrap();
    c.step().unwrap();
    assert_eq!(c.r[9], 0x1111_2222);
    assert_eq!(c.r[1], 0);
    c.step().unwrap();
    assert_eq!(c.r[8], 0x3333_4444);
    c.r[9] = 0x5555_6666;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100 + 32, 4).unwrap(), 0x5555_6666);
    c.r[15] = 0x7777_8888;
    c.step().unwrap();
    assert_eq!(c.bus.read(RAM + 0x100 + 128, 4).unwrap(), 0x7777_8888);
}

#[test]
fn register_list_postincrement_advances_the_base_past_the_list() {
    // Stock 0x01c07e12: eb12 f800 {r15-r11} = [r2++]; eb12 00f0
    // {r7-r4} = [r2++]; 0x01c07f18: eb32 00f0 [r2++] = {r7-r4}. Lowest
    // register at the lowest address, as the other list forms; that the base
    // advances by the list's size is read from the `++` (inferred).
    let mut c = cpu(&[0xeb12, 0xf800, 0xeb12, 0x00f0, 0xeb32, 0x00f0]);
    c.r[2] = RAM + 0x40;
    for i in 0..9 {
        c.bus.write(RAM + 0x40 + 4 * i, 0x100 + i, 4).unwrap();
    }
    c.step().unwrap();
    assert_eq!(c.r[11..16], [0x100, 0x101, 0x102, 0x103, 0x104]);
    assert_eq!(c.r[2], RAM + 0x40 + 20);
    c.step().unwrap();
    assert_eq!(c.r[4..8], [0x105, 0x106, 0x107, 0x108]);
    assert_eq!(c.r[2], RAM + 0x40 + 36);
    c.r[4..8].copy_from_slice(&[0xa, 0xb, 0xc, 0xd]);
    c.step().unwrap();
    for (i, value) in [0xa, 0xb, 0xc, 0xd].into_iter().enumerate() {
        assert_eq!(c.bus.read(RAM + 0x40 + 36 + 4 * i as u32, 4).unwrap(), value);
    }
    assert_eq!(c.r[2], RAM + 0x40 + 52);
}

#[test]
fn data_cache_flush_of_a_line_changes_nothing() {
    // Stock 0x01c00e1e: csync; 0225 flush [r5]; csync, over every line
    // (memory is not cached here, as flushinv [rN]).
    let mut c = cpu(&[0x0225, 0x022f]);
    c.r = std::array::from_fn(|i| RAM + 32 * i as u32);
    let (r, sr) = (c.r, c.sr);
    c.step().unwrap();
    c.step().unwrap();
    assert_eq!((c.r, c.sr, c.pc), (r, sr, XIP + 4));
}
// Packed 16-bit forms (JieLi clang -mcpu=r3 assembles them; encodings and
// text from the vendor objdump). x: rD 15:12, rB 11:8, rA 7:4; the meaning
// of saturation, x2 and the lanes is inferred, see src/simd.rs.

/// One wide instruction on r1 = a, r2 = b, r0 = old; returns r0.
fn simd(h: u16, x: u16, a: u32, b: u32, old: u32) -> u32 {
    let mut c = cpu(&[h, x]);
    c.r[0] = old;
    c.r[1] = a;
    c.r[2] = b;
    c.step().unwrap();
    assert_eq!((c.r[1], c.r[2]), (a, b), "sources unchanged");
    c.r[0]
}

#[test]
fn half_add_and_subtract_wrap_and_keep_the_other_half() {
    // e500 0218: r0.h = r1.l + r2.l; e500 0213: r0.l = r1.l - r2.h
    assert_eq!(
        simd(0xe500, 0x0218, 0x1111_7fff, 0x2222_0001, 0xaaaa_bbbb),
        0x8000_bbbb
    );
    assert_eq!(
        simd(0xe500, 0x0213, 0x0000_0005, 0x0007_0000, 0xaaaa_bbbb),
        0xaaaa_fffe
    );
}

#[test]
fn half_multiply_is_integer_or_q15_with_signed_saturation() {
    // e541 0214: r0.l = r1.h * r2.l (ssat): the product saturated to 16 bits
    assert_eq!(
        simd(0xe541, 0x0214, 0x00c8_0000, 0x0000_0003, 0xaaaa_bbbb),
        0xaaaa_0258
    );
    assert_eq!(simd(0xe541, 0x0214, 0x012c_0000, 0x0000_00c8, 0), 0x7fff);
    assert_eq!(simd(0xe541, 0x0214, 0xfed4_0000, 0x0000_00c8, 0), 0x8000);
    // e543 6654: r6.l = r5.h * r6.l (ssat,x2): (a * b) >> 15, truncated
    let mut c = cpu(&[0xe543, 0x6654]);
    c.r[5] = 0x00ca_1234; // d = 202
    c.r[6] = 0x0000_7fff; // f = 32767
    c.step().unwrap();
    assert_eq!(c.r[6], 0x0000_00c9); // 202 * 32767 >> 15 = 201
                                     // negative products floor; -32768 * -32768 saturates
    assert_eq!(
        simd(0xe543, 0x0218, 0xffff, 0x4001, 0x1234_5678),
        0xffff_5678
    );
    assert_eq!(simd(0xe543, 0x0218, 0x8000, 0x8000, 0), 0x7fff_0000);
}

#[test]
fn half_multiply_into_a_word_keeps_the_full_product() {
    // e551 0212: r0 = r1.l * r2.h (ssat); e553: (ssat,x2)
    assert_eq!(
        simd(0xe551, 0x0212, 0xffff_8000, 0x7fff_0000, 0),
        (-32768i32 * 32767) as u32
    );
    assert_eq!(
        simd(0xe553, 0x0212, 0x0000_c000, 0x4000_0000, 0),
        0xe000_0000
    );
    assert_eq!(
        simd(0xe553, 0x0212, 0x0000_8000, 0x8000_0000, 0),
        0x7fff_ffff
    );
}

#[test]
fn pack_puts_the_first_half_high() {
    // e404 0212: r0 = pack(r1.l, r2.h)
    assert_eq!(
        simd(0xe404, 0x0212, 0x1111_2222, 0x3333_4444, 0),
        0x2222_3333
    );
}

#[test]
fn dual_lanes_take_the_first_element_as_the_high_lane() {
    // e519 0212: r0 = r1.h,r1.l +|+ r2.h,r2.l (ssat); 0216: +|-
    assert_eq!(
        simd(0xe519, 0x0212, 0x7000_0001, 0x2000_fffe, 0),
        0x7fff_ffff
    );
    assert_eq!(
        simd(0xe519, 0x0216, 0x7000_8001, 0x2000_0002, 0),
        0x7fff_8000
    );
    // e569 0212: r0 = r1.h,r1.l *|* r2.h,r2.l (ssat); e56b: (ssat,x2)
    assert_eq!(
        simd(0xe569, 0x0212, 0x0003_0100, 0xfffe_0200, 0),
        0xfffa_7fff
    );
    assert_eq!(
        simd(0xe56b, 0x0212, 0x4000_8000, 0x4000_8000, 0),
        0x2000_7fff
    );
}

#[test]
fn packed_table_sine_kernel_matches_the_c_interpolation() {
    // hal/fm1_simd.h asm_sine_pk body: ix = ph >> 22; w = [tab + ix << 2];
    // f = uextra(ph, p:7, l:15); f.l = w.h * f.l (ssat,x2); w.l += f.l; y = w.l (s)
    let words = [
        0xb694u16, 0xecd8, 0x542a, 0xe1b6, 0x13bc, 0xe543, 0x6654, 0xe500, 0x5650, 0x17d8,
    ];
    let table: Vec<i32> = (0..1024)
        .map(|i| (32767.0 * (2.0 * std::f64::consts::PI * i as f64 / 1024.0).sin()).round() as i32)
        .collect();
    for ph in [
        0u32,
        0x0040_0000,
        0x1234_5678,
        0x8000_0001,
        0xffff_ff80,
        0xc0de_1234,
    ] {
        let mut c = cpu(&words);
        for (i, a) in table.iter().enumerate() {
            let d = table[(i + 1) & 1023] - a;
            c.bus
                .write(
                    RAM + 4 * i as u32,
                    ((d as u32) << 16) | (*a as u32 & 0xffff),
                    4,
                )
                .unwrap();
        }
        c.r[1] = ph;
        c.r[2] = RAM;
        for _ in 0..6 {
            c.step().unwrap();
        }
        let i = (ph >> 22) as usize;
        let (a, b) = (table[i], table[(i + 1) & 1023]);
        let expected = a + (((b - a) * ((ph >> 7) & 0x7fff) as i32) >> 15);
        assert_eq!(c.r[0] as i32, expected, "ph {ph:#x}");
    }
}
