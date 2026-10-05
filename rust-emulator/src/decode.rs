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
    BytePostIncrementRegister,
    RotateRightImmediate,
    PushPopMask,
    PushRegs,
    PopPc,
    PushRets,
    PopRegs,
    PushRetsRegs,
    PopRetsRegs,
    PopPcRegs,
    MoveStackPointer,
    PushSpecial,
    PopSpecial,
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
    MultiplyAccumulateLong,
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
    ShiftPair,
    ShiftPairRegister,
    FloatOp,
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
    HalfwordRegisterPreincrementStore,
    WordPostincrementStore,
    WordPostincrementLoad,
    MemoryIndexed,
    PushSpecialMask,
    PopSpecialMask,
    Trigger,
    PairRegisterPreincrement,
    PairPostincrement,
    RegisterPostincrement,
    SaturateSigned16,
    // Packed 16-bit forms (simd.rs).
    HalfAddSubtract,
    HalfMultiply,
    HalfMultiplyWord,
    Pack,
    DualAddSubtract,
    DualMultiply,
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
    } else if h & 0xfc00 == 0x1000 {
        // 0x1000-0x13ff: b[rB++=rI] load/store; the JieLi pi32v2 objdump
        // decodes all 1024 codes this way.
        BytePostIncrementRegister
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
    } else if matches!(h, 0xe8d8 | 0xe8d4 | 0xe8d9 | 0xe8d5) {
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
    } else if h & 0xffc0 == 0x04c0 {
        PushSpecial
    } else if h & 0xffc0 == 0x0480 {
        PopSpecial
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
    } else if matches!(h, 0x0020 | 0x0000 | 0x0001) {
        // 0x0001: idle (wait for interrupt), see Cpu::execute.
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
    } else if h & 0xffe0 == 0x0220 {
        // flush [rN] (0220) and flushinv [rN] (0230), one data cache line.
        CacheFlushInvalidate
    } else if h & 0xe050 == 0x2000 {
        // Bit 3 selects r8-r15 (objdump: 2709 r9 = [sp+28]).
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
    } else if matches!(h, 0xe86c | 0xe86d) && matches!(x & 3, 0 | 2 | 3) {
        MemoryShift
    } else if h == 0xe868 && matches!(x & 3, 0 | 2) {
        MemoryAddRegister
    } else if h & 0xfff8 == 0xed50 || h & 0xfff8 == 0xed58 {
        HalfwordExtended
    } else if h & 0xfff8 == 0xedd0 {
        HalfwordPostincrement
    } else if matches!(h, 0xeed2 | 0xeed3) {
        BytePostincrementStore
    } else if matches!(h, 0xeed0 | 0xeed1 | 0xeed4 | 0xeed5) {
        BytePostincrementLoad
    } else if h & 0xfff0 == 0xe1e0 {
        MultiplyImmediate
    } else if h == 0xe194 && x & 15 <= 3 {
        BitMask
    } else if h == 0xe9d0 {
        StackPair
    } else if h == 0xe078 && x & 0xff == 1 {
        // rD = sat16(rS) (s); the (u) form (x & 0xff == 0) is not decided.
        SaturateSigned16
    } else if h == 0xe958 && x & 0x8000 == 0 {
        // [--sp] = {sp, ssp, ..., reti}: x bit n is sr[n]. Pushing pc is
        // not decided (no firmware uses it).
        PushSpecialMask
    } else if h == 0xe950 && x & 0x4000 == 0 {
        // {pc, ..., reti} = [sp++]; popping sp itself is not decided.
        PopSpecialMask
    } else if h == 0xe870 && x == 0 {
        Trigger
    } else if h == 0xe070 && x & 255 == 0 {
        ReverseBytes
    } else if h & 0xfff0 == 0xe0f0 {
        SubtractPackedImmediate
    } else if h & 0xfff0 == 0xe0a0 {
        ReverseSubtract
    } else if h == 0xe1f8 && x & 15 == 0 {
        MultiplyLong
    } else if h == 0xe1fc && x & 15 == 0 {
        MultiplyAccumulateLong
    } else if h == 0xe1f6 && x & 15 == 0 && (x >> 4) & 1 == 0 {
        // s & 1 == 0 with s = (x >> 4) & 15; x bit 12 (d's low bit) is signed.
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
    } else if h == 0xe1c4 && (x >> 9) & 7 == 0 {
        // "rD = rS <> n" in JieLi's objdump; x bit 11 is another form ("<c>").
        RotateRightImmediate
    } else if h == 0xe1c0 && (x >> 10) & 3 != 1 {
        ShiftExtended
    } else if h == 0xe1d0 && (x >> 10) & 3 != 1 && x & 0x10f0 == 0 {
        // Only the forms seen in the stock and Felucca images: an even
        // pair and a zero middle nibble.
        ShiftPair
    } else if h == 0xe1d8 && x & 0x10fc == 0 {
        // As e1c8 (amount in rC, mode in bits 0-1) on the pair rD+1:rD;
        // only the even-pair, zero-source forms seen in the stock image.
        ShiftPairRegister
    } else if h == 0xe53f && (matches!(x & 15, 0 | 1 | 2 | 3 | 5 | 6 | 7 | 8) || x & 15 == 15) {
        // Single-precision FPU (SDK -mfprev1); only the operations whose
        // meaning the stock image and SDK objects show.
        FloatOp
    } else if h & 0xff80 == 0xff00 && matches!(h & 15, 0 | 1 | 2 | 3 | 8 | 9 | 10 | 11 | 12 | 13) {
        BranchLong
    } else if h == 0xe190 && x & 15 <= 3 {
        LogicThree
    } else if h & 0xffe0 == 0xeb20 && x != 0 {
        // [rN+] = {...} (eb2X) and [rN++] = {...} (eb3X).
        StoreRegisterList
    } else if h & 0xffe0 == 0xeb00 && x != 0 {
        // {...} = [rN+] (eb0X) and {...} = [rN++] (eb1X).
        LoadRegisterList
    } else if matches!(h, 0xe9d8 | 0xe9d9 | 0xe9dc | 0xe9dd | 0xe9de) {
        StackSubword
    } else if h == 0xe9d4 {
        StackExtended
    } else if h & 0xfff8 == 0xec50 {
        MemoryPair
    } else if h & 0xfff8 == 0xec58 && x & 2 == 0 {
        // d[r0++=8] = r3_r2 (ec58 2009, Felucca 1.0 0x02024c88).
        PairPostincrement
    } else if h == 0xec5c && x & 2 != 0 {
        // r9_r8 = d[++r1=r0] (stock 0x0200940e), x bit 0 the store.
        PairRegisterPreincrement
    } else if (h & 0xfff0 == 0xe1a0 && x & 3 == 0) || (h & 0xfff0 == 0xe1b0 && x & 2 == 0) {
        // JieLi objdump: e1aX insert (x bits 0-1 = 0), e1bX uextra (0) or
        // sextra (1); other low bits are not these forms.
        BitField
    } else if h == 0xe840 {
        BranchEqualFlag
    } else if h & 0xfff0 == 0xe850 {
        BranchBit
    } else if h & 0xff00 == 0xef00 {
        MemoryMask
    } else if conditional_kind((h >> 4) & 255)
        && h & 0xf000 == 0xe000
        // Register forms with x bits 7 and 6 set: <unknown> to objdump.
        && !((h >> 4) & 7 == 1 && x & 0xc0 == 0xc0)
    {
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
    ) && (x & 0xe00 == 0 || x & 0xc00 == 0x800)
    {
        // x bit 11: float compare (only the equality and signed kinds).
        BranchCompareRegister
    } else if matches!(
        h,
        0xee50
            | 0xee51
            | 0xee52
            | 0xee53
            | 0xee54
            | 0xee55
            | 0xee58
            | 0xee59
            | 0xee5a
            | 0xee5b
            | 0xee5c
            | 0xee5d
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
    } else if h == 0xeddc && matches!(x & 15, 1 | 3) {
        HalfwordRegisterPreincrementStore
    } else if h & 0xfff8 == 0xecd8 && x & 3 == 1 {
        // ecd8-ecdf: rS advances by a signed 11-bit immediate.
        WordPostincrementStore
    } else if h & 0xfff8 == 0xecd8 && x & 3 == 0 {
        WordPostincrementLoad
    } else if (h == 0xecde && x & 2 != 0) || h == 0xedde || (h == 0xeede && x & 3 != 3) {
        // [rS++=rC]: the access at rS, then rS += rC.
        RegisterPostincrement
    } else if matches!(h, 0xecd8 | 0xedd8 | 0xeed8) {
        MemoryIndexed
    } else {
        decode_simd(h, x)
    }
}

