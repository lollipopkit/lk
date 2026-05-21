#[derive(Clone, Copy, Debug)]
pub(in crate::vm::vm) enum PackedArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::vm::vm) enum PackedCmpImmOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::vm::vm) enum PackedCmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::vm::vm) enum PackedValueOperand {
    Reg(u16),
    Const(u16),
}

#[derive(Clone, Copy, Debug)]
pub(in crate::vm::vm) enum PackedAddOperand {
    Reg(u16),
    Imm(i16),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::vm::vm) enum PackedHotCallKind {
    Generic,
    NativeFast,
    ClosureExact,
    Exact,
}

#[derive(Clone, Debug)]
pub(in crate::vm::vm) enum PackedHotKind {
    Nop,
    Move {
        dst: u16,
        src: u16,
    },
    LoadK {
        dst: u16,
        kidx: u16,
    },
    LoadLocal {
        dst: u16,
        idx: u16,
    },
    StoreLocal {
        idx: u16,
        src: u16,
    },
    LoadGlobal {
        dst: u16,
        name_k: u16,
    },
    DefineGlobal {
        name_k: u16,
        src: u16,
    },
    LoadCapture {
        dst: u16,
        idx: u16,
    },
    Access {
        dst: u16,
        base: u16,
        field: u16,
    },
    AccessIntArith {
        access_dst: u16,
        base: u16,
        field: u16,
        arith_op: PackedArithOp,
        arith_dst: u16,
        arith_a: u16,
        arith_b: u16,
    },
    AccessK {
        dst: u16,
        base: u16,
        key: u16,
    },
    ListLen {
        dst: u16,
        src: u16,
    },
    MapLen {
        dst: u16,
        src: u16,
    },
    StrLen {
        dst: u16,
        src: u16,
    },
    Len {
        dst: u16,
        src: u16,
    },
    Index {
        dst: u16,
        base: u16,
        idx: u16,
    },
    MapGetInterned {
        dst: u16,
        map: u16,
        key: u16,
    },
    MapGetInternedCmpJmp {
        dst: u16,
        map: u16,
        key: u16,
        op: PackedCmpOp,
        rhs: u16,
        jump_pc: usize,
    },
    MapGetInternedUpsertAdd {
        get_dst: u16,
        cmp_dst: u16,
        map: u16,
        key: u16,
        default: PackedValueOperand,
        default_load: Option<(u16, u16)>,
        add_dst: u16,
        add_rhs: PackedAddOperand,
        write_temps: bool,
    },
    MapGetDynamic {
        dst: u16,
        map: u16,
        key: u16,
    },
    MapGetDynamicCmpJmp {
        dst: u16,
        map: u16,
        key: u16,
        op: PackedCmpOp,
        rhs: u16,
        jump_pc: usize,
    },
    MapGetDynamicUpsertAdd {
        get_dst: u16,
        cmp_dst: u16,
        map: u16,
        key: u16,
        default: PackedValueOperand,
        default_load: Option<(u16, u16)>,
        add_dst: u16,
        add_rhs: PackedAddOperand,
        write_temps: bool,
    },
    MapHas {
        dst: u16,
        map: u16,
        key: u16,
    },
    MapHasIncJmp {
        dst: u16,
        map: u16,
        key: u16,
        inc_r: u16,
        inc_imm: i16,
        true_pc: usize,
        false_pc: usize,
    },
    MapHasK {
        dst: u16,
        map: u16,
        key: u16,
    },
    MapHasKIncJmp {
        dst: u16,
        map: u16,
        key: u16,
        inc_r: u16,
        inc_imm: i16,
        true_pc: usize,
        false_pc: usize,
    },
    StrConcatKnownCap {
        dst: u16,
        a: u16,
        b: u16,
    },
    StrConcatToStr {
        dst: u16,
        lhs: u16,
        src: u16,
    },
    IntArith {
        op: PackedArithOp,
        dst: u16,
        a: u16,
        b: u16,
    },
    IntArithAddIntImm {
        arith_op: PackedArithOp,
        arith_dst: u16,
        arith_a: u16,
        arith_b: u16,
        add_dst: u16,
        add_imm: i16,
    },
    IntArithCmpIntJmp {
        arith_op: PackedArithOp,
        arith_dst: u16,
        arith_a: u16,
        arith_b: u16,
        cmp_op: PackedCmpOp,
        cmp_a: u16,
        cmp_b: u16,
        jump_pc: usize,
    },
    IntArithCmpIntMove {
        arith_op: PackedArithOp,
        arith_dst: u16,
        arith_a: u16,
        arith_b: u16,
        cmp_op: PackedCmpOp,
        cmp_a: u16,
        cmp_b: u16,
        move_dst: u16,
        move_src: u16,
    },
    AddIntFloorDivImm {
        add_dst: u16,
        a: u16,
        b: u16,
        div_dst: u16,
        imm: i16,
    },
    MulIntFloorDivImm {
        mul_dst: u16,
        a: u16,
        b: u16,
        div_dst: u16,
        imm: i16,
    },
    MulIntAddInt {
        mul_dst: u16,
        mul_a: u16,
        mul_b: u16,
        add_dst: u16,
        add_a: u16,
        add_b: u16,
    },
    MulIntMulIntAddInt {
        first_dst: u16,
        first_a: u16,
        first_b: u16,
        second_dst: u16,
        second_a: u16,
        second_b: u16,
        add_dst: u16,
        add_a: u16,
        add_b: u16,
    },
    FloatArith {
        op: PackedArithOp,
        dst: u16,
        a: u16,
        b: u16,
    },
    Floor {
        dst: u16,
        src: u16,
    },
    FloorDivImm {
        dst: u16,
        src: u16,
        imm: i16,
    },
    ToBool {
        dst: u16,
        src: u16,
    },
    StartsWithK {
        dst: u16,
        src: u16,
        key: u16,
    },
    StartsWithKJmp {
        src: u16,
        key: u16,
        ofs: i16,
    },
    ContainsK {
        dst: u16,
        src: u16,
        key: u16,
    },
    ContainsKJmp {
        src: u16,
        key: u16,
        ofs: i16,
    },
    ToIter {
        dst: u16,
        src: u16,
    },
    MapSetInterned {
        map: u16,
        key: u16,
        val: u16,
    },
    MapSetInternedMove {
        map: u16,
        key: u16,
        val: u16,
    },
    BuildList {
        dst: u16,
        base: u16,
        len: u16,
    },
    BuildMap {
        dst: u16,
        base: u16,
        len: u16,
    },
    ForRangePrep {
        idx: u16,
        limit: u16,
        step: u16,
        inclusive: bool,
        explicit: bool,
    },
    ForRangeLoop {
        idx: u16,
        write_idx: bool,
        ofs: i16,
        /// Pre-computed back-PC for ForRangeStep fusion. If the next BC32 word is a
        /// ForRangeStep, this caches the jump target — avoiding a per-iteration
        /// decode of the extension word.
        fusion_back_pc: Option<usize>,
    },
    ForRangeStep {
        back_ofs: i16,
        tail: Option<PackedRangeTail>,
    },
    ToStr {
        dst: u16,
        src: u16,
    },
    ToStrAddRhs {
        tmp: u16,
        src: u16,
        out: u16,
        lhs: u16,
        add_pc: usize,
    },
    MakeClosure {
        dst: u16,
        proto: u16,
    },
    Arith {
        op: PackedArithOp,
        dst: u16,
        a: u16,
        b: u16,
    },
    ArithAddIntImm {
        op: PackedArithOp,
        arith_dst: u16,
        a: u16,
        b: u16,
        add_dst: u16,
        add_imm: i16,
    },
    AddIntImm {
        dst: u16,
        src: u16,
        imm: i16,
    },
    CmpImm {
        op: PackedCmpImmOp,
        dst: u16,
        src: u16,
        imm: i16,
    },
    CmpImmJmp {
        op: PackedCmpImmOp,
        src: u16,
        imm: i16,
        ofs: i16,
    },
    CmpImmMulIntAddInt {
        op: PackedCmpImmOp,
        src: u16,
        imm: i16,
        mul_dst: u16,
        mul_a: u16,
        mul_b: u16,
        add_dst: u16,
        add_a: u16,
        add_b: u16,
        ofs: i16,
    },
    Cmp {
        op: PackedCmpOp,
        dst: u16,
        a: u16,
        b: u16,
    },
    CmpInt {
        op: PackedCmpOp,
        dst: u16,
        a: u16,
        b: u16,
    },
    CmpIntJmp {
        op: PackedCmpOp,
        a: u16,
        b: u16,
        ofs: i16,
    },
    CmpIntMove {
        op: PackedCmpOp,
        a: u16,
        b: u16,
        dst: u16,
        src: u16,
        ofs: i16,
    },
    CmpIntAddIntImm {
        op: PackedCmpOp,
        a: u16,
        b: u16,
        dst: u16,
        src: u16,
        imm: i16,
        ofs: i16,
    },
    CmpIntSubAccessSub {
        op: PackedCmpOp,
        a: u16,
        b: u16,
        first_dst: u16,
        first_a: u16,
        first_b: u16,
        access_pc: usize,
        access_dst: u16,
        access_base: u16,
        access_field: u16,
        final_dst: u16,
        final_a: u16,
        final_b: u16,
        ofs: i16,
    },
    CmpJmp {
        op: PackedCmpOp,
        a: u16,
        b: u16,
        ofs: i16,
    },
    Jmp {
        ofs: i16,
    },
    JmpFalse {
        r: u16,
        ofs: i16,
    },
    JmpFalseSet {
        r: u16,
        dst: u16,
        ofs: i16,
    },
    JmpTrueSet {
        r: u16,
        dst: u16,
        ofs: i16,
    },
    Ret {
        base: u16,
        retc: u8,
    },
    ListPush {
        list: u16,
        val: u16,
    },
    ListPushMove {
        list: u16,
        val: u16,
    },
    MapSet {
        map: u16,
        key: u16,
        val: u16,
    },
    MapSetMove {
        map: u16,
        key: u16,
        val: u16,
    },
    CallNativeFast {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    CallClosureExact {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    CallExact {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    CallMethod0 {
        dst: u16,
        receiver: u16,
        method: u16,
    },
    CallGlobalMethod0 {
        dst: u16,
        receiver: u16,
        method: u16,
    },
    Call {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    MoveCall {
        moves: Vec<(u16, u16)>,
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
        call_kind: PackedHotCallKind,
    },
    CmpLtImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
    CmpLeImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
    AddIntImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
}

#[derive(Clone, Copy, Debug)]
pub(in crate::vm::vm) struct PackedRangeTail {
    pub(in crate::vm::vm) guard_pc: usize,
    pub(in crate::vm::vm) body_pc: usize,
    pub(in crate::vm::vm) exit_pc: usize,
    pub(in crate::vm::vm) idx: u16,
    pub(in crate::vm::vm) write_idx: bool,
}

#[derive(Clone)]
pub(in crate::vm::vm) struct PackedHotSlot {
    pub(in crate::vm::vm) word: u32,
    pub(in crate::vm::vm) next_pc: usize,
    pub(in crate::vm::vm) kind: PackedHotKind,
}

#[derive(Clone)]
pub(in crate::vm::vm) enum PackedHotEntry {
    Slot(PackedHotSlot),
    Miss(u32),
}
