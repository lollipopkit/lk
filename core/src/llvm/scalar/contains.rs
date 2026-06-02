mod int_lists;

use crate::llvm::{
    const_display::{llvm_string_constant, native_const_list_display},
    dynamic_containers::{emit_dynamic_int_list_copy, emit_dynamic_int_list_equality, emit_dynamic_int_list_slice},
    ir_text::next_tmp,
    straightline_value::{
        NativeListElementKind, NativeStraightlineValue, native_runtime_string_key_kind,
        native_static_collection_equality_bool, native_static_compare_bool, native_static_container_test,
        native_static_contains, native_static_index, native_static_int_range, native_static_list_from_values,
        native_static_map_rest, native_static_object_from_fields, native_static_slice_from, native_static_to_iter,
        native_straightline_heap_const_value,
    },
};
use crate::vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Opcode32};

pub(in crate::llvm) use int_lists::{
    local_static_iter_zip_before, static_int_list_chunk_method, static_int_list_filter_map_method,
    static_int_list_index_value, static_int_list_reduce_method, static_int_list_single_arg_method,
    static_int_list_values, static_int_list_zip_method, static_iter_builtin_call, static_list_empty_arg_method,
};
use int_lists::{static_dynamic_int_list_contains, static_dynamic_int_list_slice};

use super::{
    block_helpers::{
        emit_static_scalar_value_store_if_needed, local_register_kind_before, local_static_container_before,
        local_static_i64_before, static_register_value_trusted_before, store_native_scalar_call_result,
        text_value_from_reg, three_regs_in_bounds,
    },
    facts::{NativeScalarFacts, NativeScalarKind},
};

pub(in crate::llvm) fn static_object_from_registers(
    values: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    pc: usize,
    instr: Instr32,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let start = instr.b() as usize;
    let width = (instr.c() as usize).checked_mul(2)?.checked_add(1)?;
    let fields = values
        .get(start..start.checked_add(width)?)?
        .iter()
        .enumerate()
        .map(|(offset, value)| {
            value
                .clone()
                .or_else(|| local_static_i64_before(code, int_consts, pc, u8::try_from(start + offset).ok()?))
        })
        .collect::<Option<Vec<_>>>()?;
    native_static_object_from_fields(&fields, symbol)
}

pub(in crate::llvm) fn local_static_object_before(
    values: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    if let Some(value @ NativeStraightlineValue::Object { .. }) = values.get(reg as usize).cloned().flatten() {
        return Some(value);
    }
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::NewObject => static_object_from_registers(values, code, int_consts, prev_pc, prev, String::new()),
            Opcode32::Move if prev.b() != reg => {
                local_static_object_before(values, code, int_consts, prev_pc, prev.b())
            }
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn static_int_range_from_registers(
    values: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    pc: usize,
    instr: Instr32,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let start = instr.b() as usize;
    let value = |offset| {
        values
            .get(start + offset)
            .cloned()
            .flatten()
            .or_else(|| local_static_i64_before(code, int_consts, pc, u8::try_from(start + offset).ok()?))
    };
    native_static_int_range(value(0)?, value(1)?, value(2)?, instr.c() != 0, symbol)
}

pub(in crate::llvm) fn static_index_from_registers(
    values: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    target: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    if let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    } = target
        && !matches!(
            code.get(id).copied().map(Instr32::opcode),
            Some(Opcode32::LoadHeapConst)
        )
        && !static_register_value_trusted_before(code, pc, instr.b())
    {
        return None;
    }
    let key = static_register_value_trusted_before(code, pc, instr.c())
        .then(|| {
            values
                .get(instr.c() as usize)
                .and_then(Clone::clone)
                .or_else(|| local_static_heap_const_before(code, heap_values, pc, instr.c()))
                .or_else(|| local_static_i64_value_before(code, int_consts, strings, heap_values, pc, instr.c()))
                .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
        })
        .flatten()?;
    if let (
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        },
        NativeStraightlineValue::I64(_),
    ) = (&target, &key)
        && dynamic_list_mutated_before(code, heap_values, pc, *id)
    {
        return None;
    }
    native_static_index(target.clone(), key.clone(), String::new())
        .or_else(|| static_int_list_index_value(code, int_consts, strings, heap_values, &target, &key))
}

fn dynamic_list_mutated_before(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    list_id: usize,
) -> bool {
    (0..pc).any(|check_pc| {
        let Some(instr) = code.get(check_pc).copied() else {
            return false;
        };
        if !matches!(instr.opcode(), Opcode32::SetIndex | Opcode32::ListPush) {
            return false;
        }
        matches!(
            local_static_container_before(code, heap_values, check_pc, instr.a()),
            Some(NativeStraightlineValue::DynamicList { id, .. }) if id == list_id
        )
    })
}

