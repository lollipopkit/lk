use crate::{
    llvm::{
        const_display::llvm_string_constant,
        dynamic_containers::{
            emit_dynamic_f64_list_get, emit_dynamic_i64_int_map_get, emit_dynamic_i64_int_map_iter_key,
            emit_dynamic_i64_int_map_iter_value, emit_dynamic_int_list_get, emit_dynamic_ptr_list_get,
            emit_dynamic_string_f64_map_get, emit_dynamic_string_f64_map_iter_value, emit_dynamic_string_int_map_get,
            emit_dynamic_string_int_map_iter_key, emit_dynamic_string_int_map_iter_value,
        },
        ir_text::{emit_branch_to_next, native_relative_target, next_tmp},
        scalar::{
            block_helpers::{
                emit_static_string_i64_map_get, local_register_kind_before, local_static_container_before,
                local_static_i64_before, static_register_value_trusted_before, static_string_i64_map_supported,
                static_string_value_trusted_at_call, store_native_scalar_call_result, three_regs_in_bounds,
            },
            blocks::const_lists::{
                emit_const_list_element_dynamic_index, emit_const_list_element_index, static_const_list_elements,
            },
            contains::{
                emit_static_map_iter_value_get, local_static_heap_const_before, local_static_i64_value_before,
                local_static_index_key_before, local_static_index_value_before, local_static_map_rest_before,
                local_static_string_before, static_index_from_registers,
            },
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{
            NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
            native_const_runtime_string, native_static_arg_list_display, native_static_index,
        },
    },
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Opcode32},
};