/// The packed 16-bit forms of simd.rs: only the signed-saturating ones whose
/// mode bits the vendor objdump prints (other bits stay unsupported).
fn decode_simd(h: u32, x: u32) -> Op {
    use Op::*;
    if h == 0xe500 {
        HalfAddSubtract
    } else if matches!(h, 0xe541 | 0xe543) && x & 1 == 0 {
        HalfMultiply
    } else if matches!(h, 0xe551 | 0xe553) && x & 9 == 0 {
        HalfMultiplyWord
    } else if h == 0xe404 && x & 9 == 0 {
        Pack
    } else if h & 0xfff3 == 0xe511 {
        DualAddSubtract
    } else if h & 0xfff1 == 0xe561 && x & 12 == 0 {
        DualMultiply
    } else {
        Unsupported
    }
}

/// Length in bytes the interpreter gives a form whose word is `h` (after
/// `primary`); `None` for `Unsupported` and for `Wide` without its extension.
pub(crate) fn length(op: Op) -> Option<u32> {
    use Op::*;
    match op {
        Unsupported | Wide => None,
        MovImm32 | CallRel32 | BranchLong => Some(6),
        MovSpecial | MovMask | MovImm16 | PushPopMask | Rel22 => Some(4),
        // Every form decided by the extension halfword.
        BranchRegisterMask
        | MemoryShift
        | MemoryAddRegister
        | HalfwordExtended
        | HalfwordPostincrement
        | BytePostincrementStore
        | BytePostincrementLoad
        | MultiplyImmediate
        | BitMask
        | StackPair
        | ReverseBytes
        | SubtractPackedImmediate
        | ReverseSubtract
        | MultiplyLong
        | MultiplyAccumulateLong
        | DivideLong
        | CarryArithmetic
        | MultiplyExtended
        | Divide
        | CountLeadingZeros
        | Absolute
        | Maximum
        | Minimum
        | MemoryAdd
        | DecrementBranch
        | MemoryBit
        | AddSubtractExtended
        | ShiftRegisterExtended
        | BytePreincrement
        | RotateRightImmediate
        | ShiftExtended
        | ShiftPair
        | ShiftPairRegister
        | FloatOp
        | LogicThree
        | StoreRegisterList
        | LoadRegisterList
        | StackSubword
        | StackExtended
        | MemoryPair
        | BitField
        | BranchEqualFlag
        | BranchBit
        | MemoryMask
        | ConditionalBlock
        | MemoryLogic
        | LogicImmediate
        | StoreImmediate
        | AddImmediate
        | AddStackExtended
        | AdjustStackExtended
        | BranchCompareImmediate
        | BranchCompareRegister
        | ByteExtended
        | WordExtended
        | WordRegisterPreincrementStore
        | WordRegisterPreincrement
        | HalfwordRegisterPreincrement
        | HalfwordRegisterPreincrementStore
        | WordPostincrementStore
        | WordPostincrementLoad
        | MemoryIndexed
        | PushSpecialMask
        | PopSpecialMask
        | Trigger
        | PairRegisterPreincrement
        | PairPostincrement
        | RegisterPostincrement
        | SaturateSigned16
        | HalfAddSubtract
        | HalfMultiply
        | HalfMultiplyWord
        | Pack
        | DualAddSubtract
        | DualMultiply => Some(4),
        _ => Some(2),
    }
}