pub(in crate::llvm) fn emit_static_map_iter_value_get(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<bool> {
    let key = local_static_i64_before(code, int_consts, pc, instr.c());

    let pair_reg = instr.b();
    let start = pc.saturating_sub(64);
    for pair_pc in (start..pc).rev() {
        let pair = code.get(pair_pc).copied()?;
        if pair.a() != pair_reg {
            continue;
        }
        if pair.opcode() != Opcode32::GetIndex {
            return Some(false);
        }
        if let Some(NativeStraightlineValue::I64(key)) = key.as_ref()
            && let Ok(field) = key.parse::<usize>()
            && let Some(NativeStraightlineValue::List { elements, .. }) =
                local_static_container_before(code, heap_values, pair_pc, pair.b())
        {
            if let Some(values) = elements
                .iter()
                .map(|value| match value {
                    ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
                        ConstHeapValue32Data::List(fields) => match fields.get(field)? {
                            ConstRuntimeValue32Data::Int(value) => Some(*value),
                            _ => None,
                        },
                        _ => None,
                    },
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
            {
                let index = next_tmp(tmp_index);
                ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", pair.c()));
                let mut selected = "0".to_string();
                for (idx, value) in values.into_iter().enumerate() {
                    let cmp = next_tmp(tmp_index);
                    let tmp = next_tmp(tmp_index);
                    ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
                    ir.push_str(&format!("  {tmp} = select i1 {cmp}, i64 {value}, i64 {selected}\n"));
                    selected = tmp;
                }
                ir.push_str(&format!("  store i64 {selected}, ptr %r{}.slot\n", instr.a()));
                *static_regs.get_mut(instr.a() as usize)? = None;
                return Some(true);
            }
            if elements
                .iter()
                .all(|value| nested_string_field_len(value, field).is_some())
            {
                *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::DynamicTextChar);
                return Some(true);
            }
        }

        let iter_reg = pair.b();
        for iter_pc in (start..pair_pc).rev() {
            let iter = code.get(iter_pc).copied()?;
            if iter.a() != iter_reg {
                continue;
            }
            if iter.opcode() != Opcode32::ToIter {
                return Some(false);
            }
            if key.is_none() && call_defines_reg_before(code, start, iter_pc, iter.b()) {
                *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::I64("0".to_string()));
                return Some(true);
            }
            if matches!(key, Some(NativeStraightlineValue::I64(ref value)) if value == "0")
                && call_defines_reg_before(code, start, iter_pc, iter.b())
            {
                let index = next_tmp(tmp_index);
                ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", pair.c()));
                ir.push_str(&format!("  store i64 {index}, ptr %r{}.slot\n", instr.a()));
                *static_regs.get_mut(instr.a() as usize)? = None;
                return Some(true);
            }
            if !matches!(key, Some(NativeStraightlineValue::I64(ref value)) if value == "1") {
                return Some(false);
            }
            let Some(NativeStraightlineValue::Map { mut entries, .. }) =
                local_static_container_before(code, heap_values, iter_pc, iter.b())
            else {
                return Some(false);
            };
            entries.sort_by(|(lhs, _), (rhs, _)| lhs.cmp(rhs));
            let values = entries
                .iter()
                .map(|(_, value)| match value {
                    ConstRuntimeValue32Data::Int(value) => Some(*value),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            if values.is_empty() {
                return Some(false);
            }
            let index = next_tmp(tmp_index);
            ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", pair.c()));
            let mut selected = "0".to_string();
            for (idx, value) in values.into_iter().enumerate() {
                let cmp = next_tmp(tmp_index);
                let tmp = next_tmp(tmp_index);
                ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
                ir.push_str(&format!("  {tmp} = select i1 {cmp}, i64 {value}, i64 {selected}\n"));
                selected = tmp;
            }
            ir.push_str(&format!("  store i64 {selected}, ptr %r{}.slot\n", instr.a()));
            *static_regs.get_mut(instr.a() as usize)? = None;
            return Some(true);
        }
        return Some(false);
    }
    Some(false)
}

fn call_defines_reg_before(code: &[Instr32], start: usize, pc: usize, reg: u8) -> bool {
    (start..pc).rev().any(|prev_pc| {
        let Some(prev) = code.get(prev_pc).copied() else {
            return false;
        };
        prev.a() == reg && prev.opcode() == Opcode32::Call
    })
}

fn nested_string_field_len(value: &ConstRuntimeValue32Data, field: usize) -> Option<usize> {
    let ConstRuntimeValue32Data::Heap(value) = value else {
        return None;
    };
    let ConstHeapValue32Data::List(fields) = value.as_ref() else {
        return None;
    };
    match fields.get(field)? {
        ConstRuntimeValue32Data::ShortStr(value) => Some(value.len()),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => Some(value.len()),
            _ => None,
        },
        _ => None,
    }
}

pub(in crate::llvm) fn emit_static_to_iter_block(
    static_regs: &mut [Option<NativeStraightlineValue>],
    register_count: usize,
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
) -> Option<()> {
    if instr.a() as usize >= register_count || instr.b() as usize >= register_count {
        return None;
    }
    let Some(target) = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
        .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, instr.b()))
        .or_else(|| local_static_string_before(code, strings, pc, instr.b()))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, instr.b()))
    else {
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::I64("0".to_string()));
        return Some(());
    };
    let value = match target {
        NativeStraightlineValue::DynamicMap { id, key, value } => {
            NativeStraightlineValue::DynamicMapIter { id, key, value }
        }
        NativeStraightlineValue::DynamicConstListElement { .. } => target,
        _ => native_static_to_iter(target, String::new()).unwrap_or(NativeStraightlineValue::I64("0".to_string())),
    };
    *static_regs.get_mut(instr.a() as usize)? = Some(value);
    Some(())
}