pub(super) fn emit_get_index_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    if emit_static_map_iter_value_get(ir, static_regs, code, int_consts, heap_values, pc, instr, tmp_index)
        == Some(true)
    {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let target = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
        .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, instr.b()))
        .or_else(|| local_static_string_before(code, strings, pc, instr.b()))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, instr.b()));
    let Some(target) = target else {
        if emit_local_new_list_index(ir, static_regs, code, int_consts, pc, instr, facts, tmp_index) {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_local_dynamic_const_list_element_index(
            ir,
            extra_globals,
            static_regs,
            code,
            int_consts,
            heap_values,
            pc,
            instr,
            facts,
            tmp_index,
        ) {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if facts
            .register_kind_before(pc, instr.b())
            .or_else(|| local_register_kind_before(code, pc, instr.b()))
            == Some(NativeScalarKind::I64)
        {
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64("0".to_string()));
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        return false;
    };
    if matches!(target, NativeStraightlineValue::DynamicConstListElement { .. }) {
        let ok = emit_get_index_target(
            ir,
            extra_globals,
            static_regs,
            code,
            int_consts,
            strings,
            heap_values,
            pc,
            instr,
            facts,
            target,
            tmp_index,
        );
        if ok {
            emit_branch_to_next(ir, pc, code.len());
        }
        return ok;
    }
    if let Some(value) = static_index_from_registers(
        static_regs,
        code,
        int_consts,
        strings,
        heap_values,
        pc,
        instr,
        target.clone(),
    ) {
        if store_index_value(ir, extra_globals, static_regs, instr.a(), value, tmp_index).is_none() {
            return false;
        }
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if let Some(value) = static_string_key_index_before(static_regs, code, strings, pc, instr, target.clone()) {
        if store_index_value(ir, extra_globals, static_regs, instr.a(), value, tmp_index).is_none() {
            return false;
        }
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let ok = emit_get_index_target(
        ir,
        extra_globals,
        static_regs,
        code,
        int_consts,
        strings,
        heap_values,
        pc,
        instr,
        facts,
        target,
        tmp_index,
    );
    if ok {
        emit_branch_to_next(ir, pc, code.len());
    }
    ok
}

fn emit_local_new_list_index(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    pc: usize,
    instr: Instr32,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    let Some(NativeStraightlineValue::I64(key)) = static_regs
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
    else {
        return false;
    };
    let Some(index) = key.parse::<usize>().ok() else {
        return false;
    };
    let Some((start, len)) = local_new_list_source_before(code, pc, instr.b()) else {
        return false;
    };
    if index >= len {
        return false;
    }
    let Some(src) = start.checked_add(index).and_then(|reg| u8::try_from(reg).ok()) else {
        return false;
    };
    match facts
        .register_kind_before(pc, src)
        .or_else(|| local_register_kind_before(code, pc, src))
    {
        Some(NativeScalarKind::Bool | NativeScalarKind::I64 | NativeScalarKind::Nil | NativeScalarKind::MaybeI64) => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{src}.slot\n"));
            ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = None;
            true
        }
        Some(NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr) => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load ptr, ptr %r{src}.slot\n"));
            ir.push_str(&format!("  store ptr {value}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(value));
            true
        }
        Some(NativeScalarKind::F64) => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load double, ptr %r{src}.slot\n"));
            ir.push_str(&format!("  store double {value}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = None;
            true
        }
        None => false,
    }
}

fn local_new_list_source_before(code: &[Instr32], pc: usize, reg: u8) -> Option<(usize, usize)> {
    let start = pc.saturating_sub(128);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            crate::vm::Opcode32::NewList => Some((prev.b() as usize, prev.c() as usize)),
            crate::vm::Opcode32::Move if prev.b() != reg => local_new_list_source_before(code, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

fn static_string_key_index_before(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    strings: &[String],
    pc: usize,
    instr: Instr32,
    target: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let key = static_regs
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| dominating_static_string_before(code, strings, pc, instr.c()))?;
    native_static_index(target, key, String::new())
}

fn dominating_static_string_before(
    code: &[Instr32],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let (write_pc, instr) = last_write_before(code, pc, reg)?;
    if branch_before_write_can_skip_to(code, write_pc, pc) {
        return None;
    }
    match instr.opcode() {
        Opcode32::LoadString => local_static_string_before(code, strings, pc, reg),
        Opcode32::Move if instr.b() != reg => dominating_static_string_before(code, strings, write_pc, instr.b()),
        _ => None,
    }
}

fn last_write_before(code: &[Instr32], pc: usize, reg: u8) -> Option<(usize, Instr32)> {
    code.iter().copied().take(pc).enumerate().rev().find(|(_, instr)| {
        instr.a() == reg && !matches!(instr.opcode(), Opcode32::Nop | Opcode32::Jmp | Opcode32::Test)
    })
}

fn branch_before_write_can_skip_to(code: &[Instr32], write_pc: usize, pc: usize) -> bool {
    code.iter()
        .copied()
        .take(write_pc)
        .enumerate()
        .any(|(branch_pc, instr)| {
            let target = match instr.opcode() {
                Opcode32::Jmp => native_relative_target(branch_pc, instr.sj_arg(), code.len()),
                Opcode32::Test => native_relative_target(branch_pc, instr.c() as i8 as i32, code.len()),
                _ => None,
            };
            matches!(target, Some(target) if target > write_pc && target <= pc)
        })
}

#[allow(clippy::too_many_arguments)]
fn emit_get_index_target(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    facts: &NativeScalarFacts,
    target: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> bool {
    if matches!(
        target,
        NativeStraightlineValue::Text(_) | NativeStraightlineValue::String { .. }
    ) {
        return emit_text_index(
            ir,
            extra_globals,
            static_regs,
            code,
            pc,
            instr,
            facts,
            target,
            tmp_index,
        );
    }
    if let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    } = target
    {
        let index_kind = facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()));
        if index_kind != Some(NativeScalarKind::I64)
            || emit_dynamic_int_list_get(ir, id, instr.a(), instr.c(), tmp_index).is_none()
        {
            return false;
        }
        static_regs[instr.a() as usize] = None;
        return true;
    }
    if let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::F64,
    } = target
    {
        let index_kind = facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()));
        if index_kind != Some(NativeScalarKind::I64)
            || emit_dynamic_f64_list_get(ir, id, instr.a(), instr.c(), tmp_index).is_none()
        {
            return false;
        }
        static_regs[instr.a() as usize] = None;
        return true;
    }
    if let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
    } = target
    {
        let index_kind = facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()));
        let Some(value) = (index_kind == Some(NativeScalarKind::I64))
            .then(|| emit_dynamic_ptr_list_get(ir, id, instr.a(), instr.c(), tmp_index))
            .flatten()
        else {
            return false;
        };
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(value));
        return true;
    }
    if let NativeStraightlineValue::DynamicMapIter { id, key, value } = target {
        let index_kind = facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()));
        if index_kind != Some(NativeScalarKind::I64) {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicMapEntry {
            id,
            index_reg: instr.c(),
            key,
            value,
        });
        return true;
    }
    if let NativeStraightlineValue::DynamicMapEntry {
        id,
        index_reg,
        key,
        value,
    } = target
    {
        let Some(NativeStraightlineValue::I64(field)) = static_regs
            .get(instr.c() as usize)
            .and_then(Clone::clone)
            .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
        else {
            return false;
        };
        match field.as_str() {
            "0" => {
                match key {
                    NativeMapKeyKind::Str => {
                        let Some(key) = emit_dynamic_string_int_map_iter_key(
                            ir,
                            extra_globals,
                            id,
                            instr.a(),
                            index_reg,
                            tmp_index,
                        ) else {
                            return false;
                        };
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(key));
                    }
                    NativeMapKeyKind::I64 => {
                        if emit_dynamic_i64_int_map_iter_key(ir, id, instr.a(), index_reg, tmp_index).is_none() {
                            return false;
                        }
                        static_regs[instr.a() as usize] = None;
                    }
                };
            }
            "1" => {
                match value {
                    NativeMapValueKind::I64 => match key {
                        NativeMapKeyKind::Str => {
                            if emit_dynamic_string_int_map_iter_value(ir, id, instr.a(), index_reg, tmp_index).is_none()
                            {
                                return false;
                            }
                        }
                        NativeMapKeyKind::I64 => {
                            if emit_dynamic_i64_int_map_iter_value(ir, id, instr.a(), index_reg, tmp_index).is_none() {
                                return false;
                            }
                        }
                    },
                    NativeMapValueKind::F64 => {
                        if key != NativeMapKeyKind::Str {
                            return false;
                        }
                        if emit_dynamic_string_f64_map_iter_value(ir, id, instr.a(), index_reg, tmp_index).is_none() {
                            return false;
                        }
                    }
                }
                static_regs[instr.a() as usize] = None;
            }
            _ => return false,
        }
        return true;
    }
    if let NativeStraightlineValue::DynamicMap {
        id,
        key: NativeMapKeyKind::Str,
        value: NativeMapValueKind::I64,
    } = target
    {
        let Some(key) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
            return false;
        };
        if emit_dynamic_string_int_map_get(ir, extra_globals, id, instr.a(), key, tmp_index).is_none() {
            return false;
        }
        static_regs[instr.a() as usize] = None;
        return true;
    }
    if let NativeStraightlineValue::DynamicMap {
        id,
        key: NativeMapKeyKind::I64,
        value: NativeMapValueKind::I64,
    } = target
    {
        let key_kind = facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()));
        if !matches!(key_kind, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)) {
            return false;
        }
        if emit_dynamic_i64_int_map_get(ir, id, instr.a(), instr.c(), tmp_index).is_none() {
            return false;
        }
        static_regs[instr.a() as usize] = None;
        return true;
    }
    if let NativeStraightlineValue::DynamicMap {
        id,
        key: NativeMapKeyKind::Str,
        value: NativeMapValueKind::F64,
    } = target
    {
        let Some(key) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
            return false;
        };
        if emit_dynamic_string_f64_map_get(ir, extra_globals, id, instr.a(), key, tmp_index).is_none() {
            return false;
        }
        static_regs[instr.a() as usize] = None;
        return true;
    }
    if let NativeStraightlineValue::ArgList { elements } = &target {
        let key = static_regs.get(instr.c() as usize).and_then(Clone::clone).or_else(|| {
            (!register_written_by_enclosing_loop(code, pc, instr.c()))
                .then(|| local_static_i64_before(code, int_consts, pc, instr.c()))
                .flatten()
        });
        if let Some(NativeStraightlineValue::I64(key)) = key {
            let Some(index) = key.parse::<usize>().ok() else {
                return false;
            };
            let Some(value) = elements.get(index).cloned() else {
                return false;
            };
            return store_index_value(ir, extra_globals, static_regs, instr.a(), value, tmp_index).is_some();
        }
        if facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()))
            != Some(NativeScalarKind::I64)
        {
            return false;
        }
        let index = next_tmp(tmp_index);
        ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", instr.c()));
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicArgListElement {
            elements: elements.clone(),
            index,
        });
        return true;
    }
    if let NativeStraightlineValue::List { .. } = &target
        && let Some(key) = static_regs
            .get(instr.c() as usize)
            .and_then(Clone::clone)
            .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
        && let Some(value) = native_static_index(target.clone(), key, String::new())
        && !register_written_by_enclosing_loop(code, pc, instr.c())
    {
        return store_index_value(ir, extra_globals, static_regs, instr.a(), value, tmp_index).is_some();
    }
    if let NativeStraightlineValue::List { elements, .. } = &target
        && (local_static_index_key_before(static_regs, code, int_consts, strings, heap_values, pc, instr.c()).is_none()
            || register_written_by_enclosing_loop(code, pc, instr.c()))
    {
        return emit_dynamic_const_list_index(ir, extra_globals, static_regs, elements, pc, instr, tmp_index);
    }
    if let NativeStraightlineValue::DynamicConstListElement { elements, index } = &target {
        let trusted_static_key = static_register_value_trusted_before(code, pc, instr.c())
            .then(|| {
                static_regs
                    .get(instr.c() as usize)
                    .and_then(Clone::clone)
                    .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
            })
            .flatten();
        if let Some(NativeStraightlineValue::I64(key)) = trusted_static_key {
            let Some(key) = key.parse::<usize>().ok() else {
                return false;
            };
            let Some(value) =
                emit_const_list_element_index(ir, extra_globals, elements, index, key, instr.a(), pc, tmp_index)
            else {
                return false;
            };
            static_regs[instr.a() as usize] = Some(value);
            return true;
        }
        if facts
            .register_kind_before(pc, instr.c())
            .or_else(|| local_register_kind_before(code, pc, instr.c()))
            != Some(NativeScalarKind::I64)
        {
            return false;
        }
        let inner_index = next_tmp(tmp_index);
        ir.push_str(&format!("  {inner_index} = load i64, ptr %r{}.slot\n", instr.c()));
        let Some(value) = emit_const_list_element_dynamic_index(
            ir,
            extra_globals,
            elements,
            index,
            &inner_index,
            instr.a(),
            pc,
            tmp_index,
        ) else {
            return false;
        };
        static_regs[instr.a() as usize] = Some(value);
        return true;
    }
    if let NativeStraightlineValue::DynamicArgListElement { elements, index } = &target {
        let Some(NativeStraightlineValue::I64(key)) = static_regs
            .get(instr.c() as usize)
            .and_then(Clone::clone)
            .or_else(|| local_static_i64_before(code, int_consts, pc, instr.c()))
        else {
            return false;
        };
        let Some(values) = elements
            .iter()
            .map(|value| native_static_index(value.clone(), NativeStraightlineValue::I64(key.clone()), String::new()))
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        return emit_dynamic_arg_list_value_select(
            ir,
            extra_globals,
            static_regs,
            instr.a(),
            values,
            index,
            pc,
            tmp_index,
        );
    }
    if matches!(target, NativeStraightlineValue::I64(_)) {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64("0".to_string()));
        return true;
    }
    if let NativeStraightlineValue::Map { entries, .. } = &target
        && (facts.register_kind_before(pc, instr.c()) == Some(NativeScalarKind::StrPtr)
            || !static_string_value_trusted_at_call(code, pc, instr.c()))
        && static_string_i64_map_supported(entries)
    {
        return emit_string_i64_map_index(
            ir,
            extra_globals,
            static_regs,
            entries,
            code,
            pc,
            instr,
            facts,
            target.clone(),
            tmp_index,
        );
    }
    let trusted_key = static_register_value_trusted_before(code, pc, instr.c());
    let Some(key) = static_regs
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| {
            trusted_key
                .then(|| local_static_i64_value_before(code, int_consts, strings, heap_values, pc, instr.c()))
                .flatten()
        })
        .or_else(|| {
            trusted_key
                .then(|| local_static_i64_before(code, int_consts, pc, instr.c()))
                .flatten()
        })
        .or_else(|| {
            trusted_key
                .then(|| local_static_heap_const_before(code, heap_values, pc, instr.c()))
                .flatten()
        })
    else {
        return false;
    };
    let Some(value) = native_static_index(target, key, String::new()) else {
        return false;
    };
    if store_index_value(ir, extra_globals, static_regs, instr.a(), value, tmp_index).is_none() {
        return false;
    }
    true
}