/// Length in bytes of the instruction word `h` as a conditional block counts
/// it when it skips instructions (`Cpu::conditional`), from the word alone.
pub(crate) fn skip_length(h: u32) -> u32 {
    // ff00-ff7f: the six-byte compare-branches (objdump), as BranchLong.
    if matches!(h & 0xffe0, 0xffc0 | 0xffe0) || h == 0xff80 || h & 0xff80 == 0xff00 {
        6
    } else if h >> 13 == 7 {
        4
    } else {
        2
    }
}

/// The interpreter's view of one instruction (examples/op_scan compares
/// it with the vendor disassembler).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Description {
    /// The form's name (`Op` variant), "Unsupported" when not decoded.
    pub form: String,
    /// Bytes the interpreter advances over this slot; `None` if unsupported.
    pub length: Option<u32>,
    /// Bytes a conditional block skips for this slot.
    pub skip_length: u32,
    /// The word the interpreter executes for this slot (`primary`).
    pub word: u16,
    /// Whether the word opens a parallel bundle (this slot is its primary).
    pub parallel: bool,
}

/// Describe the instruction whose halfwords start at `words[0]`; the slot of
/// a parallel bundle is described as the interpreter executes it (its
/// primary word, with the bundle's own length).
pub fn describe(words: &[u16]) -> Description {
    let h = words.first().copied().unwrap_or(0) as u32;
    let word = primary(h);
    let mut op = decode(word);
    if op == Op::Wide {
        if let Some(&x) = words.get(1) {
            op = decode_wide(word, x as u32);
        }
    }
    let parallel = is_parallel(h);
    // A bundle's primary slot advances as `Cpu::execute_bundle` does.
    let length = length(op).map(|length| match (parallel, h >> 13 == 6) {
        (true, true) => 2,
        (true, false) => 4,
        (false, _) => length,
    });
    Description {
        form: format!("{op:?}"),
        length,
        skip_length: skip_length(h),
        word: word as u16,
        parallel,
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
            // if (rA < #packed): e9a3 0ba0 (objdump: < 81920).
            | 0x9a
            | 0x9b
            | 0xa1
            | 0xa2
            | 0xa3
            | 0xc1
            // if (rA > #packed): Felucca/SLOOP ota_session ec23 0ba0 (> 81920).
            | 0xc2
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
            | 0xe1
            // ifs (rA > #packed): FM-1_093 0x020a3008 ee21 0e5e (> 3552).
            | 0xe2
            // ifs (rA > #imm12): FM-1_093 0x02004872 ee37 5fff (r7 > -1).
            | 0xe3
            | 0xe9
            // ifs (rA <= #packed): SLOOP's fx.c gain_next and punch.c sweeps
            // (eea3 0d80 = <= 0x1000; eea2 0d08 / 0cb0 = <= 0x2200 / 0x5800).
            | 0xea
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

    #[test]
    fn describe_reports_the_interpreted_form_and_its_length() {
        // r3 = 29392640 (6 bytes), r1 = [r3+32] (2), call (4).
        let d = describe(&[0xffc3, 0x7f00, 0x01c0]);
        assert_eq!((d.form.as_str(), d.length), ("MovImm32", Some(6)));
        assert_eq!(describe(&[0x68b1]).length, Some(2));
        assert_eq!(describe(&[0xea80, 0xc6ba]).length, Some(4));
        // A wide word without its extension halfword is not decided yet.
        assert_eq!(describe(&[0xe86c]).length, None);
        // Bundles: "r6 = r0  #" (0xd606, 2 bytes) and "r0 = r8 >> 8  #"
        // (0xf1c0 0x0888, 4 bytes) execute their primary word.
        let d = describe(&[0xd606]);
        assert!(d.parallel);
        assert_eq!((d.word, d.length), (0x1606, Some(2)));
        let d = describe(&[0xf1c0, 0x0888]);
        assert_eq!((d.word, d.length), (0xe1c0, Some(4)));
        assert_eq!(describe(&[0x0081]).form, "Rti");
        assert_eq!(describe(&[0x0002]).length, None);
    }

    #[test]
    fn every_form_decided_by_the_extension_is_four_bytes() {
        // The packed 16-bit forms (objdump: e500 3430 `r3.l = r3.l + r4.l`,
        // e543 4434, e404 2102, e519 2102, e551 2990) were described as two
        // bytes, which op_scan reports as a length mismatch.
        for h in 0xe000..=0xefffu32 {
            for x in [0u32, 0x0102, 0x2990, 0x3430, 0x4434, 0x8008, 0xffff] {
                let op = decode_wide(h, x);
                if op != Op::Unsupported {
                    assert_eq!(length(op), Some(4), "{h:04x} {x:04x} {op:?}");
                }
            }
        }
    }
}