pub(in crate::llvm) fn emit_static_type_test_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    register_count: usize,
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    facts: &NativeScalarFacts,
    pc: usize,
    instr: Instr32,
) -> Option<()> {
    if !three_regs_in_bounds(register_count, instr) {
        return None;
    }
    let value = match instr.opcode() {
        Opcode32::IsNil => i64::from(facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::Nil)),
        Opcode32::IsList | Opcode32::IsMap => {
            static_container_test_value(static_regs, code, int_consts, strings, heap_values, facts, pc, instr)?
        }
        _ => return None,
    };
    *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::Bool(value.to_string()));
    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
    Some(())
}

pub(in crate::llvm) fn local_static_map_rest_before(
    code: &[Instr32],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::MapRest => {
                let target = local_static_container_before(code, heap_values, prev_pc, prev.b())?;
                let keys = (0..prev.c())
                    .map(|i| local_static_string_before(code, strings, prev_pc, prev.b().checked_add(1 + i)?))
                    .collect::<Option<Vec<_>>>()?;
                native_static_map_rest(target, &keys, String::new())
            }
            Opcode32::Move if prev.b() != reg => {
                local_static_map_rest_before(code, strings, heap_values, prev_pc, prev.b())
            }
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn local_static_i64_value_before(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::Move if prev.b() != reg => {
                local_static_i64_value_before(code, int_consts, strings, heap_values, prev_pc, prev.b())
            }
            Opcode32::GetIndex => {
                let target = local_static_container_before(code, heap_values, prev_pc, prev.b())
                    .or_else(|| local_static_map_rest_before(code, strings, heap_values, prev_pc, prev.b()))
                    .or_else(|| {
                        local_static_index_value_before(code, int_consts, strings, heap_values, prev_pc, prev.b())
                    });
                let key = local_static_heap_const_before(code, heap_values, prev_pc, prev.c())
                    .or_else(|| local_static_string_before(code, strings, prev_pc, prev.c()))
                    .or_else(|| local_static_i64_before(code, int_consts, prev_pc, prev.c()));
                let target = target?;
                let key = key?;
                let value = native_static_index(target.clone(), key.clone(), String::new())
                    .or_else(|| static_int_list_index_value(code, int_consts, strings, heap_values, &target, &key))?;
                matches!(value, NativeStraightlineValue::I64(_)).then_some(value)
            }
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn local_static_index_value_before(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::GetIndex => {
                let target = local_static_container_before(code, heap_values, prev_pc, prev.b())
                    .or_else(|| local_static_map_rest_before(code, strings, heap_values, prev_pc, prev.b()))
                    .or_else(|| {
                        local_static_index_value_before(code, int_consts, strings, heap_values, prev_pc, prev.b())
                    })?;
                let key = local_static_heap_const_before(code, heap_values, prev_pc, prev.c())
                    .or_else(|| local_static_string_before(code, strings, prev_pc, prev.c()))
                    .or_else(|| local_static_i64_before(code, int_consts, prev_pc, prev.c()))?;
                native_static_index(target.clone(), key.clone(), String::new())
                    .or_else(|| static_int_list_index_value(code, int_consts, strings, heap_values, &target, &key))
            }
            Opcode32::SliceFrom => {
                let target = local_static_container_before(code, heap_values, prev_pc, prev.b())
                    .or_else(|| local_static_map_rest_before(code, strings, heap_values, prev_pc, prev.b()))
                    .or_else(|| {
                        local_static_index_value_before(code, int_consts, strings, heap_values, prev_pc, prev.b())
                    })?;
                let start = local_static_i64_before(code, int_consts, prev_pc, prev.c())?;
                static_slice_from_value(code, heap_values, target, start, String::new())
            }
            Opcode32::Move if prev.b() != reg => {
                local_static_index_value_before(code, int_consts, strings, heap_values, prev_pc, prev.b())
            }
            Opcode32::Call if prev.c() == 3 => {
                local_static_core_get_call_before(code, int_consts, strings, heap_values, prev_pc, prev)
            }
            Opcode32::NewList => local_static_new_list_before(code, int_consts, strings, heap_values, prev_pc, prev),
            _ => None,
        };
    }
    None
}

fn local_static_new_list_before(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
) -> Option<NativeStraightlineValue> {
    let mut values = Vec::with_capacity(instr.c() as usize);
    for reg in instr.b()..instr.b().checked_add(instr.c())? {
        values.push(
            local_static_string_before(code, strings, pc, reg)
                .or_else(|| local_static_i64_before(code, int_consts, pc, reg))
                .or_else(|| local_static_bool_before(code, pc, reg))
                .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, reg))?,
        );
    }
    native_static_list_from_values(&values, String::new())
}