fn emit_dynamic_arg_list_value_select(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    dst: u8,
    values: Vec<NativeStraightlineValue>,
    index: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> bool {
    if values.is_empty() {
        static_regs[dst as usize] = Some(NativeStraightlineValue::Nil);
        return true;
    }
    if values
        .iter()
        .all(|value| matches!(value, NativeStraightlineValue::I64(_)))
    {
        let mut selected = "0".to_string();
        for (idx, value) in values.into_iter().enumerate() {
            let NativeStraightlineValue::I64(value) = value else {
                return false;
            };
            let cmp = next_tmp(tmp_index);
            let next = next_tmp(tmp_index);
            ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
            ir.push_str(&format!("  {next} = select i1 {cmp}, i64 {value}, i64 {selected}\n"));
            selected = next;
        }
        ir.push_str(&format!("  store i64 {selected}, ptr %r{dst}.slot\n"));
        static_regs[dst as usize] = None;
        return true;
    }
    let mut selected = "@lk_nil_text".to_string();
    for (idx, value) in values.into_iter().enumerate() {
        let display = native_static_arg_list_display(&NativeStraightlineValue::ArgList { elements: vec![value] })
            .and_then(|display| display.strip_prefix('[')?.strip_suffix(']').map(str::to_string));
        let Some(display) = display else {
            return false;
        };
        let symbol = format!("@lk_arg_list_select_{pc}_{idx}");
        extra_globals.push_str(&llvm_string_constant(&symbol, &display));
        let cmp = next_tmp(tmp_index);
        let next = next_tmp(tmp_index);
        ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
        ir.push_str(&format!("  {next} = select i1 {cmp}, ptr {symbol}, ptr {selected}\n"));
        selected = next;
    }
    ir.push_str(&format!("  store ptr {selected}, ptr %r{dst}.slot\n"));
    static_regs[dst as usize] = Some(NativeStraightlineValue::StringPtr(selected));
    true
}

fn register_written_by_enclosing_loop(code: &[Instr32], pc_limit: usize, reg: u8) -> bool {
    code.iter()
        .copied()
        .enumerate()
        .skip(pc_limit.saturating_add(1))
        .filter(|(_, instr)| instr.opcode() == crate::vm::Opcode32::Jmp)
        .any(|(jump_pc, instr)| {
            let target = jump_pc as i64 + 1 + instr.sj_arg() as i64;
            target >= 0 && (target as usize) <= pc_limit && (target as usize..jump_pc).any(|pc| code[pc].a() == reg)
        })
}

fn emit_text_index(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    facts: &NativeScalarFacts,
    target: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> bool {
    let index_kind = facts
        .register_kind_before(pc, instr.c())
        .or_else(|| local_register_kind_before(code, pc, instr.c()));
    if index_kind != Some(NativeScalarKind::I64) {
        return false;
    }
    if let NativeStraightlineValue::String { value, .. } = target {
        let index = next_tmp(tmp_index);
        ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", instr.c()));
        let mut selected = "@lk_empty_text".to_string();
        for (idx, ch) in value.chars().enumerate() {
            let symbol = format!("@lk_string_index_{pc}_{idx}");
            extra_globals.push_str(&llvm_string_constant(&symbol, &ch.to_string()));
            let cmp = next_tmp(tmp_index);
            let next = next_tmp(tmp_index);
            ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
            ir.push_str(&format!("  {next} = select i1 {cmp}, ptr {symbol}, ptr {selected}\n"));
            selected = next;
        }
        ir.push_str(&format!("  store ptr {selected}, ptr %r{}.slot\n", instr.a()));
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(selected));
    } else {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicTextChar);
    }
    true
}

