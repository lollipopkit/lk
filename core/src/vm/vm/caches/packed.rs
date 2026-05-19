#[derive(Clone, Copy)]
pub(in crate::vm::vm) enum PackedArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Copy)]
pub(in crate::vm::vm) enum PackedCmpImmOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy)]
pub(in crate::vm::vm) enum PackedCmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone)]
pub(in crate::vm::vm) enum PackedHotKind {
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
    Access {
        dst: u16,
        base: u16,
        field: u16,
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
    MapGetDynamic {
        dst: u16,
        map: u16,
        key: u16,
    },
    StrConcatKnownCap {
        dst: u16,
        a: u16,
        b: u16,
    },
    IntArith {
        op: PackedArithOp,
        dst: u16,
        a: u16,
        b: u16,
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
    StartsWithK {
        dst: u16,
        src: u16,
        key: u16,
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
    Ret {
        base: u16,
        retc: u8,
    },
    ListPush {
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

#[derive(Clone, Copy)]
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