fn local_static_bool_before(code: &[Instr32], pc: usize, reg: u8) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::LoadBool => Some(NativeStraightlineValue::Bool(i64::from(prev.b() != 0).to_string())),
            Opcode32::Move if prev.b() != reg => local_static_bool_before(code, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

fn local_static_core_get_call_before(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    call: Instr32,
) -> Option<NativeStraightlineValue> {
    let receiver = call.b().checked_add(1)?;
    let method = call.b().checked_add(2)?;
    let args = call.b().checked_add(3)?;
    let NativeStraightlineValue::String { value: method, .. } = local_static_string_before(code, strings, pc, method)?
    else {
        return None;
    };
    let receiver = local_static_heap_const_before(code, heap_values, pc, receiver)
        .or_else(|| local_static_container_before(code, heap_values, pc, receiver))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, receiver))?;
    let key = local_static_call_single_arg(code, int_consts, strings, heap_values, pc, args)?;
    match method.as_str() {
        "get" => native_static_index(receiver, key, String::new()),
        "skip" => native_static_slice_from(receiver, key, String::new()),
        "take" => {
            let (NativeStraightlineValue::List { mut elements, .. }, NativeStraightlineValue::I64(count)) =
                (receiver, key)
            else {
                return None;
            };
            elements.truncate(usize::try_from(count.parse::<i64>().ok()?.max(0)).ok()?);
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol: String::new(),
                elements,
            })
        }
        "concat" | "chain" => {
            let (
                NativeStraightlineValue::List { mut elements, .. },
                NativeStraightlineValue::List { elements: rhs, .. },
            ) = (receiver, key)
            else {
                return None;
            };
            elements.extend(rhs);
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol: String::new(),
                elements,
            })
        }
        _ => None,
    }
}

fn local_static_call_single_arg(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::NewList if prev.c() == 1 => local_static_string_before(code, strings, prev_pc, prev.b())
                .or_else(|| local_static_i64_before(code, int_consts, prev_pc, prev.b()))
                .or_else(|| local_static_heap_const_before(code, heap_values, prev_pc, prev.b()))
                .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, prev_pc, prev.b())),
            Opcode32::Move if prev.b() != reg => {
                local_static_call_single_arg(code, int_consts, strings, heap_values, prev_pc, prev.b())
            }
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn local_static_index_key_before(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    if !static_register_value_trusted_before(code, pc, reg) {
        return None;
    }
    let key = static_regs
        .get(reg as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_value_before(code, int_consts, strings, heap_values, pc, reg))
        .or_else(|| local_static_i64_before(code, int_consts, pc, reg))?;
    match &key {
        NativeStraightlineValue::I64(value) if value.parse::<i64>().is_ok() => Some(key),
        _ => None,
    }
}

pub(in crate::llvm) fn local_direct_load_nil_before(code: &[Instr32], pc: usize, reg: u8) -> bool {
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let Some(prev) = code.get(prev_pc).copied() else {
            return false;
        };
        if prev.a() != reg {
            continue;
        }
        return prev.opcode() == Opcode32::LoadNil;
    }
    false
}

