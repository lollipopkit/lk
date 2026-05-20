use std::collections::{BTreeMap, BTreeSet};

use super::{CaptureSpec, Function, FunctionTranslator, LlvmBackendOptions, Op, Val, rk_index, rk_is_const};

pub(super) const DEFAULT_RETURN_LABEL: &str = "block_return_default";

pub(super) fn strip_nested_module_header(ir: &str) -> String {
    ir.lines()
        .filter(|line| {
            !line.starts_with("; ModuleID")
                && !line.starts_with("source_filename")
                && !line.starts_with("target triple")
                && !line.starts_with("declare ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn function_translator_with_captures<'a>(
    function: &'a Function,
    name: &'a str,
    options: &'a LlvmBackendOptions,
    capture_specs: Option<&'a [CaptureSpec]>,
) -> FunctionTranslator<'a> {
    FunctionTranslator::new(function, name, options).with_capture_specs(capture_specs)
}

pub(super) fn known_specialization_key(index: usize, known_params: &BTreeMap<usize, KnownReg>) -> String {
    let mut key = format!("p{index}");
    for (idx, known) in known_params {
        match known {
            KnownReg::StringHandle { text, .. } => {
                key.push_str(&format!("|s{idx}:{}", text.escape_debug()));
            }
            KnownReg::ConstMap { entries } => {
                key.push_str(&format!("|m{idx}:"));
                for (entry_key, value) in entries {
                    key.push_str(&format!("{}={:?};", entry_key.escape_debug(), value));
                }
            }
            _ => {}
        }
    }
    key
}

pub(super) fn infer_integer_parameter_indices(function: &Function) -> BTreeSet<usize> {
    let mut sources = vec![BTreeSet::new(); function.n_regs as usize];
    for (idx, reg) in function.param_regs.iter().copied().enumerate() {
        if let Some(slot) = sources.get_mut(reg as usize) {
            slot.insert(idx);
        }
    }

    let mut required = BTreeSet::new();
    let mut integers = BTreeSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for op in &function.code {
            match *op {
                Op::Move(dst, src) | Op::StoreLocal(dst, src) | Op::LoadLocal(dst, src) => {
                    changed |= copy_sources(&mut sources, dst, src);
                    if operand_is_known_integer(function, &integers, src) {
                        changed |= integers.insert(dst);
                    }
                }
                Op::AddInt(dst, a, b) | Op::SubInt(dst, a, b) | Op::MulInt(dst, a, b) | Op::ModInt(dst, a, b) => {
                    mark_required_sources(&sources, &mut required, a);
                    mark_required_sources(&sources, &mut required, b);
                    changed |= union_sources(&mut sources, dst, a, b);
                    changed |= integers.insert(dst);
                }
                Op::Add(dst, a, b)
                | Op::Sub(dst, a, b)
                | Op::Mul(dst, a, b)
                | Op::Div(dst, a, b)
                | Op::Mod(dst, a, b)
                    if operand_is_known_integer(function, &integers, a)
                        && operand_is_known_integer(function, &integers, b) =>
                {
                    mark_required_sources(&sources, &mut required, a);
                    mark_required_sources(&sources, &mut required, b);
                    changed |= union_sources(&mut sources, dst, a, b);
                    changed |= integers.insert(dst);
                }
                Op::Sub(_, a, b) | Op::Mul(_, a, b) | Op::Div(_, a, b) | Op::Mod(_, a, b) => {
                    if operand_is_const_int(function, a) {
                        mark_required_sources(&sources, &mut required, b);
                    }
                    if operand_is_const_int(function, b) {
                        mark_required_sources(&sources, &mut required, a);
                    }
                }
                Op::AddIntImm(dst, src, _) => {
                    mark_required_sources(&sources, &mut required, src);
                    changed |= copy_sources(&mut sources, dst, src);
                    changed |= integers.insert(dst);
                }
                Op::FloorDivImm { dst, src, .. } => {
                    mark_required_sources(&sources, &mut required, src);
                    changed |= copy_sources(&mut sources, dst, src);
                    changed |= integers.insert(dst);
                }
                Op::AddIntImmJmp { r, .. } => {
                    mark_required_sources(&sources, &mut required, r);
                }
                Op::CmpIntJmp { a, b, .. } => {
                    mark_required_sources(&sources, &mut required, a);
                    mark_required_sources(&sources, &mut required, b);
                }
                Op::Len { dst, .. } | Op::Floor { dst, .. } => {
                    changed |= integers.insert(dst);
                }
                Op::CmpEq(dst, a, b)
                | Op::CmpNe(dst, a, b)
                | Op::CmpLt(dst, a, b)
                | Op::CmpLe(dst, a, b)
                | Op::CmpGt(dst, a, b)
                | Op::CmpGe(dst, a, b)
                | Op::CmpI { dst, a, b, .. } => {
                    if operand_is_const_int(function, a) {
                        mark_required_sources(&sources, &mut required, b);
                    }
                    if operand_is_const_int(function, b) {
                        mark_required_sources(&sources, &mut required, a);
                    }
                    if operand_is_known_integer(function, &integers, a) {
                        mark_required_sources(&sources, &mut required, b);
                    }
                    if operand_is_known_integer(function, &integers, b) {
                        mark_required_sources(&sources, &mut required, a);
                    }
                    changed |= union_sources(&mut sources, dst, a, b);
                }
                Op::CmpEqImm(_, src, _)
                | Op::CmpNeImm(_, src, _)
                | Op::CmpLtImm(_, src, _)
                | Op::CmpLeImm(_, src, _)
                | Op::CmpGtImm(_, src, _)
                | Op::CmpGeImm(_, src, _)
                | Op::CmpLtImmJmp { r: src, .. }
                | Op::CmpLeImmJmp { r: src, .. }
                | Op::CmpEqImmJmp { r: src, .. }
                | Op::CmpGtImmJmp { r: src, .. }
                | Op::CmpGeImmJmp { r: src, .. }
                | Op::CmpNeImmJmp { r: src, .. } => {
                    mark_required_sources(&sources, &mut required, src);
                }
                _ => {}
            }
            for idx in &required {
                if let Some(reg) = function.param_regs.get(*idx) {
                    changed |= integers.insert(*reg);
                }
            }
        }
    }

    required
}

pub(super) fn infer_integer_registers(function: &Function, integer_params: &BTreeSet<usize>) -> BTreeSet<u16> {
    let mut integers = BTreeSet::new();
    for idx in integer_params {
        if let Some(reg) = function.param_regs.get(*idx) {
            integers.insert(*reg);
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for op in &function.code {
            match *op {
                Op::Move(dst, src) | Op::StoreLocal(dst, src) | Op::LoadLocal(dst, src) => {
                    changed |= mark_if_integer(function, &mut integers, dst, src);
                }
                Op::AddInt(dst, a, b)
                | Op::SubInt(dst, a, b)
                | Op::MulInt(dst, a, b)
                | Op::ModInt(dst, a, b)
                | Op::Add(dst, a, b)
                | Op::Sub(dst, a, b)
                | Op::Mul(dst, a, b)
                | Op::Div(dst, a, b)
                | Op::Mod(dst, a, b) => {
                    if operand_is_known_integer(function, &integers, a)
                        && operand_is_known_integer(function, &integers, b)
                    {
                        changed |= integers.insert(dst);
                    }
                }
                Op::AddIntImm(dst, src, _) => {
                    if operand_is_known_integer(function, &integers, src) {
                        changed |= integers.insert(dst);
                    }
                }
                Op::FloorDivImm { dst, .. } => {
                    changed |= integers.insert(dst);
                }
                Op::Len { dst, .. } | Op::Floor { dst, .. } => {
                    changed |= integers.insert(dst);
                }
                _ => {}
            }
        }
    }

    integers
}

pub(super) fn register_is_string_constant_source(function: &Function, reg: u16) -> bool {
    register_is_string_constant_source_inner(function, reg, &mut BTreeSet::new())
}

fn register_is_string_constant_source_inner(function: &Function, reg: u16, seen: &mut BTreeSet<u16>) -> bool {
    if !seen.insert(reg) {
        return false;
    }
    let mut assignment = None;
    for op in &function.code {
        if !op_assigned_regs(op).contains(&reg) {
            continue;
        }
        if assignment.replace(op).is_some() {
            return false;
        }
    }
    let Some(op) = assignment else {
        return false;
    };
    match *op {
        Op::LoadK(dst, kidx) if dst == reg => {
            matches!(function.consts.get(kidx as usize).and_then(Val::as_str), Some(_))
        }
        Op::Move(dst, src) | Op::StoreLocal(dst, src) | Op::LoadLocal(dst, src) if dst == reg => {
            register_is_string_constant_source_inner(function, src, seen)
        }
        _ => false,
    }
}

pub(super) struct BlockRange {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) label: String,
}

pub(super) struct ForRangeLoopParams {
    pub(super) block_idx: usize,
    pub(super) instr_idx: usize,
    pub(super) idx: u16,
    pub(super) limit: u16,
    pub(super) step: u16,
    pub(super) inclusive: bool,
    pub(super) ofs: i16,
}

fn operand_is_const_int(function: &Function, operand: u16) -> bool {
    rk_is_const(operand) && matches!(function.consts.get(rk_index(operand) as usize), Some(Val::Int(_)))
}

fn operand_is_known_integer(function: &Function, integers: &BTreeSet<u16>, operand: u16) -> bool {
    operand_is_const_int(function, operand) || (!rk_is_const(operand) && integers.contains(&operand))
}

fn mark_if_integer(function: &Function, integers: &mut BTreeSet<u16>, dst: u16, src: u16) -> bool {
    if operand_is_known_integer(function, integers, src) {
        integers.insert(dst)
    } else {
        false
    }
}

fn op_assigned_regs(op: &Op) -> Vec<u16> {
    match *op {
        Op::LoadK(dst, _)
        | Op::Move(dst, _)
        | Op::Not(dst, _)
        | Op::ToStr(dst, _)
        | Op::ToBool(dst, _)
        | Op::Add(dst, _, _)
        | Op::StrConcatKnownCap(dst, _, _)
        | Op::StrConcatToStr(dst, _, _)
        | Op::Sub(dst, _, _)
        | Op::Mul(dst, _, _)
        | Op::Div(dst, _, _)
        | Op::Mod(dst, _, _)
        | Op::AddInt(dst, _, _)
        | Op::AddFloat(dst, _, _)
        | Op::AddIntImm(dst, _, _)
        | Op::SubInt(dst, _, _)
        | Op::SubFloat(dst, _, _)
        | Op::MulInt(dst, _, _)
        | Op::MulFloat(dst, _, _)
        | Op::DivFloat(dst, _, _)
        | Op::FloorDivImm { dst, .. }
        | Op::ModInt(dst, _, _)
        | Op::ModFloat(dst, _, _)
        | Op::CmpEq(dst, _, _)
        | Op::CmpNe(dst, _, _)
        | Op::CmpLt(dst, _, _)
        | Op::CmpLe(dst, _, _)
        | Op::CmpGt(dst, _, _)
        | Op::CmpGe(dst, _, _)
        | Op::CmpI { dst, .. }
        | Op::CmpEqImm(dst, _, _)
        | Op::CmpNeImm(dst, _, _)
        | Op::CmpLtImm(dst, _, _)
        | Op::CmpLeImm(dst, _, _)
        | Op::CmpGtImm(dst, _, _)
        | Op::CmpGeImm(dst, _, _)
        | Op::In(dst, _, _)
        | Op::LoadLocal(dst, _)
        | Op::StoreLocal(dst, _)
        | Op::LoadGlobal(dst, _)
        | Op::LoadCapture { dst, .. }
        | Op::Access(dst, _, _)
        | Op::AccessK(dst, _, _)
        | Op::IndexK(dst, _, _)
        | Op::Len { dst, .. }
        | Op::ListLen { dst, .. }
        | Op::MapLen { dst, .. }
        | Op::StrLen { dst, .. }
        | Op::Floor { dst, .. }
        | Op::StartsWithK(dst, _, _)
        | Op::ContainsK(dst, _, _)
        | Op::MapHas(dst, _, _)
        | Op::MapHasK(dst, _, _)
        | Op::MapGetInterned(dst, _, _)
        | Op::MapGetDynamic(dst, _, _)
        | Op::BuildMap { dst, .. }
        | Op::BuildList { dst, .. }
        | Op::MakeClosure { dst, .. }
        | Op::CallMethod0 { dst, .. }
        | Op::CallGlobalMethod0 { dst, .. } => vec![dst],
        Op::NullishPick { dst, .. } | Op::JmpFalseSet { dst, .. } | Op::JmpTrueSet { dst, .. } => vec![dst],
        _ => Vec::new(),
    }
}

fn mark_required_sources(sources: &[BTreeSet<usize>], required: &mut BTreeSet<usize>, operand: u16) {
    if rk_is_const(operand) {
        return;
    }
    if let Some(source) = sources.get(operand as usize) {
        required.extend(source.iter().copied());
    }
}

fn copy_sources(sources: &mut [BTreeSet<usize>], dst: u16, src: u16) -> bool {
    if rk_is_const(src) {
        return false;
    }
    let src_sources = sources.get(src as usize).cloned().unwrap_or_default();
    let Some(dst_sources) = sources.get_mut(dst as usize) else {
        return false;
    };
    let before = dst_sources.len();
    dst_sources.extend(src_sources);
    dst_sources.len() != before
}

fn union_sources(sources: &mut [BTreeSet<usize>], dst: u16, a: u16, b: u16) -> bool {
    let mut changed = false;
    changed |= copy_sources(sources, dst, a);
    changed |= copy_sources(sources, dst, b);
    changed
}

#[derive(Clone)]
pub(super) enum KnownReg {
    Global(String),
    StringHandle {
        handle: String,
        text: String,
        len: usize,
    },
    ConstMap {
        entries: BTreeMap<String, Val>,
    },
    List {
        base: u16,
        len: u16,
    },
    IndexedValue {
        base: String,
        index: String,
    },
    AccessedValue {
        base: String,
        key: String,
    },
    AccessedConstStr {
        base_reg: u16,
        base: String,
        key: String,
    },
    AccessedStrInt {
        base_reg: u16,
        base: String,
        prefix: String,
        suffix: String,
    },
    AddMapGetConstStr {
        lhs: String,
        base_reg: u16,
        key: String,
    },
    AddMapGetStrInt {
        lhs: String,
        base_reg: u16,
        prefix: String,
        suffix: String,
    },
    StringIntKey {
        prefix: String,
        suffix: String,
    },
    StringLength {
        len: String,
        ascii: bool,
    },
    IndexedAsciiCharLength {
        base_len: String,
        index: String,
    },
    AotClosure {
        symbol: String,
        proto_index: u16,
        arity: usize,
        integer_params: BTreeSet<usize>,
    },
    Int,
}

#[derive(Clone)]
pub(super) struct NativeClosureBinding {
    pub(super) symbol: String,
    pub(super) proto_index: u16,
    pub(super) arity: usize,
    pub(super) integer_params: BTreeSet<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RuntimeHelper {
    InternString,
    ToString,
    LoadGlobal,
    DefineGlobal,
    BuildList,
    BuildMap,
    ListPush,
    ListPushInt,
    ListPushStrInt,
    MapSet,
    MapSetConstStr,
    MapSetStrInt,
    StringIntKey,
    MakeAotFunction,
    Call,
    CallNative,
    CallMethod,
    Access,
    AccessStrInt,
    MapGetConstStr,
    MapGetStrInt,
    MapHas,
    MapHasConstStr,
    MapHasStrInt,
    Index,
    IndexLen,
    In,
    Len,
    ListSlice,
    ToIter,
    MakeFloat,
    Floor,
    FloorDivImm,
    StartsWith,
    StartsWithConst,
    Contains,
    Compare,
    AddAccess,
    MulAccess,
    AddMapGetConstStr,
    AddMapGetStrInt,
    MulMapGetConstStr,
    MulMapGetStrInt,
    MapSetAddMapGetConstStr,
    MapSetAddMapGetStrInt,
    MapUpdateIntConstStr,
    MapUpdateIntStrInt,
    SubAccess,
    AddValue,
    SubValue,
    MulValue,
    DivValue,
    ModValue,
    IntDecimalLen,
}

impl RuntimeHelper {
    pub(super) const ALL: [RuntimeHelper; 54] = [
        RuntimeHelper::InternString,
        RuntimeHelper::ToString,
        RuntimeHelper::LoadGlobal,
        RuntimeHelper::DefineGlobal,
        RuntimeHelper::BuildList,
        RuntimeHelper::BuildMap,
        RuntimeHelper::ListPush,
        RuntimeHelper::ListPushInt,
        RuntimeHelper::ListPushStrInt,
        RuntimeHelper::MapSet,
        RuntimeHelper::MapSetConstStr,
        RuntimeHelper::MapSetStrInt,
        RuntimeHelper::StringIntKey,
        RuntimeHelper::MakeAotFunction,
        RuntimeHelper::Call,
        RuntimeHelper::CallNative,
        RuntimeHelper::CallMethod,
        RuntimeHelper::Access,
        RuntimeHelper::AccessStrInt,
        RuntimeHelper::MapGetConstStr,
        RuntimeHelper::MapGetStrInt,
        RuntimeHelper::MapHas,
        RuntimeHelper::MapHasConstStr,
        RuntimeHelper::MapHasStrInt,
        RuntimeHelper::Index,
        RuntimeHelper::IndexLen,
        RuntimeHelper::In,
        RuntimeHelper::Len,
        RuntimeHelper::ListSlice,
        RuntimeHelper::ToIter,
        RuntimeHelper::MakeFloat,
        RuntimeHelper::Floor,
        RuntimeHelper::FloorDivImm,
        RuntimeHelper::StartsWith,
        RuntimeHelper::StartsWithConst,
        RuntimeHelper::Contains,
        RuntimeHelper::Compare,
        RuntimeHelper::AddAccess,
        RuntimeHelper::MulAccess,
        RuntimeHelper::AddMapGetConstStr,
        RuntimeHelper::AddMapGetStrInt,
        RuntimeHelper::MulMapGetConstStr,
        RuntimeHelper::MulMapGetStrInt,
        RuntimeHelper::MapSetAddMapGetConstStr,
        RuntimeHelper::MapSetAddMapGetStrInt,
        RuntimeHelper::MapUpdateIntConstStr,
        RuntimeHelper::MapUpdateIntStrInt,
        RuntimeHelper::SubAccess,
        RuntimeHelper::AddValue,
        RuntimeHelper::SubValue,
        RuntimeHelper::MulValue,
        RuntimeHelper::DivValue,
        RuntimeHelper::ModValue,
        RuntimeHelper::IntDecimalLen,
    ];

    pub(super) fn symbol(self) -> &'static str {
        match self {
            RuntimeHelper::InternString => "lk_rt_intern_string",
            RuntimeHelper::ToString => "lk_rt_to_string",
            RuntimeHelper::LoadGlobal => "lk_rt_load_global",
            RuntimeHelper::DefineGlobal => "lk_rt_define_global",
            RuntimeHelper::BuildList => "lk_rt_build_list",
            RuntimeHelper::BuildMap => "lk_rt_build_map",
            RuntimeHelper::ListPush => "lk_rt_list_push",
            RuntimeHelper::ListPushInt => "lk_rt_list_push_int",
            RuntimeHelper::ListPushStrInt => "lk_rt_list_push_str_int",
            RuntimeHelper::MapSet => "lk_rt_map_set",
            RuntimeHelper::MapSetConstStr => "lk_rt_map_set_const_str",
            RuntimeHelper::MapSetStrInt => "lk_rt_map_set_str_int",
            RuntimeHelper::StringIntKey => "lk_rt_str_int_key",
            RuntimeHelper::MakeAotFunction => "lk_rt_make_aot_function",
            RuntimeHelper::Call => "lk_rt_call",
            RuntimeHelper::CallNative => "lk_rt_call_native",
            RuntimeHelper::CallMethod => "lk_rt_call_method",
            RuntimeHelper::Access => "lk_rt_access",
            RuntimeHelper::AccessStrInt => "lk_rt_access_str_int",
            RuntimeHelper::MapGetConstStr => "lk_rt_map_get_const_str",
            RuntimeHelper::MapGetStrInt => "lk_rt_map_get_str_int",
            RuntimeHelper::MapHas => "lk_rt_map_has",
            RuntimeHelper::MapHasConstStr => "lk_rt_map_has_const_str",
            RuntimeHelper::MapHasStrInt => "lk_rt_map_has_str_int",
            RuntimeHelper::Index => "lk_rt_index",
            RuntimeHelper::IndexLen => "lk_rt_index_len",
            RuntimeHelper::In => "lk_rt_in",
            RuntimeHelper::Len => "lk_rt_len",
            RuntimeHelper::ListSlice => "lk_rt_list_slice",
            RuntimeHelper::ToIter => "lk_rt_to_iter",
            RuntimeHelper::MakeFloat => "lk_rt_float",
            RuntimeHelper::Floor => "lk_rt_floor",
            RuntimeHelper::FloorDivImm => "lk_rt_floor_div_imm",
            RuntimeHelper::StartsWith => "lk_rt_starts_with",
            RuntimeHelper::StartsWithConst => "lk_rt_starts_with_const",
            RuntimeHelper::Contains => "lk_rt_contains",
            RuntimeHelper::Compare => "lk_rt_cmp",
            RuntimeHelper::AddAccess => "lk_rt_add_access",
            RuntimeHelper::MulAccess => "lk_rt_mul_access",
            RuntimeHelper::AddMapGetConstStr => "lk_rt_add_map_get_const_str",
            RuntimeHelper::AddMapGetStrInt => "lk_rt_add_map_get_str_int",
            RuntimeHelper::MulMapGetConstStr => "lk_rt_mul_map_get_const_str",
            RuntimeHelper::MulMapGetStrInt => "lk_rt_mul_map_get_str_int",
            RuntimeHelper::MapSetAddMapGetConstStr => "lk_rt_map_set_add_map_get_const_str",
            RuntimeHelper::MapSetAddMapGetStrInt => "lk_rt_map_set_add_map_get_str_int",
            RuntimeHelper::MapUpdateIntConstStr => "lk_rt_map_update_int_const_str",
            RuntimeHelper::MapUpdateIntStrInt => "lk_rt_map_update_int_str_int",
            RuntimeHelper::SubAccess => "lk_rt_sub_access",
            RuntimeHelper::AddValue => "lk_rt_add",
            RuntimeHelper::SubValue => "lk_rt_sub",
            RuntimeHelper::MulValue => "lk_rt_mul",
            RuntimeHelper::DivValue => "lk_rt_div",
            RuntimeHelper::ModValue => "lk_rt_mod",
            RuntimeHelper::IntDecimalLen => "lk_rt_int_decimal_len",
        }
    }

    pub(super) fn temp_prefix(self) -> &'static str {
        match self {
            RuntimeHelper::AddValue => "addval",
            RuntimeHelper::SubValue => "subval",
            RuntimeHelper::MulValue => "mulval",
            RuntimeHelper::DivValue => "divval",
            RuntimeHelper::ModValue => "modval",
            _ => "rtval",
        }
    }

    pub(super) fn declaration(self) -> &'static str {
        match self {
            RuntimeHelper::InternString => "declare i64 @lk_rt_intern_string(i8*, i64)",
            RuntimeHelper::ToString => "declare i64 @lk_rt_to_string(i64)",
            RuntimeHelper::LoadGlobal => "declare i64 @lk_rt_load_global(i64)",
            RuntimeHelper::DefineGlobal => "declare void @lk_rt_define_global(i64, i64)",
            RuntimeHelper::BuildList => "declare i64 @lk_rt_build_list(i64*, i64)",
            RuntimeHelper::ListPush => "declare i64 @lk_rt_list_push(i64, i64)",
            RuntimeHelper::ListPushInt => "declare i64 @lk_rt_list_push_int(i64, i64)",
            RuntimeHelper::ListPushStrInt => "declare i64 @lk_rt_list_push_str_int(i64, i8*, i64, i64)",
            RuntimeHelper::MapSet => "declare i64 @lk_rt_map_set(i64, i64, i64)",
            RuntimeHelper::MapSetConstStr => "declare i64 @lk_rt_map_set_const_str(i64, i8*, i64, i64)",
            RuntimeHelper::MapSetStrInt => "declare i64 @lk_rt_map_set_str_int(i64, i8*, i64, i64, i64)",
            RuntimeHelper::StringIntKey => "declare i64 @lk_rt_str_int_key(i8*, i64, i64)",
            RuntimeHelper::MakeAotFunction => "declare i64 @lk_rt_make_aot_function(i8*, i64)",
            RuntimeHelper::Call => "declare i64 @lk_rt_call(i64, i64*, i64, i64)",
            RuntimeHelper::CallNative => "declare i64 @lk_rt_call_native(i64, i64*, i64, i64)",
            RuntimeHelper::CallMethod => "declare i64 @lk_rt_call_method(i64, i64, i64*, i64, i64)",
            RuntimeHelper::BuildMap => "declare i64 @lk_rt_build_map(i64*, i64)",
            RuntimeHelper::Access => "declare i64 @lk_rt_access(i64, i64)",
            RuntimeHelper::AccessStrInt => "declare i64 @lk_rt_access_str_int(i64, i8*, i64, i64)",
            RuntimeHelper::MapGetConstStr => "declare i64 @lk_rt_map_get_const_str(i64, i8*, i64)",
            RuntimeHelper::MapGetStrInt => "declare i64 @lk_rt_map_get_str_int(i64, i8*, i64, i64)",
            RuntimeHelper::MapHas => "declare i64 @lk_rt_map_has(i64, i64)",
            RuntimeHelper::MapHasConstStr => "declare i64 @lk_rt_map_has_const_str(i64, i8*, i64)",
            RuntimeHelper::MapHasStrInt => "declare i64 @lk_rt_map_has_str_int(i64, i8*, i64, i64)",
            RuntimeHelper::Index => "declare i64 @lk_rt_index(i64, i64)",
            RuntimeHelper::IndexLen => "declare i64 @lk_rt_index_len(i64, i64)",
            RuntimeHelper::In => "declare i64 @lk_rt_in(i64, i64)",
            RuntimeHelper::Len => "declare i64 @lk_rt_len(i64)",
            RuntimeHelper::ListSlice => "declare i64 @lk_rt_list_slice(i64, i64)",
            RuntimeHelper::ToIter => "declare i64 @lk_rt_to_iter(i64)",
            RuntimeHelper::MakeFloat => "declare i64 @lk_rt_float(double)",
            RuntimeHelper::Floor => "declare i64 @lk_rt_floor(i64)",
            RuntimeHelper::FloorDivImm => "declare i64 @lk_rt_floor_div_imm(i64, i64)",
            RuntimeHelper::StartsWith => "declare i64 @lk_rt_starts_with(i64, i64)",
            RuntimeHelper::StartsWithConst => "declare i64 @lk_rt_starts_with_const(i64, i8*, i64)",
            RuntimeHelper::Contains => "declare i64 @lk_rt_contains(i64, i64)",
            RuntimeHelper::Compare => "declare i64 @lk_rt_cmp(i64, i64, i64)",
            RuntimeHelper::AddAccess => "declare i64 @lk_rt_add_access(i64, i64, i64)",
            RuntimeHelper::MulAccess => "declare i64 @lk_rt_mul_access(i64, i64, i64)",
            RuntimeHelper::AddMapGetConstStr => "declare i64 @lk_rt_add_map_get_const_str(i64, i64, i8*, i64)",
            RuntimeHelper::AddMapGetStrInt => "declare i64 @lk_rt_add_map_get_str_int(i64, i64, i8*, i64, i64)",
            RuntimeHelper::MulMapGetConstStr => "declare i64 @lk_rt_mul_map_get_const_str(i64, i64, i8*, i64)",
            RuntimeHelper::MulMapGetStrInt => "declare i64 @lk_rt_mul_map_get_str_int(i64, i64, i8*, i64, i64)",
            RuntimeHelper::MapSetAddMapGetConstStr => {
                "declare i64 @lk_rt_map_set_add_map_get_const_str(i64, i8*, i64, i64)"
            }
            RuntimeHelper::MapSetAddMapGetStrInt => {
                "declare i64 @lk_rt_map_set_add_map_get_str_int(i64, i8*, i64, i64, i64)"
            }
            RuntimeHelper::MapUpdateIntConstStr => {
                "declare i64 @lk_rt_map_update_int_const_str(i64, i8*, i64, i64, i64)"
            }
            RuntimeHelper::MapUpdateIntStrInt => {
                "declare i64 @lk_rt_map_update_int_str_int(i64, i8*, i64, i64, i64, i64)"
            }
            RuntimeHelper::SubAccess => "declare i64 @lk_rt_sub_access(i64, i64, i64)",
            RuntimeHelper::AddValue => "declare i64 @lk_rt_add(i64, i64)",
            RuntimeHelper::SubValue => "declare i64 @lk_rt_sub(i64, i64)",
            RuntimeHelper::MulValue => "declare i64 @lk_rt_mul(i64, i64)",
            RuntimeHelper::DivValue => "declare i64 @lk_rt_div(i64, i64)",
            RuntimeHelper::ModValue => "declare i64 @lk_rt_mod(i64, i64)",
            RuntimeHelper::IntDecimalLen => "declare i64 @lk_rt_int_decimal_len(i64)",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct StringConstant {
    pub(super) label: String,
    pub(super) encoded: String,
    pub(super) len: usize,
    pub(super) array_len: usize,
}

pub(super) struct IrWriter {
    buf: String,
    indent: usize,
}

impl IrWriter {
    pub(super) fn new() -> Self {
        Self {
            buf: String::new(),
            indent: 0,
        }
    }

    pub(super) fn indent(&mut self) {
        self.indent += 1;
    }

    pub(super) fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    pub(super) fn line<S: AsRef<str>>(&mut self, line: S) {
        let line = line.as_ref();
        if !line.is_empty() {
            for _ in 0..self.indent {
                self.buf.push_str("  ");
            }
        }
        self.buf.push_str(line);
        self.buf.push('\n');
    }

    pub(super) fn raw_line<S: AsRef<str>>(&mut self, line: S) {
        self.buf.push_str(line.as_ref());
        self.buf.push('\n');
    }

    pub(super) fn finish(self) -> String {
        self.buf
    }
}
