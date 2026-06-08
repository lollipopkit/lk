use super::{
    block_helpers::{
        concat_text_values, control_flow_static_boundaries, kind_symbolic_value, local_static_container_before,
        local_static_i64_before, mark_static_untaken_return_path, static_callable_value,
        static_register_value_trusted_before, static_string_i64_map_supported, text_value_from_native,
    },
    contains::{
        local_static_heap_const_before, local_static_i64_value_before, local_static_object_before,
        static_index_from_registers, static_int_list_index_value, static_int_range_from_registers,
        static_object_from_registers, static_slice_from_value,
    },
};
use crate::llvm::{
    callee_eval::native_direct_call_static_return_value,
    ir_text::native_relative_target,
    map_mutate::native_static_map_mutate,
    output::{emit_native_map_set, emit_native_static_core_call_method, emit_native_static_parse_builtin},
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
        NativeStringKeyKind, NativeTextPart, native_runtime_string_key_kind, native_static_compare_bool,
        native_static_global, native_static_i64_binary, native_static_index, native_static_list_from_values,
        native_static_list_join, native_static_load_cell, native_static_map_from_pairs, native_static_map_rest,
        native_static_set_index, native_static_store_cell, native_static_to_iter,
    },
};
use crate::vm::{ConstHeapValueData, ConstRuntimeValueData, FunctionData, Instr, Opcode};
mod analysis;
mod arg_lists;
mod entry;
mod list_push;
mod list_returns;
mod map_methods;
mod returns;
mod slots;
mod string_ops;