pub(in crate::llvm) fn local_static_callable_before(
    functions: &[crate::vm::Function32Data],
    code: &[Instr32],
    pc: usize,
    reg: u8,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    if let Some(value @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. })) =
        static_regs.get(reg as usize).cloned().flatten()
    {
        return Some(value);
    }
    for prev_pc in (0..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::Move => local_static_callable_before(functions, code, prev_pc, prev.b(), static_regs),
            Opcode32::LoadFunction => Some(NativeStraightlineValue::Function(prev.bx())),
            Opcode32::MakeClosure if functions.get(prev.b() as usize)?.capture_count == 0 => {
                Some(NativeStraightlineValue::Closure {
                    function_index: prev.b() as u16,
                    captures: Vec::new(),
                })
            }
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn text_value_from_trusted_reg(
    ir: &mut String,
    code: &[Instr32],
    pc: usize,
    reg: u8,
    kind: Option<NativeScalarKind>,
    static_regs: &[Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if static_register_value_trusted_before(code, pc, reg) {
        text_value_from_reg(ir, reg, kind, static_regs, tmp_index)
    } else {
        text_value_from_reg(ir, reg, kind, &[], tmp_index)
    }
}

pub(in crate::llvm) fn local_static_string_before(
    code: &[Instr32],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::LoadString => {
                let value = strings.get(prev.bx() as usize)?;
                Some(NativeStraightlineValue::String {
                    symbol: String::new(),
                    value: value.clone(),
                    len: value.chars().count(),
                    key_kind: native_runtime_string_key_kind(value),
                })
            }
            Opcode32::Move if prev.b() != reg => local_static_string_before(code, strings, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn local_static_heap_const_before(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::LoadHeapConst => {
                native_straightline_heap_const_value(0, prev.bx(), heap_values.get(prev.bx() as usize)?)
            }
            Opcode32::Move if prev.b() != reg => local_static_heap_const_before(code, heap_values, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

fn static_container_test_value(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    facts: &NativeScalarFacts,
    pc: usize,
    instr: Instr32,
) -> Option<i64> {
    if let Some(value) = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
        .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, instr.b()))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, instr.b()))
    {
        return native_static_container_test(value, instr.opcode()).and_then(|value| match value {
            NativeStraightlineValue::Bool(value) => value.parse::<i64>().ok(),
            _ => None,
        });
    }
    Some(i64::from(matches!(
        (instr.opcode(), facts.register_kind_before(pc, instr.b())),
        (Opcode32::IsList, Some(NativeScalarKind::StrPtr))
    )))
}

pub(in crate::llvm) fn emit_static_contains_or_slice_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    register_count: usize,
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<()> {
    match instr.opcode() {
        Opcode32::Contains if three_regs_in_bounds(register_count, instr) => {
            emit_static_contains_block(ir, static_regs, code, int_consts, heap_values, pc, instr)
        }
        Opcode32::SliceFrom => {
            if !three_regs_in_bounds(register_count, instr) {
                return None;
            }
            emit_static_slice_from_block(
                ir,
                extra_globals,
                static_regs,
                code,
                int_consts,
                heap_values,
                pc,
                instr,
                tmp_index,
            )
        }
        Opcode32::MapRest => emit_static_map_rest_block(static_regs, instr, tmp_index),
        _ => None,
    }
}

pub(in crate::llvm) fn emit_dynamic_int_list_move(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<bool> {
    let src = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()));
    let Some(NativeStraightlineValue::DynamicList {
        id: src_id,
        element: NativeListElementKind::I64,
    }) = src
    else {
        return Some(false);
    };
    let dst = if matches!(
        code.get(src_id).copied().map(Instr32::opcode),
        Some(Opcode32::SliceFrom)
    ) {
        local_static_container_before(code, heap_values, pc, instr.a())
    } else {
        None
    };
    if let Some(NativeStraightlineValue::DynamicList {
        id: dst_id,
        element: NativeListElementKind::I64,
    }) = dst
    {
        emit_dynamic_int_list_copy(ir, src_id, dst_id, tmp_index)?;
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::DynamicList {
            id: dst_id,
            element: NativeListElementKind::I64,
        });
    } else {
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::DynamicList {
            id: src_id,
            element: NativeListElementKind::I64,
        });
    }
    Some(true)
}

fn emit_static_contains_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
) -> Option<()> {
    let needle = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.b()))?;
    let haystack = static_regs
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.c()))?;
    let value = native_static_contains(needle.clone(), haystack.clone())
        .or_else(|| static_dynamic_int_list_contains(code, heap_values, needle, haystack))?;
    emit_static_scalar_value_store_if_needed(ir, instr.a(), &value)?;
    *static_regs.get_mut(instr.a() as usize)? = Some(value);
    Some(())
}

fn emit_static_slice_from_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<()> {
    let target = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()));
    let start = static_regs
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))?;
    let Some(target) = target else {
        ir.push_str(&format!("  store i64 0, ptr %list{pc}.len.slot\n"));
        ir.push_str(&format!("  store i64 0, ptr %list{pc}.text.len.slot\n"));
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::DynamicList {
            id: pc,
            element: NativeListElementKind::I64,
        });
        return Some(());
    };
    if let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    } = target
    {
        emit_dynamic_int_list_slice(ir, id, pc, instr.c(), tmp_index)?;
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::DynamicList {
            id: pc,
            element: NativeListElementKind::I64,
        });
        return Some(());
    }
    if let NativeStraightlineValue::DynamicConstListElement { elements, index } = target {
        let NativeStraightlineValue::I64(start) = start else {
            return None;
        };
        let start = start.parse::<usize>().ok()?;
        let mut sliced = Vec::with_capacity(elements.len());
        for value in elements {
            let ConstRuntimeValue32Data::Heap(value) = value else {
                return None;
            };
            let ConstHeapValue32Data::List(values) = value.as_ref() else {
                return None;
            };
            let tail = values.iter().skip(start).cloned().collect();
            sliced.push(ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(
                tail,
            ))));
        }
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::DynamicConstListElement {
            elements: sliced,
            index,
        });
        return Some(());
    }
    let symbol = format!("@lk_block_slice_str_{}", *tmp_index);
    *tmp_index += 1;
    let value = static_slice_from_value(code, heap_values, target, start, symbol)?;
    if store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value.clone(), tmp_index).is_none() {
        emit_static_scalar_value_store_if_needed(ir, instr.a(), &value)?;
        *static_regs.get_mut(instr.a() as usize)? = Some(value);
    }
    Some(())
}

