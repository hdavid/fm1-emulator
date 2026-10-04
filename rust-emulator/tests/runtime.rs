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
fn an_interrupt_preserves_the_unfinished_repeat() {
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
    c.step().unwrap();
    assert_eq!(c.pc, XIP + 6);
    c.bus.write(TIMER5, 0x4000, 4).unwrap();
    c.step().unwrap(); // rti
    while c.pc != XIP + 4 {
        c.step().unwrap();
        assert!(c.steps <= 5);
    }
    assert_eq!(c.r[1], RAM + 12);
    assert_eq!(c.bus.read(RAM + 8, 4).unwrap(), 42);
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
fn register_list_loads_descend_without_changing_the_base() {
    // Vendor form: 04 eb 04 01 => {r8, r2} = [r4+].
    for upper in [4, 8, 15] {
        let mut c = cpu(&[0xeb04, (1 << upper) | (1 << 2)]);
        c.r[4] = RAM;
        c.bus.write(RAM, 0x11223344, 4).unwrap();
        c.bus.write(RAM + 4, 0xaabbccdd, 4).unwrap();
        c.step().unwrap();
        assert_eq!(c.r[upper], 0x11223344);
        assert_eq!(c.r[2], 0xaabbccdd);
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