pub(in crate::llvm) use super::kind::{NativeScalarFacts, NativeScalarKind};
use analysis::*;
use arg_lists::*;
pub(in crate::llvm) use entry::native_scalar_block_facts_with_statics_and_functions;
use list_push::propagate_list_push;
use list_returns::dynamic_list_return_value;
use map_methods::*;
use returns::*;
use slots::*;
pub(in crate::llvm) fn native_scalar_block_facts_with_initial(
    register_count: usize,
    global_count: usize,
    global_names: &[String],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    mut kinds: Vec<Option<NativeScalarKind>>,
    mut static_values: Vec<Option<NativeStraightlineValue>>,
    mut global_kinds: Vec<Option<NativeScalarKind>>,
    mut static_globals: Vec<Option<NativeStraightlineValue>>,
    functions: Option<&[FunctionData]>,
    static_captures: &[NativeStraightlineValue],
    depth: usize,
    recursive_hints: &[(u16, Option<NativeScalarKind>)],
) -> Option<NativeScalarFacts> {
    if kinds.len() != register_count
        || static_values.len() != register_count
        || global_kinds.len() != global_count
        || static_globals.len() != global_count
    {
        return None;
    }
    let mut registers_before = Vec::with_capacity(code.len());
    let mut globals_before = Vec::with_capacity(code.len());
    let static_boundaries = control_flow_static_boundaries(code);
    let mut skip_static_pcs = vec![false; code.len()];
    for (pc, instr) in code.iter().copied().enumerate() {
        registers_before.push(kinds.clone());
        globals_before.push(global_kinds.clone());
        if skip_static_pcs.get(pc).copied().unwrap_or(false) {
            continue;
        }
        match instr.opcode() {
            Opcode::Nop | Opcode::Jmp => {}
            Opcode::LoadNil => {
                if !set_static_value(
                    &mut kinds,
                    &mut static_values,
                    instr.a(),
                    Some(NativeScalarKind::Nil),
                    NativeStraightlineValue::Nil,
                ) {
                    return None;
                }
            }
            Opcode::LoadInt => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                    return None;
                }
            }
            Opcode::LoadFloat => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::F64) {
                    return None;
                }
            }
            Opcode::LoadBool => {
                let value = i64::from(instr.b() != 0);
                if !set_static_value(
                    &mut kinds,
                    &mut static_values,
                    instr.a(),
                    Some(NativeScalarKind::Bool),
                    NativeStraightlineValue::Bool(value.to_string()),
                ) {
                    return None;
                }
            }
            Opcode::LoadString => {
                let Some(value) = strings.get(instr.bx() as usize) else {
                    return None;
                };
                if !set_static_value(
                    &mut kinds,
                    &mut static_values,
                    instr.a(),
                    Some(NativeScalarKind::StrPtr),
                    NativeStraightlineValue::String {
                        symbol: String::new(),
                        value: value.clone(),
                        len: value.chars().count(),
                        key_kind: native_runtime_string_key_kind(value),
                    },
                ) {
                    return None;
                }
            }
            Opcode::LoadHeapConst => {
                let Some(value) = heap_values.get(instr.bx() as usize) else {
                    return None;
                };
                if let Some(value) = dynamic_heap_container_value(value, pc) {
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                    continue;
                }
                let Some(value) = native_static_heap_const_value(value) else {
                    return None;
                };
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), value.0, value.1) {
                    return None;
                }
            }
            Opcode::LoadFunction => {
                if !set_static_value(
                    &mut kinds,
                    &mut static_values,
                    instr.a(),
                    None,
                    NativeStraightlineValue::Function(instr.bx()),
                ) {
                    return None;
                }
            }
            Opcode::LoadCapture => {
                let value = static_captures.get(instr.bx() as usize)?.clone();
                let kind = static_value_kind(&value);
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                    return None;
                }
            }
            Opcode::MakeClosure => {
                if let Some(value) = static_callable_value(functions?, instr, &static_values) {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        Some(NativeScalarKind::I64),
                        value,
                    ) {
                        return None;
                    }
                } else {
                    kinds[instr.a() as usize] = None;
                    static_values[instr.a() as usize] = None;
                }
            }
            Opcode::Move => {
                if let Some(value) = static_kind(&static_values, instr.b()) {
                    let kind = native_kind(&kinds, instr.b());
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                    continue;
                }
                let kind = native_kind(&kinds, instr.b()).unwrap_or(NativeScalarKind::I64);
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode::Move2 => {
                let Some(first_kind) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                let first_static = static_kind(&static_values, instr.b());
                if let Some(value) = first_static {
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), Some(first_kind), value) {
                        return None;
                    }
                } else if !set_native_kind(&mut kinds, &mut static_values, instr.a(), first_kind) {
                    return None;
                }

                let Some(second_kind) = native_kind(&kinds, instr.c()) else {
                    return None;
                };
                let second_static = static_kind(&static_values, instr.c());
                if let Some(value) = second_static {
                    if !set_static_value(&mut kinds, &mut static_values, instr.b(), Some(second_kind), value) {
                        return None;
                    }
                } else if !set_native_kind(&mut kinds, &mut static_values, instr.b(), second_kind) {
                    return None;
                }
            }
            Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat => {
                let Some(lhs) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                let Some(rhs) = native_kind(&kinds, instr.c()) else {
                    return None;
                };
                if !matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::F64)
                    || !matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::F64)
                {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::F64) {
                    return None;
                }
            }
            Opcode::AddMulInt | Opcode::Add2Int => {
                let acc = native_kind(&kinds, instr.a())
                    .or_else(|| static_kind(&static_values, instr.a()).and_then(|value| static_value_kind(&value)))
                    .unwrap_or(NativeScalarKind::I64);
                let lhs = native_kind(&kinds, instr.b())
                    .or_else(|| static_kind(&static_values, instr.b()).and_then(|value| static_value_kind(&value)))
                    .unwrap_or(NativeScalarKind::I64);
                let rhs = native_kind(&kinds, instr.c())
                    .or_else(|| static_kind(&static_values, instr.c()).and_then(|value| static_value_kind(&value)))
                    .unwrap_or(NativeScalarKind::I64);
                if !matches!(acc, NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
                    || !matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
                    || !matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
                {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                    return None;
                }
            }
            Opcode::AddInt
            | Opcode::SubInt
            | Opcode::MulInt
            | Opcode::DivInt
            | Opcode::ModInt
            | Opcode::MidInt
            | Opcode::MinInt
            | Opcode::MaxInt => {
                if let (Some(NativeStraightlineValue::I64(lhs)), Some(NativeStraightlineValue::I64(rhs))) = (
                    static_kind(&static_values, instr.b())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.b())),
                    static_kind(&static_values, instr.c())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c())),
                ) && let Some(value) = native_static_i64_binary(&lhs, &rhs, instr.opcode())
                {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        Some(NativeScalarKind::I64),
                        NativeStraightlineValue::I64(value),
                    ) {
                        return None;
                    }
                    continue;
                }
                if instr.opcode() == Opcode::AddInt {
                    let lhs_static = static_kind(&static_values, instr.b());
                    let rhs_static = static_kind(&static_values, instr.c());
                    let lhs_kind =
                        native_kind(&kinds, instr.b()).or_else(|| lhs_static.as_ref().and_then(static_value_kind));
                    let rhs_kind =
                        native_kind(&kinds, instr.c()).or_else(|| rhs_static.as_ref().and_then(static_value_kind));
                    if matches!(lhs_kind, Some(NativeScalarKind::StrPtr))
                        || matches!(rhs_kind, Some(NativeScalarKind::StrPtr))
                    {
                        let lhs =
                            lhs_static.or_else(|| lhs_kind.and_then(|kind| dynamic_text_part(kind, instr.b())))?;
                        let rhs =
                            rhs_static.or_else(|| rhs_kind.and_then(|kind| dynamic_text_part(kind, instr.c())))?;
                        let text = concat_text_values(lhs, rhs)?;
                        let kind = static_value_kind(&text);
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, text) {
                            return None;
                        }
                        continue;
                    }
                }
                let Some(lhs) = native_kind(&kinds, instr.b())
                    .or_else(|| static_kind(&static_values, instr.b()).and_then(|value| static_value_kind(&value)))
                else {
                    return None;
                };
                let rhs = native_kind(&kinds, instr.c())
                    .or_else(|| static_kind(&static_values, instr.c()).and_then(|value| static_value_kind(&value)))
                    .unwrap_or(NativeScalarKind::I64);
                let lhs_is_i64 = matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64);
                let rhs_is_i64 = matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64);
                if (!lhs.is_numeric() && lhs != NativeScalarKind::MaybeI64)
                    || (!rhs.is_numeric() && rhs != NativeScalarKind::MaybeI64)
                {
                    return None;
                }
                let out = if lhs_is_i64 && rhs_is_i64 {
                    NativeScalarKind::I64
                } else if lhs == NativeScalarKind::F64 || rhs == NativeScalarKind::F64 {
                    NativeScalarKind::F64
                } else {
                    NativeScalarKind::I64
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), out) {
                    return None;
                }
            }
            Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI => {
                let lhs = native_kind(&kinds, instr.b()).unwrap_or(NativeScalarKind::I64);
                if !matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64) {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                    return None;
                }
            }
            Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt => {
                if static_register_value_trusted_before(code, pc, instr.b())
                    && static_register_value_trusted_before(code, pc, instr.c())
                    && let (Some(lhs), Some(rhs)) = (
                        static_kind(&static_values, instr.b()),
                        static_kind(&static_values, instr.c()),
                    )
                    && let Some(value) = native_static_compare_bool(&lhs, &rhs, instr.opcode())
                {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        Some(NativeScalarKind::Bool),
                        NativeStraightlineValue::Bool(i64::from(value).to_string()),
                    ) {
                        return None;
                    }
                    continue;
                }
                let Some(lhs) = native_kind(&kinds, instr.b())
                    .or_else(|| static_kind(&static_values, instr.b()).and_then(|value| static_value_kind(&value)))
                else {
                    return None;
                };
                let rhs = native_kind(&kinds, instr.c())
                    .or_else(|| static_kind(&static_values, instr.c()).and_then(|value| static_value_kind(&value)))
                    .unwrap_or(NativeScalarKind::I64);
                let ordered_string_compare = matches!(
                    instr.opcode(),
                    Opcode::CmpLtInt | Opcode::CmpLeInt | Opcode::CmpGtInt | Opcode::CmpGeInt
                ) && lhs == NativeScalarKind::StrPtr
                    && rhs == NativeScalarKind::StrPtr;
                if !lhs.is_numeric()
                    && !ordered_string_compare
                    && !matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt)
                {
                    return None;
                }
                if matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt)
                    && lhs != rhs
                    && (!lhs.is_numeric() || !rhs.is_numeric())
                    && !matches!(
                        (lhs, rhs),
                        (
                            NativeScalarKind::MaybeI64,
                            NativeScalarKind::I64 | NativeScalarKind::Nil
                        ) | (
                            NativeScalarKind::I64 | NativeScalarKind::Nil,
                            NativeScalarKind::MaybeI64
                        ) | (NativeScalarKind::I64, NativeScalarKind::Nil)
                            | (NativeScalarKind::Nil, NativeScalarKind::I64)
                            | (NativeScalarKind::StrPtr, NativeScalarKind::Nil)
                            | (NativeScalarKind::Nil, NativeScalarKind::StrPtr)
                    )
                {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                if native_kind(&kinds, instr.a()).is_none() {
                    return None;
                }
                if let Some(NativeStraightlineValue::Bool(value)) = static_kind(&static_values, instr.a()) {
                    let value = value != "0";
                    let fallthrough = pc + 1;
                    let relative = match instr.opcode() {
                        Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code.len())?,
                        Opcode::BrFalse | Opcode::BrTrue => native_relative_target(pc, instr.sbx() as i32, code.len())?,
                        _ => return None,
                    };
                    let truthy_takes =
                        matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue;
                    let falsy_takes =
                        matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse;
                    let truthy_target = if truthy_takes { relative } else { fallthrough };
                    let falsy_target = if falsy_takes { relative } else { fallthrough };
                    let untaken = if value { falsy_target } else { truthy_target };
                    mark_static_untaken_return_path(&mut skip_static_pcs, &static_boundaries, code, untaken);
                }
            }
            Opcode::BrNil | Opcode::BrNotNil => {
                if native_kind(&kinds, instr.a()).is_none() {
                    return None;
                }
                if let Some(value) = static_kind(&static_values, instr.a()) {
                    let is_nil = matches!(value, NativeStraightlineValue::Nil);
                    let branch_taken =
                        (instr.opcode() == Opcode::BrNil && is_nil) || (instr.opcode() == Opcode::BrNotNil && !is_nil);
                    let fallthrough = pc + 1;
                    let relative = native_relative_target(pc, instr.sbx() as i32, code.len())?;
                    let untaken = if branch_taken { fallthrough } else { relative };
                    mark_static_untaken_return_path(&mut skip_static_pcs, &static_boundaries, code, untaken);
                }
            }
            Opcode::BrEqZeroInt
            | Opcode::BrNeZeroInt
            | Opcode::BrEqIntI4
            | Opcode::BrNeIntI4
            | Opcode::BrModEqZeroIntI4
            | Opcode::BrModNeZeroIntI4 => {
                if !matches!(
                    native_kind(&kinds, instr.a()),
                    Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
                ) {
                    return None;
                }
            }
            opcode if opcode.is_compare_test() => {
                if native_kind(&kinds, instr.a()).is_none()
                    || (!opcode.is_int_immediate_compare_test() && native_kind(&kinds, instr.b()).is_none())
                {
                    return None;
                }
                let jmp = code.get(pc + 1).copied()?;
                if jmp.opcode() != Opcode::Jmp {
                    return None;
                }
                let rhs = if opcode.is_int_immediate_compare_test() {
                    Some(NativeStraightlineValue::I64(i64::from(instr.sc()).to_string()))
                } else {
                    static_kind(&static_values, instr.b())
                };
                if let (Some(lhs), Some(rhs)) = (static_kind(&static_values, instr.a()), rhs)
                    && let Some(value) = static_compare_test_value(instr.opcode(), &lhs, &rhs)
                {
                    let jump_when = if opcode.is_int_immediate_compare_test() {
                        instr.b() != 0
                    } else {
                        instr.c() != 0
                    };
                    let branch_taken = value == jump_when;
                    let fallthrough = pc + 2;
                    let relative = native_relative_target(pc + 1, jmp.sj_arg(), code.len())?;
                    let untaken = if branch_taken { fallthrough } else { relative };
                    mark_static_untaken_return_path(&mut skip_static_pcs, &static_boundaries, code, untaken);
                }
            }
            Opcode::ForLoopI => {
                if !matches!(native_kind(&kinds, instr.a()), Some(NativeScalarKind::I64))
                    || !matches!(native_kind(&kinds, instr.b()), Some(NativeScalarKind::I64))
                    || !matches!(native_kind(&kinds, instr.c()), Some(NativeScalarKind::I64))
                {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                    return None;
                }
            }
            Opcode::Not => {
                let Some(kind) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                if !matches!(
                    kind,
                    NativeScalarKind::Bool
                        | NativeScalarKind::Nil
                        | NativeScalarKind::I64
                        | NativeScalarKind::F64
                        | NativeScalarKind::StrPtr
                        | NativeScalarKind::MaybeI64
                ) {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode::IsNil | Opcode::IsList | Opcode::IsMap => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode::ToString => {
                let value = static_kind(&static_values, instr.b())
                    .or_else(|| native_kind(&kinds, instr.b()).and_then(|kind| dynamic_text_part(kind, instr.b())))
                    .unwrap_or_else(|| dynamic_text_part(NativeScalarKind::I64, instr.b()).unwrap());
                let Some(text) = text_value_from_native(value) else {
                    return None;
                };
                let kind = static_value_kind(&text);
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, text) {
                    return None;
                }
            }
            Opcode::ConcatString => {
                let lhs = static_kind(&static_values, instr.b())
                    .or_else(|| native_kind(&kinds, instr.b()).and_then(|kind| dynamic_text_part(kind, instr.b())))
                    .unwrap_or_else(|| dynamic_text_part(NativeScalarKind::I64, instr.b()).unwrap());
                let rhs = static_kind(&static_values, instr.c())
                    .or_else(|| native_kind(&kinds, instr.c()).and_then(|kind| dynamic_text_part(kind, instr.c())))
                    .unwrap_or_else(|| dynamic_text_part(NativeScalarKind::I64, instr.c()).unwrap());
                let Some(text) = concat_text_values(lhs, rhs) else {
                    return None;
                };
                let kind = static_value_kind(&text);
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, text) {
                    return None;
                }
            }
            Opcode::ConcatN => {
                // ConcatN: concatenate multiple register operands into a string.
                // Static folding of N-ary concat is complex; mark result as StrPtr only.
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::StrPtr) {
                    return None;
                }
            }
            Opcode::Len => {
                if let Some(target) = static_kind(&static_values, instr.b()) {
                    if matches!(target, NativeStraightlineValue::DynamicMapIter { .. })
                        || native_dynamic_text_len_supported(&target)
                    {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                            return None;
                        }
                    } else {
                        set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64);
                    }
                } else {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                        return None;
                    }
                }
            }
            Opcode::StringSplit => {
                if string_ops::propagate_string_split(&mut kinds, &mut static_values, code, heap_values, pc, instr)
                    .is_none()
                {
                    return None;
                }
            }
            Opcode::ListJoin => {
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                let Some(delimiter) = static_kind(&static_values, instr.c()) else {
                    return None;
                };
                if let (
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: element @ (NativeListElementKind::Text | NativeListElementKind::StrPtr),
                    },
                    NativeStraightlineValue::String { value: delimiter, .. },
                ) = (&target, &delimiter)
                {
                    if !delimiter.is_ascii()
                        || !set_static_value(
                            &mut kinds,
                            &mut static_values,
                            instr.a(),
                            None,
                            NativeStraightlineValue::DynamicJoinedText {
                                id: *id,
                                delimiter_len: delimiter.len(),
                                element: *element,
                            },
                        )
                    {
                        return None;
                    }
                    continue;
                }
                if let Some(value) = native_static_list_join(target.clone(), delimiter.clone(), String::new()) {
                    let kind = static_value_kind(&value);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                    continue;
                }
                let (
                    NativeStraightlineValue::DynamicSplitText {
                        text,
                        delimiter: split_delimiter,
                    },
                    NativeStraightlineValue::String {
                        value: join_delimiter, ..
                    },
                ) = (target, delimiter)
                else {
                    return None;
                };
                if split_delimiter != join_delimiter
                    || !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        Some(NativeScalarKind::StrPtr),
                        if let [NativeTextPart::StrPtr(ptr)] = text.as_slice() {
                            NativeStraightlineValue::StringPtr(ptr.clone())
                        } else {
                            NativeStraightlineValue::Text(text)
                        },
                    )
                {
                    return None;
                }
            }
            Opcode::StringStartsWith => {
                let Some(prefix) = static_kind(&static_values, instr.c()) else {
                    return None;
                };
                let NativeStraightlineValue::String { value: prefix, .. } = prefix else {
                    return None;
                };
                if let Some(NativeStraightlineValue::String { value: target, .. }) =
                    static_kind(&static_values, instr.b())
                {
                    let value = i64::from(target.starts_with(&prefix)).to_string();
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        Some(NativeScalarKind::Bool),
                        NativeStraightlineValue::Bool(value),
                    ) {
                        return None;
                    }
                } else if native_kind(&kinds, instr.b()) == Some(NativeScalarKind::StrPtr) {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Opcode::GetGlobal => {
                if let Some(value) = global_names
                    .get(instr.bx() as usize)
                    .and_then(|name| native_static_global(name))
                {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        static_value_kind(&value),
                        value,
                    ) {
                        return None;
                    }
                    continue;
                }
                if let Some(value) = static_global(&static_globals, instr.bx()) {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        static_value_kind(&value),
                        value,
                    ) {
                        return None;
                    }
                    continue;
                }
                let Some(kind) = native_global_kind(&global_kinds, instr.bx()) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode::SetGlobal => {
                if let Some(value) = static_kind(&static_values, instr.a()) {
                    if !set_static_global(&mut static_globals, &mut global_kinds, instr.bx(), value) {
                        return None;
                    }
                } else {
                    let Some(kind) = native_kind(&kinds, instr.a()) else {
                        return None;
                    };
                    if !set_native_global_kind(&mut global_kinds, &mut static_globals, instr.bx(), kind) {
                        return None;
                    }
                }
            }
            Opcode::GetIndex | Opcode::GetList => {
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    let target_kind = native_kind(&kinds, instr.b())?;
                    if target_kind == NativeScalarKind::I64 || target_kind == NativeScalarKind::MaybeI64 {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                            return None;
                        }
                    } else if target_kind == NativeScalarKind::StrPtr {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::StrPtr) {
                            return None;
                        }
                    } else if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                        return None;
                    }
                    continue;
                };
                if let Some(value) = static_index_from_registers(
                    &static_values,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr,
                    target.clone(),
                ) {
                    let kind = static_value_kind(&value);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                    continue;
                }
                if matches!(target, NativeStraightlineValue::Text(_)) {
                    if native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                        || !set_static_value(
                            &mut kinds,
                            &mut static_values,
                            instr.a(),
                            None,
                            NativeStraightlineValue::DynamicTextChar,
                        )
                    {
                        return None;
                    }
                } else if matches!(
                    target,
                    NativeStraightlineValue::DynamicMapIter { .. } | NativeStraightlineValue::DynamicMapEntry { .. }
                ) {
                    let index_kind = native_kind(&kinds, instr.c());
                    let field = static_kind(&static_values, instr.c())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()));
                    let Some(ok) = propagate_dynamic_map_iter_get_index(
                        &mut kinds,
                        &mut static_values,
                        instr,
                        target.clone(),
                        index_kind,
                        field,
                    ) else {
                        return None;
                    };
                    if !ok {
                        return None;
                    }
                } else if matches!(
                    target,
                    NativeStraightlineValue::DynamicList {
                        element: NativeListElementKind::I64,
                        ..
                    }
                ) {
                    let key = static_kind(&static_values, instr.c())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()));
                    if let Some(key) = key
                        && let Some(value) =
                            static_int_list_index_value(code, int_consts, strings, heap_values, &target, &key)
                    {
                        if !set_static_value(
                            &mut kinds,
                            &mut static_values,
                            instr.a(),
                            Some(NativeScalarKind::I64),
                            value,
                        ) {
                            return None;
                        }
                    } else if native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                        || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64)
                    {
                        return None;
                    }
                } else if matches!(
                    target,
                    NativeStraightlineValue::DynamicList {
                        element: NativeListElementKind::F64,
                        ..
                    }
                ) {
                    if native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                        || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::F64)
                    {
                        return None;
                    }
                } else if let Some(ok) = {
                    let key = static_kind(&static_values, instr.c())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()));
                    propagate_dynamic_string_map_get_index(&mut kinds, &mut static_values, instr, &target, key)
                } {
                    if !ok {
                        return None;
                    }
                } else if let Some(ok) = {
                    let index_kind = native_kind(&kinds, instr.c());
                    propagate_dynamic_i64_map_get_index(&mut kinds, &mut static_values, instr, &target, index_kind)
                } {
                    if !ok {
                        return None;
                    }
                } else if let Some(ok) = {
                    let index_kind = native_kind(&kinds, instr.c());
                    propagate_dynamic_string_list_get_index(&mut kinds, &mut static_values, instr, &target, index_kind)
                } {
                    if !ok {
                        return None;
                    }
                } else if let NativeStraightlineValue::Map { entries, .. } = &target
                    && native_kind(&kinds, instr.c()) == Some(NativeScalarKind::StrPtr)
                    && static_string_i64_map_supported(entries)
                {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::MaybeI64) {
                        return None;
                    }
                } else if let NativeStraightlineValue::List { elements, .. } = &target
                    && native_kind(&kinds, instr.c()) == Some(NativeScalarKind::I64)
                {
                    if let Some(key) = static_kind(&static_values, instr.c())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
                        && let Some(value) = native_static_index(target.clone(), key, String::new())
                    {
                        let kind = static_value_kind(&value);
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                            return None;
                        }
                    } else {
                        let kind = if elements.iter().all(|value| {
                            matches!(
                                value,
                                ConstRuntimeValueData::ShortStr(_) | ConstRuntimeValueData::Heap(_)
                            )
                        }) {
                            NativeScalarKind::StrPtr
                        } else {
                            NativeScalarKind::I64
                        };
                        if kind == NativeScalarKind::StrPtr {
                            if !set_static_value(
                                &mut kinds,
                                &mut static_values,
                                instr.a(),
                                Some(NativeScalarKind::StrPtr),
                                NativeStraightlineValue::StringPtr(format!("%r{}.slot", instr.a())),
                            ) {
                                return None;
                            }
                        } else if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                            return None;
                        }
                    }
                } else if let Some(value) =
                    arg_list_get_index_value(&static_values, &kinds, code, int_consts, pc, instr, target.clone())
                {
                    let kind = static_value_kind(&value);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                } else {
                    let Some(key) = static_kind(&static_values, instr.c())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
                    else {
                        return None;
                    };
                    if let Some(value) = native_static_index(target.clone(), key, String::new()) {
                        let kind = static_value_kind(&value);
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                            return None;
                        }
                    } else if matches!(target, NativeStraightlineValue::I64(_)) {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                            return None;
                        }
                    } else if matches!(target, NativeStraightlineValue::StringPtr(_)) {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::StrPtr) {
                            return None;
                        }
                    } else {
                        return None;
                    }
                }
            }
            Opcode::GetFieldK => {
                let Some(key) = field_key_value(strings, instr) else {
                    return None;
                };
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    let target_kind = native_kind(&kinds, instr.b())?;
                    let result_kind = match target_kind {
                        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => NativeScalarKind::MaybeStrPtr,
                        _ => NativeScalarKind::I64,
                    };
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), result_kind) {
                        return None;
                    }
                    continue;
                };
                if let Some(value) = native_static_index(target.clone(), key.clone(), String::new()) {
                    let kind = static_value_kind(&value);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                } else if let Some(ok) =
                    propagate_dynamic_string_map_get_index(&mut kinds, &mut static_values, instr, &target, Some(key))
                {
                    if !ok {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Opcode::SetIndex => {
                let Some(target) = static_kind(&static_values, instr.a()) else {
                    return None;
                };
                if matches!(
                    target,
                    NativeStraightlineValue::DynamicMap {
                        key: NativeMapKeyKind::Str,
                        value: NativeMapValueKind::I64,
                        ..
                    }
                ) {
                    let key_supported = instr.a() == instr.b()
                        || static_kind(&static_values, instr.b())
                            .as_ref()
                            .is_some_and(native_string_int_map_key_supported);
                    if !key_supported || native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64) {
                        return None;
                    }
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, target) {
                        return None;
                    }
                } else if matches!(
                    target,
                    NativeStraightlineValue::DynamicList {
                        element: NativeListElementKind::I64,
                        ..
                    }
                ) {
                    if native_kind(&kinds, instr.b()) != Some(NativeScalarKind::I64)
                        || native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                    {
                        return None;
                    }
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, target) {
                        return None;
                    }
                } else if let Some(value) = arg_list_set_index_value(
                    target.clone(),
                    &static_values,
                    int_consts,
                    code,
                    pc,
                    instr.b(),
                    instr.c(),
                ) {
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                } else {
                    let Some(key) = static_kind(&static_values, instr.b()) else {
                        return None;
                    };
                    let Some(value) = static_kind(&static_values, instr.c()) else {
                        return None;
                    };
                    let Some(value) = native_static_set_index(target, key, value) else {
                        return None;
                    };
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                }
            }
            Opcode::SetFieldK => {
                let Some(target) = static_kind(&static_values, instr.a()) else {
                    return None;
                };
                let Some(key) = field_key_value(strings, instr) else {
                    return None;
                };
                if matches!(
                    target,
                    NativeStraightlineValue::DynamicMap {
                        key: NativeMapKeyKind::Str,
                        value: NativeMapValueKind::I64,
                        ..
                    }
                ) {
                    if native_kind(&kinds, instr.b()) != Some(NativeScalarKind::I64) {
                        return None;
                    }
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, target) {
                        return None;
                    }
                } else {
                    let Some(value) = static_kind(&static_values, instr.b()) else {
                        return None;
                    };
                    let Some(value) = native_static_set_index(target, key, value) else {
                        return None;
                    };
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                }
            }
            Opcode::ListPush => {
                if propagate_list_push(&mut kinds, &mut static_values, instr).is_none() {
                    return None;
                }
            }
            Opcode::NewList => {
                let start = instr.b() as usize;
                let end = start.checked_add(instr.c() as usize)?;
                if end > kinds.len() {
                    return None;
                }
                if instr.c() == 1
                    && let Some(value) = single_callable_arg_list(static_values.get(start).cloned().flatten())
                {
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                    continue;
                }
                if let Some(value) = object_arg_list_from_registers(&static_values, code, int_consts, pc, start, end) {
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                    continue;
                }
                if static_values.get(start..end).is_some_and(|values| {
                    values.iter().any(|value| {
                        matches!(
                            value,
                            Some(
                                NativeStraightlineValue::List { .. }
                                    | NativeStraightlineValue::DynamicList { .. }
                                    | NativeStraightlineValue::DynamicMap { .. }
                            )
                        )
                    })
                }) {
                    let Some(values) = static_values.get(start..end) else {
                        return None;
                    };
                    let allow_i64_placeholder = values
                        .iter()
                        .any(|value| matches!(value, Some(NativeStraightlineValue::DynamicList { .. })));
                    let elements = values
                        .iter()
                        .enumerate()
                        .map(|(offset, value)| {
                            let reg = u8::try_from(start + offset).ok()?;
                            value
                                .clone()
                                .or_else(|| {
                                    local_static_i64_value_before(code, int_consts, strings, heap_values, pc, reg)
                                })
                                .or_else(|| local_static_i64_before(code, int_consts, pc, reg))
                                .or_else(|| local_static_heap_const_before(code, heap_values, pc, reg))
                                .or_else(|| dynamic_scalar_placeholder(&kinds, reg, allow_i64_placeholder))
                        })
                        .collect::<Option<Vec<_>>>()?;
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        None,
                        NativeStraightlineValue::ArgList { elements },
                    ) {
                        return None;
                    }
                    continue;
                }
                let all_i64 = (start..end).all(|i| match static_values.get(i).and_then(|v| v.as_ref()) {
                    Some(NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }) => false,
                    Some(NativeStraightlineValue::I64(s)) if !s.starts_with('%') => true,
                    _ => kinds.get(i).copied().flatten() == Some(NativeScalarKind::I64),
                });
                if all_i64 {
                    let value = NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::I64,
                    };
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                } else {
                    let Some(elems) = static_values.get(start..end) else {
                        return None;
                    };
                    let values = elems
                        .iter()
                        .enumerate()
                        .map(|(offset, value)| {
                            let reg = u8::try_from(start + offset).ok()?;
                            match value {
                                Some(NativeStraightlineValue::DynamicList {
                                    element: NativeListElementKind::I64,
                                    ..
                                })
                                | None => {
                                    local_static_i64_value_before(code, int_consts, strings, heap_values, pc, reg)
                                        .or_else(|| local_static_i64_before(code, int_consts, pc, reg))
                                        .or_else(|| local_static_heap_const_before(code, heap_values, pc, reg))
                                        .or_else(|| dynamic_scalar_placeholder(&kinds, reg, true))
                                        .or_else(|| value.clone())
                                }
                                _ => value.clone(),
                            }
                        })
                        .collect::<Option<Vec<_>>>()?;
                    let value = native_static_list_from_values(&values, String::new())
                        .unwrap_or(NativeStraightlineValue::ArgList { elements: values });
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                }
            }
            Opcode::Call => {
                if instr.a() != instr.b() {
                    return None;
                }
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    kinds[instr.a() as usize] = None;
                    static_values[instr.a() as usize] = None;
                    continue;
                };
                if let Some((function_index, captures)) = static_call_target(&target) {
                    let function_index = u8::try_from(function_index).ok()?;
                    let direct_instr = Instr::abc(Opcode::CallDirect, instr.a(), function_index, instr.c());
                    let kind = native_direct_call_return_kind(
                        functions?,
                        direct_instr,
                        &kinds,
                        &static_values,
                        &global_kinds,
                        &static_globals,
                        global_count,
                        global_names,
                        &captures,
                        depth,
                        recursive_hints,
                    )?;
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                        return None;
                    }
                    continue;
                }
                let start = instr.b() as usize + 1;
                let end = start.checked_add(instr.c() as usize)?;
                if let Some(ok) = propagate_dynamic_map_call(&mut kinds, &mut static_values, instr, pc, &target, start)
                {
                    if !ok {
                        return None;
                    }
                    continue;
                }
                if let Some(ok) =
                    propagate_dynamic_ptr_list_builtin_call(&mut kinds, &mut static_values, instr, pc, &target, start)
                {
                    if !ok {
                        return None;
                    }
                    continue;
                }
                if let Some(ok) = propagate_dynamic_i64_list_builtin_call(
                    &mut kinds,
                    &mut static_values,
                    code,
                    heap_values,
                    instr,
                    pc,
                    &target,
                    start,
                ) {
                    if !ok {
                        return None;
                    }
                    continue;
                }
                if let Some(ok) =
                    propagate_dynamic_f64_list_builtin_call(&mut kinds, &mut static_values, instr, pc, &target, start)
                {
                    if !ok {
                        return None;
                    }
                    continue;
                }
                let Some(args) = static_values.get(start..end) else {
                    if let Some(kind) = native_builtin_return_kind_dynamic(&target, instr.c()) {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                            return None;
                        }
                    } else {
                        return None;
                    }
                    continue;
                };
                let args_vec: Vec<_> = args.iter().cloned().collect();
                if args_vec.iter().any(|a| a.is_none()) {
                    let recovered = (start..end)
                        .map(|reg| {
                            let reg = u8::try_from(reg).ok()?;
                            static_values
                                .get(reg as usize)
                                .cloned()
                                .flatten()
                                .or_else(|| local_static_heap_const_before(code, heap_values, pc, reg))
                                .or_else(|| local_static_object_before(&static_values, code, int_consts, pc, reg))
                                .or_else(|| {
                                    local_static_i64_value_before(code, int_consts, strings, heap_values, pc, reg)
                                })
                                .or_else(|| local_static_i64_before(code, int_consts, pc, reg))
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some(args_vec) = recovered
                        && let Some(kind) = native_builtin_return_kind(target.clone(), &args_vec)
                    {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                            return None;
                        }
                        continue;
                    }
                    if let Some(kind) = native_builtin_return_kind_dynamic(&target, instr.c()) {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                            return None;
                        }
                    } else {
                        return None;
                    }
                    continue;
                }
                let args_vec: Vec<NativeStraightlineValue> = args_vec.into_iter().map(|a| a.unwrap()).collect();
                if matches!(target, NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod)) {
                    let mut tmp_index = 0usize;
                    if let Some(value) = emit_native_static_core_call_method(&args_vec, &mut tmp_index) {
                        let kind = static_value_kind(&value);
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                            return None;
                        }
                        continue;
                    }
                    if let Some(ok) =
                        propagate_dynamic_string_list_method_call(&mut kinds, &mut static_values, instr, &args_vec)
                            .or_else(|| {
                                propagate_dynamic_i64_list_method_call(&mut kinds, &mut static_values, instr, &args_vec)
                                    .or_else(|| {
                                        propagate_dynamic_f64_list_method_call(
                                            &mut kinds,
                                            &mut static_values,
                                            instr,
                                            &args_vec,
                                        )
                                    })
                            })
                    {
                        if !ok {
                            return None;
                        }
                        continue;
                    }
                    if let Some(kind) = dynamic_map_get_method_kind(&args_vec) {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                            return None;
                        }
                        continue;
                    }
                }
                if let Some(value) = match target.clone() {
                    NativeStraightlineValue::Builtin(NativeBuiltin::MapSet) => emit_native_map_set(&args_vec),
                    NativeStraightlineValue::Builtin(NativeBuiltin::MapMutate) => {
                        let [target, callable] = args_vec.as_slice() else {
                            return None;
                        };
                        native_static_map_mutate(
                            functions?,
                            target.clone(),
                            callable.clone(),
                            format!("@lk_map_mutate_{pc}"),
                        )
                    }
                    NativeStraightlineValue::Builtin(builtin) => emit_native_static_parse_builtin(builtin, &args_vec),
                    _ => None,
                } {
                    let kind = static_value_kind(&value);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                    continue;
                }
                if matches!(target, NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod))
                    && let [
                        list,
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::ArgList { elements },
                    ] = args_vec.as_slice()
                    && method == "map"
                    && matches!(
                        elements.as_slice(),
                        [NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }]
                    )
                {
                    let kind = static_value_kind(list);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, list.clone()) {
                        return None;
                    }
                    continue;
                }
                let Some(kind) = native_builtin_return_kind(target, &args_vec) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode::CallDirect => {
                let callee_index = instr.b();
                if let Some((_, hint)) = recursive_hints.iter().find(|(idx, _)| *idx as u8 == callee_index) {
                    if let Some(kind) = hint {
                        let value = kind_symbolic_value(*kind, instr.a());
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), Some(*kind), value) {
                            return None;
                        }
                    } else {
                        kinds[instr.a() as usize] = None;
                        static_values[instr.a() as usize] = None;
                    }
                } else {
                    if let Some(value) = native_direct_call_static_return_value(
                        functions?,
                        instr,
                        &static_values,
                        code,
                        int_consts,
                        pc,
                        &[],
                        depth,
                    ) {
                        let kind = static_value_kind(&value);
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                            return None;
                        }
                        continue;
                    }
                    if let Some(callee) = functions?.get(callee_index as usize) {
                        let start = instr.a().checked_add(1)? as usize;
                        let end = start.checked_add(instr.c() as usize)?;
                        let args = static_values.get(start..end)?;
                        if let Some(value) = dynamic_list_return_value(callee, args, pc) {
                            if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                                return None;
                            }
                            continue;
                        }
                    }
                    let Some(kind) = native_direct_call_return_kind(
                        functions?,
                        instr,
                        &kinds,
                        &static_values,
                        &global_kinds,
                        &static_globals,
                        global_count,
                        global_names,
                        &[],
                        depth,
                        recursive_hints,
                    ) else {
                        return None;
                    };
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                        return None;
                    }
                }
            }
            Opcode::CallNamed => {
                let Some(target) = static_kind(&static_values, instr.a()) else {
                    kinds[instr.a() as usize] = None;
                    static_values[instr.a() as usize] = None;
                    continue;
                };
                let Some((function_index, captures)) = static_call_target(&target) else {
                    kinds[instr.a() as usize] = None;
                    static_values[instr.a() as usize] = None;
                    continue;
                };
                let function = functions?.get(function_index as usize)?;
                let args = native_named_call_args(
                    function,
                    &kinds,
                    &static_values,
                    instr.a(),
                    instr.bx() & 0x7f,
                    instr.bx() >> 7,
                )?;
                if let Some((_, hint)) = recursive_hints.iter().find(|(idx, _)| *idx == function_index) {
                    if let Some(kind) = hint {
                        let value = kind_symbolic_value(*kind, instr.a());
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), Some(*kind), value) {
                            return None;
                        }
                    } else {
                        kinds[instr.a() as usize] = None;
                        static_values[instr.a() as usize] = None;
                    }
                } else {
                    let Some(kind) = native_static_function_return_kind(
                        functions?,
                        function_index as usize,
                        &args,
                        &captures,
                        &global_kinds,
                        &static_globals,
                        global_count,
                        global_names,
                        depth,
                        recursive_hints,
                    ) else {
                        return None;
                    };
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                        return None;
                    }
                }
            }
            opcode if opcode.is_return() => {
                if instr.return_count() > 1 {
                    return None;
                }
                if instr.return_count() == 1
                    && native_kind(&kinds, instr.a())
                        .or_else(|| static_kind(&static_values, instr.a()).and_then(|value| static_value_kind(&value)))
                        .is_none()
                {
                    return None;
                }
            }
            Opcode::NewMap => {
                let start = instr.b() as usize;
                let Some(width) = (instr.c() as usize).checked_mul(2) else {
                    return None;
                };
                if width == 0 {
                    let value = NativeStraightlineValue::DynamicMap {
                        id: pc,
                        key: NativeMapKeyKind::Str,
                        value: NativeMapValueKind::I64,
                    };
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                    continue;
                }
                let Some(end) = start.checked_add(width) else {
                    return None;
                };
                let Some(values) = static_values.get(start..end) else {
                    return None;
                };
                let mut pairs = Vec::with_capacity(instr.c() as usize);
                for pair in values.chunks_exact(2) {
                    let Some(key) = pair[0].clone() else {
                        return None;
                    };
                    let Some(value) = pair[1].clone() else {
                        return None;
                    };
                    pairs.push((key, value));
                }
                let Some(value) = native_static_map_from_pairs(&pairs, String::new()) else {
                    return None;
                };
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                    return None;
                }
            }
            Opcode::NewObject => {
                let value = static_object_from_registers(&static_values, code, int_consts, pc, instr, String::new())?;
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                    return None;
                }
            }
            Opcode::Contains => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode::SliceFrom => {
                let target = static_kind(&static_values, instr.b())
                    .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()));
                let Some(start) = static_kind(&static_values, instr.c())
                    .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
                else {
                    return None;
                };
                let Some(target) = target else {
                    if native_kind(&kinds, instr.b()) == Some(NativeScalarKind::I64) {
                        let value = NativeStraightlineValue::DynamicList {
                            id: pc,
                            element: NativeListElementKind::I64,
                        };
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                            return None;
                        }
                        continue;
                    }
                    return None;
                };
                let value = static_slice_from_value(code, heap_values, target, start, String::new());
                let Some(value) = value else {
                    return None;
                };
                let kind = static_value_kind(&value);
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                    return None;
                }
            }
            Opcode::MapRest => {
                let start = instr.b() as usize;
                let end = start.checked_add(1usize.checked_add(instr.c() as usize)?)?;
                let values = static_values.get(start..end)?;
                let target = values.first()?.clone()?;
                let keys = values[1..].iter().cloned().collect::<Option<Vec<_>>>()?;
                let value = native_static_map_rest(target, &keys, String::new())?;
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                    return None;
                }
            }
            Opcode::ToIter => {
                if let Some(target) = static_kind(&static_values, instr.b())
                    .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
                {
                    if let Some(value) = dynamic_map_to_iter_value(&target) {
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                            return None;
                        }
                    } else if let NativeStraightlineValue::DynamicConstListElement { .. } = target {
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, target) {
                            return None;
                        }
                    } else if let Some(value) = native_static_to_iter(target.clone(), String::new()) {
                        if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                            return None;
                        }
                    } else if matches!(target, NativeStraightlineValue::I64(_))
                        || native_kind(&kinds, instr.b()) == Some(NativeScalarKind::I64)
                    {
                        if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                            return None;
                        }
                    } else {
                        return None;
                    }
                } else if native_kind(&kinds, instr.b()) == Some(NativeScalarKind::I64)
                    || (0..pc).rev().any(|prev_pc| {
                        code.get(prev_pc)
                            .copied()
                            .is_some_and(|prev| prev.a() == instr.b() && prev.opcode() == Opcode::Call)
                    })
                {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Opcode::NewRange => {
                let value =
                    static_int_range_from_registers(&static_values, code, int_consts, pc, instr, String::new())?;
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                    return None;
                }
            }
            Opcode::Raise => {}
            Opcode::StoreCellVal => {
                if let (Some(cell), Some(value)) = (
                    static_kind(&static_values, instr.a()),
                    static_kind(&static_values, instr.b())
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.b())),
                ) && let Some(cell) = native_static_store_cell(cell, value)
                {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        static_value_kind(&cell),
                        cell,
                    ) {
                        return None;
                    }
                } else if let Some(kind) = native_kind(&kinds, instr.b()).or_else(|| native_kind(&kinds, instr.a())) {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                        return None;
                    }
                } else {
                    kinds[instr.a() as usize] = None;
                    static_values[instr.a() as usize] = None;
                }
            }
            Opcode::LoadCellVal => {
                if let Some(cell) = static_kind(&static_values, instr.b()).and_then(native_static_load_cell) {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        static_value_kind(&cell),
                        cell,
                    ) {
                        return None;
                    }
                } else {
                    let kind = native_kind(&kinds, instr.b()).unwrap_or(NativeScalarKind::I64);
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                        return None;
                    }
                }
            }
            _other => {
                return None;
            }
        }
    }
    Some(NativeScalarFacts {
        registers_before,
        globals_before,
    })
}

fn static_compare_test_value(
    opcode: Opcode,
    lhs: &NativeStraightlineValue,
    rhs: &NativeStraightlineValue,
) -> Option<bool> {
    let compare_opcode = match opcode {
        Opcode::TestEqInt | Opcode::TestEqIntI => Opcode::CmpInt,
        Opcode::TestNeInt | Opcode::TestNeIntI => Opcode::CmpNeInt,
        Opcode::TestLtInt | Opcode::TestLtIntI => Opcode::CmpLtInt,
        Opcode::TestLeInt | Opcode::TestLeIntI => Opcode::CmpLeInt,
        Opcode::TestGtInt | Opcode::TestGtIntI => Opcode::CmpGtInt,
        Opcode::TestGeInt | Opcode::TestGeIntI => Opcode::CmpGeInt,
        _ => return None,
    };
    native_static_compare_bool(lhs, rhs, compare_opcode)
}

fn field_key_value(strings: &[String], instr: Instr) -> Option<NativeStraightlineValue> {
    let value = strings.get(instr.c() as usize)?;
    Some(NativeStraightlineValue::String {
        symbol: String::new(),
        value: value.clone(),
        len: value.len(),
        key_kind: NativeStringKeyKind::Short,
    })
}
