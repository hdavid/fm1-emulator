// SPDX-License-Identifier: GPL-3.0-only
// Instruction classification, separated from execution so that a decoded
// form can be cached per PC (see code_cache.rs). The conditions below are
// the instruction tests of the interpreter's if/else chains, in their
// original order: the first matching form wins. Execution lives in
// `Cpu::execute` (cpu.rs) and `extended::execute`.

/// One instruction form. `Wide` means the 16-bit word alone does not decide:
/// the form depends on the following extension halfword (`decode_wide`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Op {
    // cpu.rs forms.
    MovImm32,
    RepeatRegister,
    RepeatImmediate,
    MovSpecial,
    MovMask,
    MovImm16,
    MovImm8,
    MovNegative,
    MovReg,
    AddSub,
    AddImm8,
    AddSp,
    AddSmall,
    Logic,
    Asr,
    Shift,
    LoadStore32,
    PushPopMask,
    PushRegs,
    PopPc,
    PushRets,
    PopRegs,
    PushRetsRegs,
    PopRetsRegs,
    PopPcRegs,
    MoveStackPointer,
    PushIrqFrame,
    PopIrqFrame,
    CallRel32,
    Rel22,
    CallRel9,
    GotoRel12,
    BranchRegister,
    TestsetByte,
    Return,
    CallReg,
    /// RTI is only valid in interrupt context; otherwise it is unsupported.
    Rti,
    Cli,
    Lock,
    Sti,
    Nop,
    // extended.rs 16-bit forms.
    MoveRegisterPair,
    Multiply,
    ClearPair,
    AddRegister,
    ClearHighRegister,
    CacheFlushInvalidate,
    StackWord,
    Extend,
    BitRegister,
    ShiftRegister,
    AddStack,
    GotoRegister,
    Ssync,
    TableBranch,
    MemorySmall,
    MemoryPostincrement,
    Wide,
    // extended.rs 32-bit (and 48-bit) forms, decided with the extension word.
    BranchRegisterMask,
    MemoryShift,
    MemoryAddRegister,
    HalfwordExtended,
    HalfwordPostincrement,
    BytePostincrementStore,
    BytePostincrementLoad,
    MultiplyImmediate,
    BitMask,
    StackPair,
    ReverseBytes,
    SubtractPackedImmediate,
    ReverseSubtract,
    MultiplyLong,
    DivideLong,
    CarryArithmetic,
    MultiplyExtended,
    Divide,
    CountLeadingZeros,
    Absolute,
    Maximum,
    Minimum,
    MemoryAdd,
    DecrementBranch,
    MemoryBit,
    AddSubtractExtended,
    ShiftRegisterExtended,
    BytePreincrement,
    ShiftExtended,
    BranchLong,
    LogicThree,
    StoreRegisterList,
    LoadRegisterList,
    StackSubword,
    StackExtended,
    MemoryPair,
    BitField,
    BranchEqualFlag,
    BranchBit,
    MemoryMask,
    ConditionalBlock,
    MemoryLogic,
    LogicImmediate,
    StoreImmediate,
    AddImmediate,
    AddStackExtended,
    AdjustStackExtended,
    BranchCompareImmediate,
    BranchCompareRegister,
    ByteExtended,
    WordExtended,
    WordRegisterPreincrementStore,
    WordRegisterPreincrement,
    HalfwordRegisterPreincrement,
    WordPostincrementStore,
    WordPostincrementLoad,
    MemoryIndexed,
    Unsupported,
}

/// Whether `h` opens a parallel bundle (a primary slot plus a following one).
pub(crate) fn is_parallel(h: u32) -> bool {
    h >> 13 == 6 || h & 0xf800 == 0xf000
}

/// The word a bundle's primary slot executes: `h` without its parallel bit.
pub(crate) fn primary(h: u32) -> u32 {
    if h >> 13 == 6 {
        h & 0x1fff
    } else if h & 0xf800 == 0xf000 {
        h & !0x1000
    } else {
        h
    }
}

