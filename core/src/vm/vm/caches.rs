use std::sync::Arc;

use crate::val::{RustFunction, RustFunctionNamed, Val};
use crate::vm::bytecode::Function;
use crate::vm::vm::frame::FrameInfo;

// Small polymorphic inline caches (4-way) for property/index access per instruction site.
// This reduces churn at megamorphic sites while staying allocation-free.
#[derive(Clone)]
pub(super) struct MapStrEntry {
    pub(super) map_ptr: usize,
    pub(super) key_ptr: usize,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) struct ObjectStrEntry {
    pub(super) obj_ptr: usize,
    pub(super) key: String,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) enum AccessIc {
    MapStr([Option<MapStrEntry>; 4]),
    ObjectStr([Option<ObjectStrEntry>; 4]),
}

// Per-op inline cache entries reused across VM executions (to avoid reallocation).
#[derive(Clone)]
pub(super) struct ListEntry {
    pub(super) base_ptr: usize,
    pub(super) idx: i64,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) struct StrEntry {
    pub(super) base_ptr: usize,
    pub(super) idx: i64,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) enum IndexIc {
    List([Option<ListEntry>; 4]),
    Str([Option<StrEntry>; 4]),
}

#[derive(Clone)]
pub(super) struct GlobalEntry(
    pub(super) usize, /*name_ptr*/
    pub(super) Val,
    pub(super) u64, /*generation*/
);

pub(super) enum CallIc {
    Rust(RustFunction, u8 /*argc*/),
    RustNamed(RustFunctionNamed, u8 /*argc*/),
    ClosurePositional {
        closure_ptr: usize,
        fun_ptr: *const Function,
        argc: u8,
        cache: ClosureFastCache,
        frame_info: FrameInfo,
    },
    ClosureNamed {
        closure_ptr: usize,
        named_len: u8,
        plan: Arc<NamedCallPlan>,
    },
}

impl Clone for CallIc {
    fn clone(&self) -> Self {
        match self {
            CallIc::Rust(f, argc) => CallIc::Rust(*f, *argc),
            CallIc::RustNamed(f, argc) => CallIc::RustNamed(*f, *argc),
            CallIc::ClosurePositional {
                closure_ptr,
                fun_ptr,
                argc,
                cache,
                frame_info,
            } => CallIc::ClosurePositional {
                closure_ptr: *closure_ptr,
                fun_ptr: *fun_ptr,
                argc: *argc,
                cache: cache.clone(),
                frame_info: frame_info.clone(),
            },
            CallIc::ClosureNamed {
                closure_ptr,
                named_len,
                plan,
            } => CallIc::ClosureNamed {
                closure_ptr: *closure_ptr,
                named_len: *named_len,
                plan: Arc::clone(plan),
            },
        }
    }
}

#[derive(Clone)]
pub(super) struct ClosureFastCache {
    pub(super) regs: Vec<Val>,
    pub(super) access_ic: Vec<Option<AccessIc>>,
    pub(super) index_ic: Vec<Option<IndexIc>>,
    pub(super) global_ic: Vec<Option<GlobalEntry>>,
    pub(super) call_ic: Vec<Option<CallIc>>,
    pub(super) for_range: Vec<Option<ForRangeState>>,
    pub(super) packed_hot: Vec<Option<PackedHotEntry>>,
    pub(super) packed_hot_key: usize,
}

impl ClosureFastCache {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            regs: Vec::new(),
            access_ic: Vec::new(),
            index_ic: Vec::new(),
            global_ic: Vec::new(),
            call_ic: Vec::new(),
            for_range: Vec::new(),
            packed_hot: Vec::new(),
            packed_hot_key: 0,
        }
    }
}

#[derive(Clone)]
pub(super) struct NamedCallPlan {
    pub(super) provided_indices: Arc<[usize]>,
    pub(super) defaults_to_eval: Arc<[usize]>,
    pub(super) optional_nil: Arc<[usize]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ForRangeState {
    pub(super) current: i64,
    pub(super) limit: i64,
    pub(super) step: i64,
    pub(super) inclusive: bool,
    pub(super) positive: bool,
}

impl ForRangeState {
    #[inline]
    pub(super) fn new(current: i64, limit: i64, step: i64, inclusive: bool) -> Self {
        Self {
            current,
            limit,
            step,
            inclusive,
            positive: step > 0,
        }
    }

    #[inline]
    pub(super) fn should_continue(&self) -> bool {
        if self.positive {
            if self.inclusive {
                self.current <= self.limit
            } else {
                self.current < self.limit
            }
        } else if self.inclusive {
            self.current >= self.limit
        } else {
            self.current > self.limit
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum PackedArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Copy)]
pub(super) enum PackedCmpImmOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy)]
pub(super) enum PackedCmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone)]
pub(super) enum PackedHotKind {
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
    ForRangeLoop {
        idx: u16,
        ofs: i16,
    },
    ForRangeStep {
        back_ofs: i16,
    },
    ToStr {
        dst: u16,
        src: u16,
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
}

#[derive(Clone)]
pub(super) struct PackedHotSlot {
    pub(super) word: u32,
    pub(super) next_pc: usize,
    pub(super) kind: PackedHotKind,
}

#[derive(Clone)]
pub(super) enum PackedHotEntry {
    Slot(PackedHotSlot),
    Miss(u32),
}

pub(super) struct VmCaches<'a> {
    pub(super) access_ic: &'a mut Vec<Option<AccessIc>>,
    pub(super) index_ic: &'a mut Vec<Option<IndexIc>>,
    pub(super) global_ic: &'a mut Vec<Option<GlobalEntry>>,
    pub(super) call_ic: &'a mut Vec<Option<CallIc>>,
    pub(super) for_range: &'a mut Vec<Option<ForRangeState>>,
    pub(super) packed_hot: &'a mut Vec<Option<PackedHotEntry>>,
}
