// SPDX-License-Identifier: GPL-3.0-only
// The decoder description examples/op_scan compares with the vendor objdump.
use fm1_emu::describe;

#[test]
fn lengths_follow_the_interpreter() {
    // ffc0 = r0 = imm32, ff80 = call rel32, ff00 = if (r0 == 0) goto (6 bytes).
    for (words, form, length) in [
        (&[0xffc0, 0x1234, 0x5678][..], "MoveImmediate32", Some(6)),
        (&[0xff80, 0, 0], "CallRel32", Some(6)),
        (&[0xff00, 0x0000, 0x0002], "BranchLong", Some(6)),
        (&[0xe040, 0x1234], "MovImm16", Some(4)),
        (&[0xe86c, 0x16fc], "MemoryShift", Some(4)),
        (&[0x1612], "MovReg", Some(2)),
        (&[0x0080], "Return", Some(2)),
    ] {
        let d = describe(words);
        assert_eq!((d.form.as_str(), d.length), (form, length), "{words:04x?}");
        assert!(!d.parallel);
    }
}

#[test]
fn unknown_forms_have_no_length() {
    let d = describe(&[0xe86c, 0x0001]); // memory shift mode 1: not a form
    assert_eq!((d.form.as_str(), d.length), ("Unknown", None));
    let d = describe(&[0xe86c]); // the extension word is missing
    assert_eq!(d.length, None);
}

#[test]
fn conditional_skips_count_six_byte_branches() {
    assert_eq!(describe(&[0xff00, 0, 2]).skip_length, 6);
    assert_eq!(describe(&[0xe86c, 0x16fc]).skip_length, 4);
    assert_eq!(describe(&[0x1612]).skip_length, 2);
}

#[test]
fn parallel_bundles_describe_their_primary_slot() {
    // 0xc000-0xdfff: a two-byte primary slot with its parallel bit cleared.
    let d = describe(&[0xd612]);
    assert!(d.parallel);
    assert_eq!((d.word, d.length), (0x1612, Some(2)));
}