fn emit_static_map_rest_block(
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<()> {
    let start = instr.b() as usize;
    let end = start.checked_add(1usize.checked_add(instr.c() as usize)?)?;
    let values = static_regs.get(start..end)?;
    let target = values.first()?.clone()?;
    let keys = values[1..].iter().cloned().collect::<Option<Vec<_>>>()?;
    let symbol = format!("@lk_block_map_rest_{}", *tmp_index);
    *tmp_index += 1;
    let value = native_static_map_rest(target, &keys, symbol)?;
    *static_regs.get_mut(instr.a() as usize)? = Some(value);
    Some(())
}

pub(in crate::llvm) fn static_slice_from_value(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    start: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    native_static_slice_from(target.clone(), start.clone(), symbol.clone())
        .or_else(|| static_dynamic_int_list_slice(code, heap_values, target, start, symbol))
}

pub(in crate::llvm) fn static_int_list_compare_bool(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    lhs: &NativeStraightlineValue,
    rhs: &NativeStraightlineValue,
    opcode: Opcode32,
) -> Option<bool> {
    if !matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) {
        return None;
    }
    let lhs = static_int_list_values(code, &[], &[], heap_values, lhs)?;
    let rhs = static_int_list_values(code, &[], &[], heap_values, rhs)?;
    let equal = lhs == rhs;
    Some(if opcode == Opcode32::CmpNeInt { !equal } else { equal })
}

pub(in crate::llvm) fn emit_static_collection_compare_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
) -> Option<()> {
    let lhs = static_compare_value(static_regs, code, int_consts, strings, heap_values, pc, instr.b())?;
    let rhs = static_compare_value(static_regs, code, int_consts, strings, heap_values, pc, instr.c())?;
    if let (NativeStraightlineValue::Bool(lhs), NativeStraightlineValue::Bool(rhs)) = (&lhs, &rhs)
        && matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt)
    {
        let equal = lhs == rhs;
        let value = if instr.opcode() == Opcode32::CmpNeInt {
            !equal
        } else {
            equal
        };
        let stored = i64::from(value);
        ir.push_str(&format!("  store i64 {stored}, ptr %r{}.slot\n", instr.a()));
        *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::Bool(stored.to_string()));
        return Some(());
    }
    if matches!(
        (&lhs, &rhs),
        (
            NativeStraightlineValue::I64(_) | NativeStraightlineValue::F64(_) | NativeStraightlineValue::Bool(_),
            NativeStraightlineValue::I64(_) | NativeStraightlineValue::F64(_) | NativeStraightlineValue::Bool(_)
        )
    ) {
        return None;
    }
    if let Some((id, elements)) =
        dynamic_ptr_list_compare_parts(&lhs, &rhs, code, int_consts, strings, heap_values, pc, instr.b()).or_else(
            || dynamic_ptr_list_compare_parts(&rhs, &lhs, code, int_consts, strings, heap_values, pc, instr.c()),
        )
    {
        emit_dynamic_ptr_static_string_list_compare(ir, extra_globals, instr, pc, id, elements)?;
        *static_regs.get_mut(instr.a() as usize)? = None;
        return Some(());
    }
    if let Some((id, elements)) =
        dynamic_text_list_compare_parts(&lhs, &rhs).or_else(|| dynamic_text_list_compare_parts(&rhs, &lhs))
    {
        let total = static_string_list_total_len(elements)?;
        let len = format!("%text_list_cmp_len_{pc}");
        let text_len = format!("%text_list_cmp_text_len_{pc}");
        let len_ok = format!("%text_list_cmp_len_ok_{pc}");
        let text_ok = format!("%text_list_cmp_text_ok_{pc}");
        let ok = format!("%text_list_cmp_ok_{pc}");
        let out = format!("%text_list_cmp_out_{pc}");
        ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
        ir.push_str(&format!("  {text_len} = load i64, ptr %list{id}.text.len.slot\n"));
        ir.push_str(&format!("  {len_ok} = icmp eq i64 {len}, {}\n", elements.len()));
        ir.push_str(&format!("  {text_ok} = icmp eq i64 {text_len}, {total}\n"));
        ir.push_str(&format!("  {ok} = and i1 {len_ok}, {text_ok}\n"));
        if instr.opcode() == Opcode32::CmpNeInt {
            let neg = format!("%text_list_cmp_ne_{pc}");
            ir.push_str(&format!("  {neg} = xor i1 {ok}, true\n"));
            ir.push_str(&format!("  {out} = zext i1 {neg} to i64\n"));
        } else {
            ir.push_str(&format!("  {out} = zext i1 {ok} to i64\n"));
        }
        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
        *static_regs.get_mut(instr.a() as usize)? = None;
        return Some(());
    }
    if emit_dynamic_int_static_list_compare(ir, instr, pc, &lhs, &rhs)
        .or_else(|| emit_dynamic_int_static_list_compare(ir, instr, pc, &rhs, &lhs))
        .is_some()
    {
        *static_regs.get_mut(instr.a() as usize)? = None;
        return Some(());
    }
    if matches!(
        (&lhs, &rhs),
        (
            NativeStraightlineValue::DynamicList {
                element: NativeListElementKind::I64,
                ..
            },
            _
        ) | (
            _,
            NativeStraightlineValue::DynamicList {
                element: NativeListElementKind::I64,
                ..
            }
        )
    ) {
        return None;
    }
    let value = static_int_list_compare_bool(code, heap_values, &lhs, &rhs, instr.opcode())
        .or_else(
            || match native_static_collection_equality_bool(&lhs, &rhs, instr.opcode())? {
                NativeStraightlineValue::Bool(value) => Some(value != "0"),
                _ => None,
            },
        )
        .or_else(|| native_static_compare_bool(&lhs, &rhs, instr.opcode()))?;
    let stored = i64::from(value);
    ir.push_str(&format!("  store i64 {stored}, ptr %r{}.slot\n", instr.a()));
    *static_regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::Bool(stored.to_string()));
    Some(())
}

