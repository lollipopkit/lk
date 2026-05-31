mod allocas; mod arithmetic; mod asserts; mod callees; mod channel; mod compare; mod const_lists; mod control;
mod finalize; mod get_index; mod object_methods; mod runtime_builtins; mod set_index; mod values;
use self::{allocas::emit_scalar_entry_allocas, arithmetic::emit_int_arithmetic_block, asserts::emit_native_assert_direct_call, callees::{callee_contains_call, callee_is_native_assert}, channel::emit_static_channel_call, compare::emit_compare_block, const_lists::emit_const_list_element_len, control::emit_test_block, finalize::finish_scalar_ir, get_index::emit_get_index_block, object_methods::{static_circle_pi_area_method, static_object_list_map_method}, runtime_builtins::emit_runtime_builtin_call, set_index::emit_set_index_block, values::emit_value_block};
use super::{
    block_helpers::{
        clear_control_flow_static_values, control_flow_static_boundaries, emit_dynamic_string_starts_with,
        emit_native_block_core_call_method, emit_static_direct_call_result, emit_static_formatted_print,
        emit_static_named_call, emit_static_scalar_value_store_if_needed, local_register_kind_before,
        local_static_container_before, local_static_i64_before, native_static_string, scalar_arg_value,
        static_call_args, static_call_target, static_callable_value, static_string_value_trusted_at_call,
        store_native_scalar_call_result, text_value_from_reg, three_regs_in_bounds,
    },
    contains::{
        emit_static_contains_or_slice_block, emit_static_to_iter_block, emit_static_type_test_block,
        local_static_callable_before, local_static_index_value_before, local_static_iter_zip_before,
        local_static_map_rest_before, local_static_string_before, static_int_list_chunk_method,
        static_int_list_filter_map_method, static_int_list_reduce_method, static_int_list_single_arg_method,
        static_int_list_zip_method, static_int_range_from_registers, static_iter_builtin_call,
        static_list_empty_arg_method, static_object_from_registers,
    },
    emit::emit_native_return_print,
    facts::{NativeScalarFacts, NativeScalarKind},
    inline::{emit_inline_direct_scalar_call, emit_inline_static_scalar_call},
};
use crate::llvm::{
    callee_eval::native_straightline_function_return,
    const_display::llvm_string_constant,
    dynamic_containers::{emit_dynamic_int_list_push, emit_dynamic_joined_text_len, emit_dynamic_string_int_map_get, emit_dynamic_text_len, emit_dynamic_text_list_push, emit_dynamic_text_list_push_len},
    ir_text::{emit_branch_to_next, native_label, native_relative_target, next_tmp, reg_in_bounds},
    map_mutate::native_static_map_mutate,
    options::LlvmBackendOptions,
    output::{emit_native_builtin_call, emit_native_dynamic_int_list_get_method},
    straightline_value::{NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue, native_const_runtime_value, native_static_global, native_static_list_join, native_static_list_push, native_static_load_cell, native_static_store_cell, native_static_string_split, native_straightline_heap_const_value},
    subfunction::compile_native_scalar_subfunction,
};
use crate::vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Module32Artifact, Opcode32};
pub(in crate::llvm) fn compile_native_scalar_main_blocks(
    artifact: &Module32Artifact,
    options: &LlvmBackendOptions,
    register_count: usize,
    global_count: usize,
    global_names: &[String],
    int_consts: &[i64],
    float_consts: &[f64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    facts: &NativeScalarFacts,
    recursive_indices: &[u16],
) -> anyhow::Result<Option<String>> {
    let Some(mut ir) = emit_scalar_entry_allocas(artifact, options, register_count, global_count, heap_values, code)
    else {
        return Ok(None);
    };
    let mut additional_subfn_indices: Vec<u16> = Vec::new();
    ir.push_str("  br label %bb0\n\n");
    let mut tmp_index = 0usize;
    let mut extra_globals = String::new();
    let mut static_regs: Vec<Option<NativeStraightlineValue>> = vec![None; register_count];
    let mut static_globals: Vec<Option<NativeStraightlineValue>> = vec![None; global_count];
    let static_boundaries = control_flow_static_boundaries(code);
    let mut skip_static_pcs = vec![false; code.len()];
    for (pc, instr) in code.iter().copied().enumerate() {
        if skip_static_pcs.get(pc).copied().unwrap_or(false) {
            ir.push_str(&format!("bb{pc}:\n"));
            emit_branch_to_next(&mut ir, pc, code.len());
            ir.push('\n');
            continue;
        }
        if static_boundaries.get(pc).copied().unwrap_or(false) {
            clear_control_flow_static_values(&mut static_regs);
        }
        ir.push_str(&format!("bb{pc}:\n"));
        match instr.opcode() {
            Opcode32::LoadString => {
                let Some(value) = strings.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let symbol = format!("@lk_block_str_{pc}");
                extra_globals.push_str(&llvm_string_constant(&symbol, value));
                static_regs[instr.a() as usize] = Some(native_static_string(value, symbol.clone()));
                ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadHeapConst => {
                let Some(value) = heap_values.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                if let ConstHeapValue32Data::LongString(value) = value {
                    let symbol = format!("@lk_block_heap_str_{pc}");
                    extra_globals.push_str(&llvm_string_constant(&symbol, value));
                    static_regs[instr.a() as usize] = Some(native_static_string(value, symbol.clone()));
                    ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
                } else {
                    if matches!(value, ConstHeapValue32Data::List(values) if values.is_empty()) {
                        ir.push_str(&format!("  store i64 0, ptr %list{pc}.len.slot\n"));
                        ir.push_str(&format!("  store i64 0, ptr %list{pc}.text.len.slot\n"));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                            id: pc,
                            element: NativeListElementKind::I64,
                        });
                    } else if let ConstHeapValue32Data::List(values) = value
                        && !values.is_empty()
                        && values.iter().all(|v| matches!(v, ConstRuntimeValue32Data::Int(_)))
                    {
                        let n = values.len();
                        ir.push_str(&format!("  store i64 {n}, ptr %list{pc}.len.slot\n"));
                        ir.push_str(&format!("  store i64 0, ptr %list{pc}.text.len.slot\n"));
                        for (i, v) in values.iter().enumerate() {
                            let ConstRuntimeValue32Data::Int(int_val) = v else {
                                unreachable!()
                            };
                            let slot = next_tmp(&mut tmp_index);
                            ir.push_str(&format!(
                                "  {slot} = getelementptr [4096 x i64], ptr %list{pc}.value.slots, i64 0, i64 {i}\n"
                            ));
                            ir.push_str(&format!("  store i64 {int_val}, ptr {slot}\n"));
                        }
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                            id: pc,
                            element: NativeListElementKind::I64,
                        });
                    } else if matches!(value, ConstHeapValue32Data::Map(values) if values.is_empty()) {
                        ir.push_str(&format!("  store i64 0, ptr %map{pc}.len.slot\n"));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicMap {
                            id: pc,
                            key: NativeMapKeyKind::Str,
                            value: NativeMapValueKind::I64,
                        });
                    } else {
                        let Some(value) = native_straightline_heap_const_value(0, instr.bx(), value) else {
                            return Ok(None);
                        };
                        static_regs[instr.a() as usize] = Some(value);
                    }
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadFunction | Opcode32::MakeClosure => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let Some(value) = static_callable_value(&artifact.module.functions, instr, &static_regs) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::NewList
            | Opcode32::LoadNil
            | Opcode32::LoadInt
            | Opcode32::LoadFloat
            | Opcode32::LoadBool
            | Opcode32::Move
            | Opcode32::ToString
            | Opcode32::ConcatString
            | Opcode32::AddFloat
            | Opcode32::SubFloat
            | Opcode32::MulFloat
            | Opcode32::DivFloat
            | Opcode32::ModFloat => {
                if !emit_value_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    global_names,
                    int_consts,
                    float_consts,
                    strings,
                    heap_values,
                    code,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
                if !emit_int_arithmetic_block(
                    &mut ir,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut static_regs,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt => {
                if !emit_compare_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode32::Test => {
                if !emit_test_block(
                    &mut ir,
                    &mut skip_static_pcs,
                    &static_boundaries,
                    &static_regs,
                    code,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode32::Not => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(kind) = facts
                    .register_kind_before(pc, instr.b())
                    .or_else(|| local_register_kind_before(code, pc, instr.b()))
                else {
                    return Ok(None);
                };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        let out = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                        ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                        ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
                        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeScalarKind::I64
                    | NativeScalarKind::F64
                    | NativeScalarKind::StrPtr
                    | NativeScalarKind::MaybeI64 => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        let out = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                        ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                        ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
                        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                    }
                }
                static_regs[instr.a() as usize] = None;
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::IsNil | Opcode32::IsList | Opcode32::IsMap => {
                if emit_static_type_test_block(
                    &mut ir,
                    &mut static_regs,
                    register_count,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    facts,
                    pc,
                    instr,
                )
                .is_none()
                {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len()) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
            }
            Opcode32::GetGlobal => {
                if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
                    return Ok(None);
                }
                if let Some(value) = global_names
                    .get(instr.bx() as usize)
                    .and_then(|name| native_static_global(name))
                {
                    if store_native_scalar_call_result(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr.a(),
                        value.clone(),
                        &mut tmp_index,
                    )
                    .is_none()
                    {
                        static_regs[instr.a() as usize] = Some(value);
                    }
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                if let Some(value) = static_globals.get(instr.bx() as usize).and_then(Clone::clone) {
                    if store_native_scalar_call_result(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr.a(),
                        value.clone(),
                        &mut tmp_index,
                    )
                    .is_none()
                    {
                        static_regs[instr.a() as usize] = Some(value);
                    }
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                let Some(kind) = facts.global_kind_before(pc, instr.bx()) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = None;
                let value = next_tmp(&mut tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!("  {value} = load {ty}, ptr %g{}.slot\n", instr.bx()));
                ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::SetGlobal => {
                if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
                    return Ok(None);
                }
                if let Some(value) = static_regs.get(instr.a() as usize).and_then(Clone::clone) {
                    static_globals[instr.bx() as usize] = Some(value);
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                let Some(kind) = facts.register_kind_before(pc, instr.a()) else {
                    return Ok(None);
                };
                static_globals[instr.bx() as usize] = None;
                static_regs[instr.a() as usize] = None;
                let value = next_tmp(&mut tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.a()));
                ir.push_str(&format!("  store {ty} {value}, ptr %g{}.slot\n", instr.bx()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Len => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let target = static_regs
                    .get(instr.b() as usize)
                    .and_then(Clone::clone)
                    .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
                    .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, instr.b()))
                    .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, instr.b()));
                let Some(target) = target else {
                    return Ok(None);
                };
                match target {
                    NativeStraightlineValue::String { value, .. } if value.is_ascii() => {
                        let len = value.len();
                        ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
                    }
                    NativeStraightlineValue::Text(parts) => {
                        if emit_dynamic_text_len(&mut ir, instr.a(), &parts, &mut tmp_index).is_none() {
                            return Ok(None);
                        }
                    }
                    NativeStraightlineValue::DynamicTextChar => {
                        ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeStraightlineValue::List { elements, .. } => {
                        let len = elements.len();
                        ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
                    }
                    NativeStraightlineValue::Map { entries, .. } => {
                        let len = entries.len();
                        ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
                    }
                    NativeStraightlineValue::DynamicJoinedText { id, delimiter_len } => {
                        if emit_dynamic_joined_text_len(&mut ir, instr.a(), id, delimiter_len, &mut tmp_index).is_none()
                        {
                            return Ok(None);
                        }
                    }
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::I64,
                    } => {
                        let len = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
                        ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeStraightlineValue::DynamicConstListElement { elements, index } => {
                        let Some(value) =
                            emit_const_list_element_len(&mut ir, &elements, &index, instr.a(), &mut tmp_index)
                        else {
                            return Ok(None);
                        };
                        static_regs[instr.a() as usize] = Some(value);
                    }
                    _ => return Ok(None),
                }
                if !matches!(
                    static_regs.get(instr.a() as usize),
                    Some(Some(NativeStraightlineValue::I64(_)))
                ) {
                    static_regs[instr.a() as usize] = None;
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::ToIter => {
                if emit_static_to_iter_block(
                    &mut static_regs,
                    register_count,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr,
                )
                .is_none()
                {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::StringSplit => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(target) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(delimiter) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if let Some(value) = native_static_string_split(target.clone(), delimiter.clone(), String::new()) {
                    static_regs[instr.a() as usize] = Some(value);
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                let (NativeStraightlineValue::Text(text), NativeStraightlineValue::String { value: delimiter, .. }) =
                    (target, delimiter)
                else {
                    return Ok(None);
                };
                if !delimiter.is_ascii() {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicSplitText { text, delimiter });
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::ListJoin => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(target) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(delimiter) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if let (
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::Text,
                    },
                    NativeStraightlineValue::String { value: delimiter, .. },
                ) = (&target, &delimiter)
                {
                    if !delimiter.is_ascii() {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicJoinedText {
                        id: *id,
                        delimiter_len: delimiter.len(),
                    });
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                if let Some(value) =
                    native_static_list_join(target.clone(), delimiter.clone(), format!("@lk_join_str_{pc}"))
                {
                    if let NativeStraightlineValue::String { value: text, .. } = &value {
                        extra_globals.push_str(&llvm_string_constant(&format!("@lk_join_str_{pc}"), text));
                    }
                    static_regs[instr.a() as usize] = Some(value);
                    emit_branch_to_next(&mut ir, pc, code.len());
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
                    return Ok(None);
                };
                if split_delimiter != join_delimiter {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Text(text));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::StringStartsWith => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(prefix) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let NativeStraightlineValue::String { value: prefix, .. } = prefix else {
                    return Ok(None);
                };
                if let Some(NativeStraightlineValue::String { value: target, .. }) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                    && static_string_value_trusted_at_call(code, pc, instr.b())
                {
                    let value = i64::from(target.starts_with(&prefix));
                    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(value.to_string()));
                } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
                    emit_dynamic_string_starts_with(
                        &mut ir,
                        &mut extra_globals,
                        "",
                        instr.a(),
                        instr.b(),
                        &prefix,
                        &mut tmp_index,
                    );
                    static_regs[instr.a() as usize] = None;
                } else {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::GetIndex => {
                if !emit_get_index_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode32::ListPush => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let target = static_regs
                    .get(instr.a() as usize)
                    .and_then(Clone::clone)
                    .or_else(|| local_static_container_before(code, heap_values, pc, instr.a()));
                let Some(NativeStraightlineValue::DynamicList {
                    id,
                    element: NativeListElementKind::I64,
                }) = target.clone()
                else {
                    if let Some(target) = target.clone()
                        && let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone)
                        && let Some(value) = native_static_list_push(target, value)
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let Some(NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::Text,
                    }) = target
                    {
                        let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                            return Ok(None);
                        };
                        if emit_dynamic_text_list_push(&mut ir, id, value, &mut tmp_index).is_none() {
                            return Ok(None);
                        }
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                            id,
                            element: NativeListElementKind::Text,
                        });
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    return Ok(None);
                };
                if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::I64)
                    && !matches!(
                        static_regs.get(instr.b() as usize).and_then(Clone::clone),
                        Some(
                            NativeStraightlineValue::DynamicTextChar
                                | NativeStraightlineValue::Text(_)
                                | NativeStraightlineValue::String { .. }
                                | NativeStraightlineValue::StringPtr(_)
                        )
                    )
                {
                    if emit_dynamic_int_list_push(&mut ir, id, instr.b(), &mut tmp_index).is_none() {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::I64,
                    });
                } else {
                    let Some(value) = text_value_from_reg(
                        &mut ir,
                        instr.b(),
                        facts.register_kind_before(pc, instr.b()),
                        &static_regs,
                        &mut tmp_index,
                    ) else {
                        return Ok(None);
                    };
                    if emit_dynamic_text_list_push(&mut ir, id, value, &mut tmp_index).is_none() {
                        if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
                            emit_dynamic_text_list_push_len(&mut ir, id, "1", &mut tmp_index);
                        } else {
                            return Ok(None);
                        }
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::Text,
                    });
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::NewObject => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let Some(value) =
                    static_object_from_registers(&static_regs, code, int_consts, pc, instr, String::new())
                else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::NewRange => {
                let Some(value) =
                    static_int_range_from_registers(&static_regs, code, int_consts, pc, instr, String::new())
                else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Contains | Opcode32::SliceFrom | Opcode32::MapRest => {
                if emit_static_contains_or_slice_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    register_count,
                    code,
                    int_consts,
                    heap_values,
                    pc,
                    instr,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::SetIndex => {
                if !emit_set_index_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    register_count,
                    pc,
                    instr,
                    code.len(),
                    facts,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode32::Call => {
                if instr.a() != instr.b() || !reg_in_bounds(register_count, instr.a()) {}
                if let Some(target @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. })) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                {
                    let Some((function_index, captures)) = static_call_target(target) else {
                        return Ok(None);
                    };
                    let Ok(function_index) = u8::try_from(function_index) else {
                        return Ok(None);
                    };
                    let Some(callee) = artifact.module.functions.get(function_index as usize) else {
                        return Ok(None);
                    };
                    let direct_instr = Instr32::abc(Opcode32::CallDirect, instr.a(), function_index, instr.c());
                    let call_arg_start = instr.b() as usize + 1;
                    let call_arg_end = call_arg_start.checked_add(instr.c() as usize).unwrap_or(usize::MAX);
                    if let Some(args) = (call_arg_start..call_arg_end)
                        .map(|reg| {
                            static_regs
                                .get(reg)
                                .cloned()
                                .flatten()
                                .or_else(|| {
                                    local_static_callable_before(
                                        &artifact.module.functions,
                                        code,
                                        pc,
                                        reg as u8,
                                        &static_regs,
                                    )
                                })
                                .or_else(|| local_static_i64_before(code, int_consts, pc, reg as u8))
                        })
                        .collect::<Option<Vec<_>>>()
                        && let Some(value) = native_straightline_function_return(
                            artifact,
                            function_index as usize,
                            &args,
                            &captures,
                            &mut static_globals,
                            0,
                            &mut ir,
                            &mut tmp_index,
                        )
                        .ok()
                        .flatten()
                    {
                        store_native_scalar_call_result(
                            &mut ir,
                            &mut extra_globals,
                            &mut static_regs,
                            instr.a(),
                            value,
                            &mut tmp_index,
                        );
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_inline_static_scalar_call(
                        &mut ir,
                        &mut extra_globals,
                        artifact,
                        callee,
                        pc,
                        direct_instr,
                        register_count,
                        global_count,
                        global_names,
                        code,
                        &static_regs,
                        &static_globals,
                        facts,
                        &captures,
                        &mut tmp_index,
                        code.len(),
                    )
                    .is_none()
                    {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = None;
                    continue;
                }
                let Some(NativeStraightlineValue::Builtin(builtin)) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                if matches!(builtin, NativeBuiltin::Send | NativeBuiltin::Recv)
                    && emit_static_channel_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        code,
                        int_consts,
                        pc,
                        instr,
                        builtin,
                        &mut tmp_index,
                    )
                    .is_some()
                {
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                if let Some(args) = static_call_args(&static_regs, instr.b(), instr.c()) {
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        callable @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }),
                    ] = args.as_slice()
                        && let Some(value) = static_object_list_map_method(
                            artifact,
                            target.clone(),
                            method,
                            callable.clone(),
                            &mut static_globals,
                            &mut ir,
                            &mut tmp_index,
                        )
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        callable @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }),
                    ] = args.as_slice()
                        && let Some(value) = static_int_list_filter_map_method(
                            artifact,
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            target.clone(),
                            method,
                            callable.clone(),
                            &mut static_globals,
                            &mut ir,
                            &mut tmp_index,
                        )
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::ArgList { elements },
                    ] = args.as_slice()
                        && method == "reduce"
                        && let Some(value) = static_int_list_reduce_method(
                            artifact,
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            target.clone(),
                            elements,
                            &mut static_globals,
                            &mut ir,
                            &mut tmp_index,
                        )
                    {
                        emit_static_scalar_value_store_if_needed(&mut ir, instr.a(), &value);
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [target, NativeStraightlineValue::String { value: method, .. }, size] = args.as_slice()
                        && method == "chunk"
                        && let Some(value) = static_int_list_chunk_method(
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            target.clone(),
                            size.clone(),
                        )
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::List { elements, .. },
                    ] = args.as_slice()
                        && method == "zip"
                        && let Some(value) =
                            static_int_list_zip_method(code, int_consts, strings, heap_values, target.clone(), elements)
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::List { elements, .. },
                    ] = args.as_slice()
                        && let Some(value) = static_int_list_single_arg_method(
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            target.clone(),
                            method,
                            elements,
                        )
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::List { elements, .. },
                    ] = args.as_slice()
                        && elements.is_empty()
                        && let Some(value) =
                            static_list_empty_arg_method(code, int_consts, strings, heap_values, target.clone(), method)
                    {
                        static_regs[instr.a() as usize] = Some(value);
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        NativeStraightlineValue::DynamicMap {
                            id,
                            key: NativeMapKeyKind::Str,
                            value: NativeMapValueKind::I64,
                        },
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::List { elements, .. },
                    ] = args.as_slice()
                        && method == "get"
                        && elements.len() == 1
                    {
                        let Some(key) = native_const_runtime_value(&elements[0], String::new()) else {
                            return Ok(None);
                        };
                        if emit_dynamic_string_int_map_get(
                            &mut ir,
                            &mut extra_globals,
                            *id,
                            instr.a(),
                            key,
                            &mut tmp_index,
                        )
                        .is_none()
                        {
                            return Ok(None);
                        }
                        static_regs[instr.a() as usize] = None;
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        NativeStraightlineValue::DynamicMap {
                            id,
                            key: NativeMapKeyKind::Str,
                            value: NativeMapValueKind::I64,
                        },
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::ArgList { elements },
                    ] = args.as_slice()
                        && method == "get"
                        && elements.len() == 1
                    {
                        if emit_dynamic_string_int_map_get(
                            &mut ir,
                            &mut extra_globals,
                            *id,
                            instr.a(),
                            elements[0].clone(),
                            &mut tmp_index,
                        )
                        .is_none()
                        {
                            return Ok(None);
                        }
                        static_regs[instr.a() as usize] = None;
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        NativeStraightlineValue::DynamicList {
                            id,
                            element: NativeListElementKind::I64,
                        },
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::List { elements, .. },
                    ] = args.as_slice()
                        && method == "get"
                        && elements.len() == 1
                        && let ConstRuntimeValue32Data::Int(index) = elements[0]
                    {
                        emit_native_dynamic_int_list_get_method(
                            &mut ir,
                            *id,
                            &index.to_string(),
                            instr.a(),
                            &mut tmp_index,
                        );
                        static_regs[instr.a() as usize] = None;
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let [
                        NativeStraightlineValue::DynamicList {
                            id,
                            element: NativeListElementKind::I64,
                        },
                        NativeStraightlineValue::String { value: method, .. },
                        NativeStraightlineValue::DynamicList {
                            id: arg_id,
                            element: NativeListElementKind::I64,
                        },
                    ] = args.as_slice()
                        && method == "get"
                    {
                        let slot = next_tmp(&mut tmp_index);
                        let index = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {slot} = getelementptr [4096 x i64], ptr %list{arg_id}.value.slots, i64 0, i64 0\n  {index} = load i64, ptr {slot}\n"));
                        emit_native_dynamic_int_list_get_method(&mut ir, *id, &index, instr.a(), &mut tmp_index);
                        static_regs[instr.a() as usize] = None;
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    let value = if let Some(value) = static_iter_builtin_call(
                        artifact,
                        code,
                        int_consts,
                        strings,
                        heap_values,
                        builtin,
                        &args,
                        &mut static_globals,
                        &mut ir,
                        &mut tmp_index,
                    ) {
                        value
                    } else if builtin == NativeBuiltin::MapMutate {
                        let [target, callable] = args.as_slice() else {
                            return Ok(None);
                        };
                        let Some(value) = native_static_map_mutate(
                            &artifact.module.functions,
                            target.clone(),
                            callable.clone(),
                            format!("@lk_map_mutate_{pc}"),
                        ) else {
                            return Ok(None);
                        };
                        value
                    } else if let Some(value) = static_circle_pi_area_method(global_names, &args) {
                        value
                    } else if let Some(value) =
                        emit_native_block_core_call_method(&mut ir, &mut extra_globals, builtin, &args, &mut tmp_index)
                    {
                        value
                    } else if let Some(value) =
                        emit_static_formatted_print(&mut ir, &mut extra_globals, builtin, &args, &mut tmp_index)
                    {
                        value
                    } else if let Some(value) = emit_native_builtin_call(&mut ir, builtin, &args, &mut tmp_index) {
                        value
                    } else {
                        emit_runtime_builtin_call(&mut ir, builtin, instr, register_count, facts, pc, &mut tmp_index);
                        static_regs[instr.a() as usize] = None;
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    };
                    if store_native_scalar_call_result(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr.a(),
                        value,
                        &mut tmp_index,
                    )
                    .is_none()
                    {
                        return Ok(None);
                    }
                    emit_branch_to_next(&mut ir, pc, code.len());
                } else {
                    let static_args = (instr.b() as usize + 1..instr.b() as usize + 1 + instr.c() as usize)
                        .map(|reg| {
                            local_static_iter_zip_before(
                                global_names,
                                code,
                                int_consts,
                                strings,
                                heap_values,
                                pc,
                                u8::try_from(reg).ok()?,
                                &static_regs,
                            )
                            .or_else(|| static_regs.get(reg).cloned().flatten())
                            .or_else(|| {
                                let reg = u8::try_from(reg).ok()?;
                                static_string_value_trusted_at_call(code, pc, reg)
                                    .then(|| local_static_string_before(code, strings, pc, reg))
                                    .flatten()
                            })
                            .or_else(|| {
                                let reg = u8::try_from(reg).ok()?;
                                static_string_value_trusted_at_call(code, pc, reg)
                                    .then(|| local_static_i64_before(code, int_consts, pc, reg))
                                    .flatten()
                            })
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some(args) = static_args {
                        if let Some(value) = static_iter_builtin_call(
                            artifact,
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            builtin,
                            &args,
                            &mut static_globals,
                            &mut ir,
                            &mut tmp_index,
                        ) {
                            store_native_scalar_call_result(
                                &mut ir,
                                &mut extra_globals,
                                &mut static_regs,
                                instr.a(),
                                value,
                                &mut tmp_index,
                            );
                            emit_branch_to_next(&mut ir, pc, code.len());
                            continue;
                        }
                        if let Some(value) =
                            emit_static_formatted_print(&mut ir, &mut extra_globals, builtin, &args, &mut tmp_index)
                        {
                            store_native_scalar_call_result(
                                &mut ir,
                                &mut extra_globals,
                                &mut static_regs,
                                instr.a(),
                                value,
                                &mut tmp_index,
                            );
                            emit_branch_to_next(&mut ir, pc, code.len());
                            continue;
                        }
                        if let Some(value) = emit_native_builtin_call(&mut ir, builtin, &args, &mut tmp_index) {
                            store_native_scalar_call_result(
                                &mut ir,
                                &mut extra_globals,
                                &mut static_regs,
                                instr.a(),
                                value,
                                &mut tmp_index,
                            );
                            emit_branch_to_next(&mut ir, pc, code.len());
                            continue;
                        }
                    }
                    let scalar_args = (instr.b() as usize + 1..instr.b() as usize + 1 + instr.c() as usize)
                        .map(|reg| scalar_arg_value(&mut ir, "", facts, pc, &static_regs, reg, &mut tmp_index))
                        .collect::<Option<Vec<_>>>();
                    if let Some(args) = scalar_args.as_ref()
                        && let Some(value) = emit_native_builtin_call(&mut ir, builtin, args, &mut tmp_index)
                    {
                        store_native_scalar_call_result(
                            &mut ir,
                            &mut extra_globals,
                            &mut static_regs,
                            instr.a(),
                            value,
                            &mut tmp_index,
                        );
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if let Some(args) = scalar_args
                        && let Some(value) =
                            emit_static_formatted_print(&mut ir, &mut extra_globals, builtin, &args, &mut tmp_index)
                    {
                        store_native_scalar_call_result(
                            &mut ir,
                            &mut extra_globals,
                            &mut static_regs,
                            instr.a(),
                            value,
                            &mut tmp_index,
                        );
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if !emit_runtime_builtin_call(&mut ir, builtin, instr, register_count, facts, pc, &mut tmp_index) {
                        return Ok(None);
                    }
                    emit_branch_to_next(&mut ir, pc, code.len());
                }
            }
            Opcode32::CallNamed => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                if emit_static_named_call(
                    &mut ir,
                    &mut extra_globals,
                    artifact,
                    facts,
                    pc,
                    &mut static_regs,
                    &mut static_globals,
                    instr,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::CallDirect => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let callee_index = instr.b();
                if emit_static_direct_call_result(
                    &mut ir,
                    &mut extra_globals,
                    artifact,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    &mut static_regs,
                    &mut static_globals,
                    instr,
                    &mut tmp_index,
                )
                .is_some()
                {
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                let is_recursive = recursive_indices.contains(&u16::from(callee_index));
                if !is_recursive {
                    let Some(callee) = artifact.module.functions.get(callee_index as usize) else {
                        return Ok(None);
                    };
                    if callee_is_native_assert(callee) {
                        if emit_native_assert_direct_call(
                            &mut ir,
                            instr,
                            pc,
                            code.len(),
                            register_count,
                            facts,
                            &mut tmp_index,
                        )
                        .is_none()
                        {
                            return Ok(None);
                        }
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
                        continue;
                    }
                    let inline_result = if callee_contains_call(callee) {
                        None
                    } else {
                        emit_inline_direct_scalar_call(
                            &mut ir,
                            &mut extra_globals,
                            artifact,
                            callee,
                            pc,
                            instr,
                            register_count,
                            global_count,
                            global_names,
                            code,
                            &static_regs,
                            &static_globals,
                            facts,
                            &mut tmp_index,
                            code.len(),
                        )
                    };
                    if inline_result.is_none() {
                        let all_recursive: Vec<u16> = recursive_indices.to_vec();
                        if compile_native_scalar_subfunction(artifact, callee_index as usize, &all_recursive).is_err()
                            || compile_native_scalar_subfunction(artifact, callee_index as usize, &all_recursive)
                                .ok()
                                .flatten()
                                .is_none()
                        {
                            return Ok(None);
                        }
                        let return_kind = facts
                            .register_kind_before(pc + 1, instr.a())
                            .unwrap_or(NativeScalarKind::I64);
                        let return_ty = return_kind.llvm_type();
                        let mut call_args = String::new();
                        for i in 0..instr.c() as usize {
                            let arg_reg = instr.a() as usize + 1 + i;
                            if arg_reg >= register_count {
                                return Ok(None);
                            }
                            let arg_kind = facts
                                .register_kind_before(pc, arg_reg as u8)
                                .unwrap_or(NativeScalarKind::I64);
                            let arg_ty = arg_kind.llvm_type();
                            let arg_tmp = next_tmp(&mut tmp_index);
                            ir.push_str(&format!("  {arg_tmp} = load {arg_ty}, ptr %r{arg_reg}.slot\n"));
                            if i > 0 {
                                call_args.push_str(", ");
                            }
                            call_args.push_str(&format!("{arg_ty} {arg_tmp}"));
                        }
                        let result = next_tmp(&mut tmp_index);
                        ir.push_str(&format!(
                            "  {result} = call {return_ty} @lk_fn_{callee_index}({call_args})\n"
                        ));
                        ir.push_str(&format!("  store {return_ty} {result}, ptr %r{}.slot\n", instr.a()));
                        static_regs[instr.a() as usize] = None;
                        additional_subfn_indices.push(u16::from(callee_index));
                        emit_branch_to_next(&mut ir, pc, code.len());
                    }
                    static_regs[instr.a() as usize] = None;
                } else {
                    let all_recursive: Vec<u16> = recursive_indices.to_vec();
                    if compile_native_scalar_subfunction(artifact, callee_index as usize, &all_recursive).is_err()
                        || compile_native_scalar_subfunction(artifact, callee_index as usize, &all_recursive)
                            .ok()
                            .flatten()
                            .is_none()
                    {
                        return Ok(None);
                    }
                    let return_kind = facts
                        .register_kind_before(pc + 1, instr.a())
                        .unwrap_or(NativeScalarKind::I64);
                    let return_ty = return_kind.llvm_type();
                    let mut call_args = String::new();
                    for i in 0..instr.c() as usize {
                        let arg_reg = instr.a() as usize + 1 + i;
                        if arg_reg >= register_count {
                            return Ok(None);
                        }
                        let arg_kind = facts
                            .register_kind_before(pc, arg_reg as u8)
                            .unwrap_or(NativeScalarKind::I64);
                        let arg_ty = arg_kind.llvm_type();
                        let arg_tmp = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {arg_tmp} = load {arg_ty}, ptr %r{arg_reg}.slot\n"));
                        if i > 0 {
                            call_args.push_str(", ");
                        }
                        call_args.push_str(&format!("{arg_ty} {arg_tmp}"));
                    }
                    let result = next_tmp(&mut tmp_index);
                    ir.push_str(&format!(
                        "  {result} = call {return_ty} @lk_fn_{callee_index}({call_args})\n"
                    ));
                    ir.push_str(&format!("  store {return_ty} {result}, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = None;
                    emit_branch_to_next(&mut ir, pc, code.len());
                }
            }
            Opcode32::Return => {
                if instr.b() == 0 {
                    ir.push_str("  ret i32 0\n");
                } else if instr.b() == 1 && reg_in_bounds(register_count, instr.a()) {
                    let Some(kind) = facts
                        .register_kind_before(pc, instr.a())
                        .or_else(|| local_register_kind_before(code, pc, instr.a()))
                    else {
                        return Ok(None);
                    };
                    emit_native_return_print(&mut ir, pc, instr.a(), kind, &mut tmp_index);
                    ir.push_str("  ret i32 0\n");
                } else {
                    return Ok(None);
                }
            }
            Opcode32::StoreCellVal => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                if let Some(kind) = facts.register_kind_before(pc, instr.b()) {
                    let value = next_tmp(&mut tmp_index);
                    let ty = kind.llvm_type();
                    ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
                    ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
                } else {
                    ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
                }
                static_regs[instr.a() as usize] = if let (Some(cell), Some(value)) = (
                    static_regs.get(instr.a() as usize).and_then(Clone::clone),
                    static_regs
                        .get(instr.b() as usize)
                        .and_then(Clone::clone)
                        .or_else(|| local_static_i64_before(code, int_consts, pc, instr.b())),
                ) {
                    native_static_store_cell(cell, value)
                } else {
                    None
                };
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadCellVal => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                if let Some(kind) = facts.register_kind_before(pc, instr.b()) {
                    let value = next_tmp(&mut tmp_index);
                    let ty = kind.llvm_type();
                    ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
                    ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
                } else {
                    let value = next_tmp(&mut tmp_index);
                    ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                }
                static_regs[instr.a() as usize] = static_regs
                    .get(instr.b() as usize)
                    .and_then(Clone::clone)
                    .and_then(native_static_load_cell);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Nop | Opcode32::Raise => emit_branch_to_next(&mut ir, pc, code.len()),
            _ => return Ok(None),
        }
        ir.push('\n');
    }
    Ok(Some(finish_scalar_ir(
        artifact,
        ir,
        &extra_globals,
        recursive_indices,
        &additional_subfn_indices,
    )))
}