/// Classify a 16-bit instruction word (the cpu.rs chain, then extended.rs).
pub(crate) fn decode(h: u32) -> Op {
    use Op::*;
    if matches!(h & 0xfff0, 0xffc0 | 0xffe0) {
        MovImm32
    } else if h & 0xff00 == 0x0300 {
        RepeatRegister
    } else if h & 0xe00f == 0x8000 {
        RepeatImmediate
    } else if h == 0xe064 {
        MovSpecial
    } else if h == 0xe060 {
        MovMask
    } else if h & 0xfff0 == 0xe040 {
        MovImm16
    } else if h & 0xe0c0 == 0x2040 {
        MovImm8
    } else if h & 0xe0f8 == 0x2010 {
        MovNegative
    } else if h & 0xff00 == 0x1600 {
        MovReg
    } else if matches!(h & 0xfe00, 0x1c00 | 0x1e00) {
        AddSub
    } else if h & 0xe0c0 == 0x20c0 {
        AddImm8
    } else if h & 0xe01f == 0x8002 {
        AddSp
    } else if h & 0xe088 == 0x8008 {
        AddSmall
    } else if matches!(h & 0xff88, 0x1900 | 0x1908 | 0x1980 | 0x1988) {
        Logic
    } else if h & 0xe088 == 0xa088 {
        Asr
    } else if h & 0xe008 == 0xa000 {
        Shift
    } else if h & 0xe008 == 0x6000 {
        LoadStore32
    } else if h == 0xe8d8 || h == 0xe8d4 {
        PushPopMask
    } else if h & 0xfff0 == 0x0460 {
        PushRegs
    } else if h == 0x0400 {
        PopPc
    } else if h == 0x0410 {
        PushRets
    } else if h & 0xfff0 == 0x0440 {
        PopRegs
    } else if h & 0xfff0 == 0x0470 && h & 15 >= 4 {
        PushRetsRegs
    } else if h & 0xfff0 == 0x0430 && h & 15 >= 4 {
        PopRetsRegs
    } else if h & 0xfff0 == 0x0450 && h & 15 >= 4 {
        PopPcRegs
    } else if matches!(h, 0x1440..=0x1443) {
        MoveStackPointer
    } else if matches!(h, 0x04e8 | 0x04e9) {
        PushIrqFrame
    } else if matches!(h, 0x04a8 | 0x04a9) {
        PopIrqFrame
    } else if h == 0xff80 {
        CallRel32
    } else if matches!(h & 0xffc0, 0xea80 | 0xeac0) {
        Rel22
    } else if h & 0xe08f == 0x8001 {
        CallRel9
    } else if h & 0xe00c == 0x8004 {
        GotoRel12
    } else if h & 0xe008 == 0x4000 {
        BranchRegister
    } else if h & 0xfff0 == 0x00b0 {
        TestsetByte
    } else if h == 0x0080 {
        Return
    } else if h & 0xfff0 == 0x00c0 {
        CallReg
    } else if h == 0x0081 {
        // Outside interrupt context the rest of the chain does not match
        // 0x0081 either (checked by `rti_outside_interrupts_is_unsupported`).
        Rti
    } else if h == 0x0060 {
        Cli
    } else if matches!(h, 0x0040 | 0x0041) {
        Lock
    } else if h == 0x0061 {
        Sti
    } else if h == 0x0020 || h == 0x0000 {
        Nop
    } else {
        decode_extended(h)
    }
}

fn decode_extended(h: u32) -> Op {
    use Op::*;
    if h & 0xff11 == 0x1500 {
        MoveRegisterPair
    } else if h & 0xff00 == 0x1b00 {
        Multiply
    } else if h & 0xfff1 == 0x1480 {
        ClearPair
    } else if h & 0xff00 == 0x1800 {
        AddRegister
    } else if h & 0xfff8 == 0x14c0 {
        ClearHighRegister
    } else if h & 0xfff0 == 0x0230 {
        CacheFlushInvalidate
    } else if h & 0xe058 == 0x2000 {
        StackWord
    } else if matches!(h & 0xff88, 0x1700 | 0x1708 | 0x1780 | 0x1788) {
        Extend
    } else if matches!(h & 0xe0f8, 0x2030 | 0x2038 | 0x20b8) {
        // extended.rs's own `h & 0xe0f8 == 0x2010` form preceded this one but
        // is unreachable: cpu.rs decodes that pattern as MovNegative first.
        BitRegister
    } else if matches!(h & 0xff88, 0x1a00 | 0x1a80 | 0x1a88) {
        ShiftRegister
    } else if h & 0xe098 == 0x8088 {
        AddStack
    } else if h & 0xfff0 == 0x00d0 {
        GotoRegister
    } else if h == 0x0022 {
        Ssync
    } else if matches!(h & 0xfff0, 0x0100 | 0x0110) {
        TableBranch
    } else if matches!(h & 0xe088, 0x4008 | 0x4088 | 0x6008 | 0x6088) {
        MemorySmall
    } else if (0x0500..0x0800).contains(&(h & 0xff80)) {
        MemoryPostincrement
    } else if h >> 13 == 7 {
        Wide
    } else {
        Unsupported
    }
}