fn emit_dynamic_int_static_list_compare(
    ir: &mut String,
    instr: Instr32,
    pc: usize,
    lhs: &NativeStraightlineValue,
    rhs: &NativeStraightlineValue,
) -> Option<()> {
    if !matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) {
        return None;
    }
    let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    } = lhs
    else {
        return None;
    };
    let NativeStraightlineValue::List { elements, .. } = rhs else {
        return None;
    };
    let expected = elements
        .iter()
        .map(|value| match value {
            ConstRuntimeValue32Data::Int(value) => Some(*value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let len = format!("%int_list_cmp_len_{pc}");
    let mut ok = format!("%int_list_cmp_len_ok_{pc}");
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {ok} = icmp eq i64 {len}, {}\n", expected.len()));
    for (index, value) in expected.iter().enumerate() {
        let slot = format!("%int_list_cmp_slot_{pc}_{index}");
        let actual = format!("%int_list_cmp_value_{pc}_{index}");
        let same = format!("%int_list_cmp_same_{pc}_{index}");
        let next = format!("%int_list_cmp_ok_{pc}_{index}");
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {index}\n"
        ));
        ir.push_str(&format!("  {actual} = load i64, ptr {slot}\n"));
        ir.push_str(&format!("  {same} = icmp eq i64 {actual}, {value}\n"));
        ir.push_str(&format!("  {next} = and i1 {ok}, {same}\n"));
        ok = next;
    }
    let out = format!("%int_list_cmp_out_{pc}");
    if instr.opcode() == Opcode32::CmpNeInt {
        let neg = format!("%int_list_cmp_ne_{pc}");
        ir.push_str(&format!("  {neg} = xor i1 {ok}, true\n"));
        ir.push_str(&format!("  {out} = zext i1 {neg} to i64\n"));
    } else {
        ir.push_str(&format!("  {out} = zext i1 {ok} to i64\n"));
    }
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_int_list_compare_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<()> {
    if !matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) {
        return None;
    }
    let lhs = static_compare_value(static_regs, code, int_consts, strings, heap_values, pc, instr.b())?;
    let rhs = static_compare_value(static_regs, code, int_consts, strings, heap_values, pc, instr.c())?;
    let (
        NativeStraightlineValue::DynamicList {
            id: lhs_id,
            element: NativeListElementKind::I64,
        },
        NativeStraightlineValue::DynamicList {
            id: rhs_id,
            element: NativeListElementKind::I64,
        },
    ) = (lhs, rhs)
    else {
        return None;
    };
    emit_dynamic_int_list_equality(
        ir,
        lhs_id,
        rhs_id,
        instr.a(),
        instr.opcode() == Opcode32::CmpNeInt,
        tmp_index,
    )?;
    *static_regs.get_mut(instr.a() as usize)? = None;
    Some(())
}

fn dynamic_text_list_compare_parts<'a>(
    lhs: &'a NativeStraightlineValue,
    rhs: &'a NativeStraightlineValue,
) -> Option<(usize, &'a [ConstRuntimeValue32Data])> {
    let id = match lhs {
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Text,
        }
        | NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        } => *id,
        _ => return None,
    };
    let NativeStraightlineValue::List { elements, .. } = rhs else {
        return None;
    };
    static_string_list_total_len(elements)?;
    Some((id, elements))
}

fn dynamic_ptr_list_compare_parts<'a>(
    lhs: &'a NativeStraightlineValue,
    rhs: &'a NativeStraightlineValue,
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    lhs_reg: u8,
) -> Option<(usize, &'a [ConstRuntimeValue32Data])> {
    let NativeStraightlineValue::DynamicList { id, element } = lhs else {
        return None;
    };
    if *element != NativeListElementKind::StrPtr
        && !recent_list_push_value_is_strptr(code, int_consts, strings, heap_values, pc, lhs_reg)
    {
        return None;
    }
    let NativeStraightlineValue::List { elements, .. } = rhs else {
        return None;
    };
    static_string_list_total_len(elements)?;
    Some((*id, elements))
}

fn recent_list_push_value_is_strptr(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    list_reg: u8,
) -> bool {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let Some(prev) = code.get(prev_pc).copied() else {
            return false;
        };
        if prev.a() != list_reg {
            continue;
        }
        return prev.opcode() == Opcode32::ListPush
            && (local_register_kind_before(code, prev_pc, prev.b()) == Some(NativeScalarKind::StrPtr)
                || local_nested_const_list_field_is_string(code, int_consts, heap_values, prev_pc, prev.b())
                || local_static_string_index_is_string(code, strings, prev_pc, prev.b()));
    }
    false
}