fn emit_dynamic_const_list_index(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    elements: &[ConstRuntimeValue32Data],
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> bool {
    let index = next_tmp(tmp_index);
    ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", instr.c()));
    if let Some(values) = elements
        .iter()
        .map(|value| match value {
            ConstRuntimeValue32Data::Int(value) => Some(*value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    {
        let mut selected = "0".to_string();
        for (idx, value) in values.into_iter().enumerate() {
            let cmp = next_tmp(tmp_index);
            let next = next_tmp(tmp_index);
            ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
            ir.push_str(&format!("  {next} = select i1 {cmp}, i64 {value}, i64 {selected}\n"));
            selected = next;
        }
        ir.push_str(&format!("  store i64 {selected}, ptr %r{}.slot\n", instr.a()));
        static_regs[instr.a() as usize] = None;
        return true;
    }
    if let Some(values) = elements
        .iter()
        .cloned()
        .map(native_const_runtime_string)
        .collect::<Option<Vec<_>>>()
    {
        emit_string_list_select(ir, extra_globals, static_regs, values, &index, pc, instr, tmp_index);
        return true;
    }
    if static_const_list_elements(elements).is_some() {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicConstListElement {
            elements: elements.to_vec(),
            index,
        });
        return true;
    }
    false
}

fn emit_local_dynamic_const_list_element_index(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    let Some((elements, outer_index_reg)) =
        local_dynamic_const_list_element_source_before(code, heap_values, pc, instr.b())
    else {
        return false;
    };
    let outer_index = next_tmp(tmp_index);
    ir.push_str(&format!("  {outer_index} = load i64, ptr %r{outer_index_reg}.slot\n"));

    if let Some(NativeStraightlineValue::I64(field)) = local_static_i64_before(code, int_consts, pc, instr.c()) {
        let Some(field) = field.parse::<usize>().ok() else {
            return false;
        };
        let Some(value) = emit_const_list_element_index(
            ir,
            extra_globals,
            &elements,
            &outer_index,
            field,
            instr.a(),
            pc,
            tmp_index,
        ) else {
            return false;
        };
        static_regs[instr.a() as usize] = Some(value);
        return true;
    }

    if facts
        .register_kind_before(pc, instr.c())
        .or_else(|| local_register_kind_before(code, pc, instr.c()))
        != Some(NativeScalarKind::I64)
    {
        return false;
    }
    let inner_index = next_tmp(tmp_index);
    ir.push_str(&format!("  {inner_index} = load i64, ptr %r{}.slot\n", instr.c()));
    let Some(value) = emit_const_list_element_dynamic_index(
        ir,
        extra_globals,
        &elements,
        &outer_index,
        &inner_index,
        instr.a(),
        pc,
        tmp_index,
    ) else {
        return false;
    };
    static_regs[instr.a() as usize] = Some(value);
    true
}

fn local_dynamic_const_list_element_source_before(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<(Vec<ConstRuntimeValue32Data>, u8)> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::GetIndex => {
                let Some(NativeStraightlineValue::List { elements, .. }) =
                    local_static_container_before(code, heap_values, prev_pc, prev.b())
                else {
                    return None;
                };
                Some((elements, prev.c()))
            }
            Opcode32::Move | Opcode32::ToIter if prev.b() != reg => {
                local_dynamic_const_list_element_source_before(code, heap_values, prev_pc, prev.b())
            }
            _ => None,
        };
    }
    None
}

fn emit_string_list_select(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    values: Vec<String>,
    index: &str,
    pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) {
    let mut selected = "@lk_empty_text".to_string();
    for (idx, value) in values.into_iter().enumerate() {
        let symbol = format!("@lk_list_get_{pc}_{idx}");
        extra_globals.push_str(&llvm_string_constant(&symbol, &value));
        let cmp = next_tmp(tmp_index);
        let next = next_tmp(tmp_index);
        ir.push_str(&format!("  {cmp} = icmp eq i64 {index}, {idx}\n"));
        ir.push_str(&format!("  {next} = select i1 {cmp}, ptr {symbol}, ptr {selected}\n"));
        selected = next;
    }
    ir.push_str(&format!("  store ptr {selected}, ptr %r{}.slot\n", instr.a()));
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(selected));
}

fn emit_string_i64_map_index(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    entries: &[(crate::vm::RuntimeMapKeyData, ConstRuntimeValue32Data)],
    _code: &[Instr32],
    _pc: usize,
    instr: Instr32,
    _facts: &NativeScalarFacts,
    target: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> bool {
    if let Some(key) = static_regs.get(instr.c() as usize).and_then(Clone::clone)
        && let Some(value) = native_static_index(target, key, String::new())
    {
        if store_index_value(ir, extra_globals, static_regs, instr.a(), value, tmp_index).is_none() {
            return false;
        }
        return true;
    }
    if emit_static_string_i64_map_get(ir, extra_globals, entries, "", instr.a(), instr.c(), tmp_index).is_none() {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    true
}

fn store_index_value(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    dst: u8,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    if store_native_scalar_call_result(ir, extra_globals, static_regs, dst, value.clone(), tmp_index).is_some() {
        return Some(());
    }
    match value {
        NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. } => {
            static_regs[dst as usize] = Some(value);
            Some(())
        }
        _ => None,
    }
}