/// Classify a `Wide` instruction from its word and extension halfword `x`.
pub(crate) fn decode_wide(h: u32, x: u32) -> Op {
    use Op::*;
    if matches!(h & 0xff00, 0xfa00 | 0xfb00) {
        BranchRegisterMask
    } else if h == 0xe86c && matches!(x & 3, 0 | 2) {
        MemoryShift
    } else if h == 0xe868 {
        MemoryAddRegister
    } else if h & 0xfff8 == 0xed50 || h & 0xfff8 == 0xed58 {
        HalfwordExtended
    } else if matches!(h, 0xedd0 | 0xedd4) {
        HalfwordPostincrement
    } else if h == 0xeed2 {
        BytePostincrementStore
    } else if matches!(h, 0xeed0 | 0xeed4) {
        BytePostincrementLoad
    } else if h & 0xfff0 == 0xe1e0 {
        MultiplyImmediate
    } else if h == 0xe194 && x & 15 <= 3 {
        BitMask
    } else if h == 0xe9d0 {
        StackPair
    } else if h == 0xe070 && x & 255 == 0 {
        ReverseBytes
    } else if h & 0xfff0 == 0xe0f0 {
        SubtractPackedImmediate
    } else if h & 0xfff0 == 0xe0a0 {
        ReverseSubtract
    } else if h == 0xe1f8 && x & 15 == 0 {
        MultiplyLong
    } else if h == 0xe1f6 && x & 15 == 0 && (x >> 12) & 1 == 0 && (x >> 4) & 1 == 0 {
        // d & 1 == 0 && s & 1 == 0 with d = x >> 12, s = (x >> 4) & 15.
        DivideLong
    } else if h == 0xe0b8 && matches!(x & 15, 0 | 2) {
        CarryArithmetic
    } else if h == 0xe1f0 && x & 15 == 0 {
        MultiplyExtended
    } else if h == 0xe1f4 && x & 15 <= 1 {
        Divide
    } else if h == 0xe180 && x & 255 == 0 {
        CountLeadingZeros
    } else if h == 0xe430 && x & 255 == 0 {
        Absolute
    } else if h == 0xe434 && x & 15 <= 1 {
        Maximum
    } else if h == 0xe435 && x & 15 <= 1 {
        Minimum
    } else if h & 0xffe0 == 0xebc0 {
        MemoryAdd
    } else if h & 0xfff0 == 0xea00 {
        DecrementBranch
    } else if h == 0xe866 {
        MemoryBit
    } else if h == 0xe0b4 && matches!(x & 15, 0 | 2) {
        AddSubtractExtended
    } else if h == 0xe1c8 {
        ShiftRegisterExtended
    } else if h == 0xeedc {
        BytePreincrement
    } else if h == 0xe1c0 && (x >> 10) & 3 != 1 {
        ShiftExtended
    } else if h & 0xff80 == 0xff00 && matches!(h & 15, 0 | 1 | 2 | 3 | 8 | 9 | 10 | 11 | 12 | 13) {
        BranchLong
    } else if h == 0xe190 && x & 15 <= 3 {
        LogicThree
    } else if h & 0xfff0 == 0xeb20 && x != 0 {
        StoreRegisterList
    } else if h & 0xfff0 == 0xeb00 && x != 0 {
        LoadRegisterList
    } else if matches!(h, 0xe9d8 | 0xe9d9 | 0xe9dc | 0xe9dd | 0xe9de) {
        StackSubword
    } else if h == 0xe9d4 {
        StackExtended
    } else if h & 0xfff8 == 0xec50 && x & 3 <= 1 {
        MemoryPair
    } else if matches!(h & 0xfff0, 0xe1a0 | 0xe1b0) {
        BitField
    } else if h == 0xe840 {
        BranchEqualFlag
    } else if h & 0xfff0 == 0xe850 {
        BranchBit
    } else if matches!(h & 0xffe0, 0xef00 | 0xef80 | 0xefc0) {
        MemoryMask
    } else if conditional_kind((h >> 4) & 255) && h & 0xf000 == 0xe000 {
        ConditionalBlock
    } else if h == 0xe864 {
        MemoryLogic
    } else if matches!(h & 0xfff0, 0xe140 | 0xe150 | 0xe160 | 0xe170) {
        LogicImmediate
    } else if h & 0xffe0 == 0xea40 {
        StoreImmediate
    } else if matches!(h & 0xfff0, 0xe100 | 0xe110 | 0xe120 | 0xe130 | 0xe0e0) {
        AddImmediate
    } else if h == 0xe8f8 {
        AddStackExtended
    } else if h == 0xe8f0 && x >> 13 == 0 {
        AdjustStackExtended
    } else if h & 0xff80 == 0xf800
        || h & 0xff80 == 0xf880
        || matches!(
            h & 0xff80,
            0xf900 | 0xf980 | 0xfc00 | 0xfc80 | 0xfd00 | 0xfd80 | 0xfe00 | 0xfe80
        )
    {
        BranchCompareImmediate
    } else if matches!(
        h & 0xfff0,
        0xe800 | 0xe880 | 0xe900 | 0xe980 | 0xec00 | 0xec80 | 0xed00 | 0xed80 | 0xee00 | 0xee80
    ) && x & 0xe00 == 0
    {
        BranchCompareRegister
    } else if matches!(
        h,
        0xee50 | 0xee51 | 0xee52 | 0xee53 | 0xee54 | 0xee55 | 0xee58 | 0xee59 | 0xee5a | 0xee5b
    ) {
        ByteExtended
    } else if h & 0xfff8 == 0xecd0 {
        WordExtended
    } else if h == 0xecdc && x & 15 == 3 {
        WordRegisterPreincrementStore
    } else if h == 0xecdc && x & 15 == 2 {
        WordRegisterPreincrement
    } else if h == 0xeddc && matches!(x & 15, 0 | 2) {
        HalfwordRegisterPreincrement
    } else if h == 0xecd8 && x & 3 == 1 {
        WordPostincrementStore
    } else if h == 0xecd8 && x & 3 == 0 {
        WordPostincrementLoad
    } else if matches!(h, 0xecd8 | 0xedd8 | 0xeed8) {
        MemoryIndexed
    } else {
        Unsupported
    }
}

fn conditional_kind(kind: u32) -> bool {
    matches!(
        kind,
        0x81 | 0x82
            | 0x83
            | 0x89
            | 0x8a
            | 0x8b
            | 0x91
            | 0x92
            | 0x93
            | 0x99
            | 0x9b
            | 0xa1
            | 0xa2
            | 0xa3
            | 0xc1
            | 0xc3
            | 0xc9
            | 0xca
            | 0xcb
            | 0xd1
            | 0xd2
            | 0xd3
            | 0xd9
            | 0xda
            | 0xdb
            | 0xe9
            | 0xeb
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rti_outside_interrupts_is_unsupported() {
        // `Rti` stands in for 0x0081; the forms after it never match 0x0081.
        assert_eq!(decode_extended(0x0081), Op::Unsupported);
    }

    #[test]
    fn wide_forms_need_the_extension_word() {
        assert_eq!(decode(0xe86c), Op::Wide);
        assert_eq!(decode_wide(0xe86c, 0), Op::MemoryShift);
        assert_eq!(decode_wide(0xe86c, 1), Op::Unsupported);
        assert_eq!(decode(0xe064), Op::MovSpecial);
        assert_eq!(decode(0x2010), Op::MovNegative);
    }
}