fn local_nested_const_list_field_is_string(
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    value_reg: u8,
) -> bool {
    let Some(inner) = previous_writer(code, pc, value_reg) else {
        return false;
    };
    if inner.opcode() != Opcode32::GetIndex {
        return false;
    }
    let Some(NativeStraightlineValue::I64(field)) = local_static_i64_before(code, int_consts, pc, inner.c()) else {
        return false;
    };
    let Some(field) = field.parse::<usize>().ok() else {
        return false;
    };
    let Some(outer) = previous_writer(code, pc, inner.b()) else {
        return false;
    };
    if outer.opcode() != Opcode32::GetIndex {
        return false;
    }
    let Some(NativeStraightlineValue::List { elements, .. }) =
        local_static_container_before(code, heap_values, pc, outer.b())
    else {
        return false;
    };
    elements.iter().all(|row| match row {
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::List(values) => values.get(field).and_then(static_string_value).is_some(),
            _ => false,
        },
        _ => false,
    })
}

fn local_static_string_index_is_string(code: &[Instr32], strings: &[String], pc: usize, value_reg: u8) -> bool {
    let Some(instr) = previous_writer(code, pc, value_reg) else {
        return false;
    };
    instr.opcode() == Opcode32::GetIndex && local_static_string_before(code, strings, pc, instr.b()).is_some()
}

fn previous_writer(code: &[Instr32], pc: usize, reg: u8) -> Option<Instr32> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() == reg {
            return Some(prev);
        }
    }
    None
}

fn emit_dynamic_ptr_static_string_list_compare(
    ir: &mut String,
    extra_globals: &mut String,
    instr: Instr32,
    pc: usize,
    id: usize,
    elements: &[ConstRuntimeValue32Data],
) -> Option<()> {
    let len = format!("%ptr_list_cmp_len_{pc}");
    let len_ok = format!("%ptr_list_cmp_len_ok_{pc}");
    let items_label = format!("ptr.list.cmp.{pc}.items");
    let done_label = format!("ptr.list.cmp.{pc}.done");
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {len_ok} = icmp eq i64 {len}, {}\n", elements.len()));
    ir.push_str(&format!(
        "  br i1 {len_ok}, label %{items_label}, label %{done_label}\n"
    ));
    ir.push_str(&format!("{items_label}:\n"));
    let mut ok = "true".to_string();
    for (index, value) in elements.iter().enumerate() {
        let expected = static_string_value(value)?;
        let symbol = format!("@lk_ptr_list_cmp_{pc}_{index}");
        extra_globals.push_str(&llvm_string_constant(&symbol, &expected));
        let slot = format!("%ptr_list_cmp_slot_{pc}_{index}");
        let actual = format!("%ptr_list_cmp_value_{pc}_{index}");
        let cmp = format!("%ptr_list_cmp_raw_{pc}_{index}");
        let same = format!("%ptr_list_cmp_same_{pc}_{index}");
        let next = format!("%ptr_list_cmp_ok_{pc}_{index}");
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 {index}\n"
        ));
        ir.push_str(&format!("  {actual} = load ptr, ptr {slot}\n"));
        ir.push_str(&format!("  {cmp} = call i32 @strcmp(ptr {actual}, ptr {symbol})\n"));
        ir.push_str(&format!("  {same} = icmp eq i32 {cmp}, 0\n"));
        ir.push_str(&format!("  {next} = and i1 {ok}, {same}\n"));
        ok = next;
    }
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
    let final_ok = format!("%ptr_list_cmp_final_{pc}");
    let out = format!("%ptr_list_cmp_out_{pc}");
    let incoming_ok = if elements.is_empty() { "true" } else { ok.as_str() };
    ir.push_str(&format!(
        "  {final_ok} = phi i1 [ false, %bb{pc} ], [ {incoming_ok}, %{items_label} ]\n"
    ));
    if instr.opcode() == Opcode32::CmpNeInt {
        let neg = format!("%ptr_list_cmp_ne_{pc}");
        ir.push_str(&format!("  {neg} = xor i1 {final_ok}, true\n"));
        ir.push_str(&format!("  {out} = zext i1 {neg} to i64\n"));
    } else {
        ir.push_str(&format!("  {out} = zext i1 {final_ok} to i64\n"));
    }
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
    Some(())
}

fn static_string_value(value: &ConstRuntimeValue32Data) -> Option<String> {
    match value {
        ConstRuntimeValue32Data::ShortStr(value) => Some(value.as_str().to_string()),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => Some(value.to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn static_string_list_total_len(elements: &[ConstRuntimeValue32Data]) -> Option<usize> {
    elements.iter().try_fold(0usize, |total, value| match value {
        ConstRuntimeValue32Data::ShortStr(value) => Some(total + value.len()),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => Some(total + value.len()),
            _ => None,
        },
        _ => None,
    })
}

fn static_compare_value(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let trusted_static = static_register_value_trusted_before(code, pc, reg)
        .then(|| static_regs.get(reg as usize).and_then(Clone::clone))
        .flatten();
    trusted_static
        .or_else(|| local_static_container_before(code, heap_values, pc, reg))
        .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, reg))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, reg))
}
