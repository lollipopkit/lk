mod allocas;
mod arithmetic;
mod asserts;
mod call_args;
mod callees;
mod cells;
mod channel;
mod compare;
mod const_lists;
mod control;
mod direct_print;
mod finalize;
mod get_index;
mod globals;
mod i64_list_methods;
mod iter;
mod len;
mod list_builtin_dispatch;
mod list_direct_calls;
mod list_methods;
mod list_push;
mod map_methods;
mod not;
mod object_methods;
mod returns;
mod runtime_builtins;
mod set_index;
mod string_methods;
mod string_split;
mod values;
use self::{
    allocas::emit_scalar_entry_allocas,
    arithmetic::{emit_int_arithmetic_block, emit_int_immediate_block},
    asserts::emit_native_assert_direct_call,
    call_args::{
        emit_recovered_builtin_call_block, emit_runtime_formatted_print_call, static_or_recovered_call_args,
        static_or_recovered_call_target,
    },
    callees::{callee_contains_call, callee_is_native_assert},
    cells::{emit_load_cell_block, emit_store_cell_block},
    channel::emit_static_channel_call,
    compare::emit_compare_block,
    control::{emit_compare_test_block, emit_for_loop_i_block, emit_test_block},
    direct_print::emit_direct_emit_helper_call,
    finalize::finish_scalar_ir,
    get_index::{emit_get_field_k_block, emit_get_index_block},
    globals::{emit_get_global_block, emit_set_global_block},
    iter::emit_to_iter_block,
    len::emit_len_block,
    list_builtin_dispatch::{emit_dynamic_list_builtin_call_block, emit_dynamic_list_builtin_call_from_regs_block},
    list_direct_calls::emit_list_direct_call,
    list_methods::{
        emit_dynamic_list_method_call_block, emit_static_i64_list_zip_arglist_call, function_has_list_return_shape,
    },
    list_push::emit_list_push_block,
    map_methods::{
        emit_dynamic_map_delete_call, emit_dynamic_map_get_call, emit_dynamic_map_get_method_call,
        emit_dynamic_map_has_call, emit_dynamic_map_keys_call, emit_dynamic_map_set_call, emit_dynamic_map_values_call,
    },
    not::emit_not_block,
    object_methods::{static_circle_pi_area_method, static_object_list_map_method},
    returns::emit_return_block,
    runtime_builtins::emit_runtime_builtin_call,
    set_index::{emit_set_field_k_block, emit_set_index_block},
    string_methods::emit_string_starts_with_block,
    string_split::emit_string_split_block,
    values::emit_value_block,
};
use super::{
    block_helpers::{
        clear_control_flow_static_values, control_flow_static_boundaries, emit_native_block_core_call_method,
        emit_static_direct_call_result, emit_static_formatted_print, emit_static_named_call,
        emit_static_scalar_value_store_if_needed, local_register_kind_before, local_static_container_before,
        local_static_i64_before, native_static_string, static_call_target, static_callable_value,
        store_native_scalar_call_result, three_regs_in_bounds,
    },
    contains::{
        emit_static_contains_or_slice_block, emit_static_type_test_block, local_static_callable_before,
        local_static_heap_const_before, static_int_list_chunk_method, static_int_list_filter_map_method,
        static_int_list_reduce_method, static_int_list_single_arg_method, static_int_range_from_registers,
        static_iter_builtin_call, static_list_empty_arg_method, static_object_from_registers,
    },
    facts::{NativeScalarFacts, NativeScalarKind},
    inline::{emit_inline_direct_scalar_call, emit_inline_static_scalar_call},
};
use crate::llvm::{
    callee_eval::native_straightline_function_return,
    const_display::llvm_string_constant,
    ir_text::{emit_branch_to_next, native_label, native_relative_target, next_tmp, reg_in_bounds},
    map_mutate::native_static_map_mutate,
    options::LlvmBackendOptions,
    output::{emit_native_builtin_call, emit_native_dynamic_int_list_get_method},
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
        NativeTextPart, native_static_list_join, native_straightline_heap_const_value,
    },
    subfunction::compile_native_scalar_subfunction,
};
use crate::vm::{ConstHeapValueData, ConstRuntimeValueData, Instr, ModuleArtifact, Opcode};
pub(in crate::llvm) fn compile_native_scalar_main_blocks(
    artifact: &ModuleArtifact,
    options: &LlvmBackendOptions,
    register_count: usize,
    global_count: usize,
    global_names: &[String],
    int_consts: &[i64],
    float_consts: &[f64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    facts: &NativeScalarFacts,
    recursive_indices: &[u16],
) -> anyhow::Result<Option<String>> {
    let Some(mut ir) = emit_scalar_entry_allocas(artifact, options, register_count, global_count, heap_values, code)
    else {
        return Ok(None);
    };
    let Some(function) = artifact.module.functions.get(artifact.module.entry as usize) else {
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
    let trace = std::env::var_os("LK_NATIVE_BLOCK_TRACE").is_some();
    for (pc, instr) in code.iter().copied().enumerate() {
        if trace {
            eprintln!("native scalar block pc={pc:04} {}", instr.disassemble());
        }
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
            Opcode::LoadString => {
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
            Opcode::LoadHeapConst => {
                let Some(value) = heap_values.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                if let ConstHeapValueData::LongString(value) = value {
                    let symbol = format!("@lk_block_heap_str_{pc}");
                    extra_globals.push_str(&llvm_string_constant(&symbol, value));
                    static_regs[instr.a() as usize] = Some(native_static_string(value, symbol.clone()));
                    ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
                } else {
                    if matches!(value, ConstHeapValueData::List(values) if values.is_empty()) {
                        ir.push_str(&format!("  store i64 0, ptr %list{pc}.len.slot\n"));
                        ir.push_str(&format!("  store i64 0, ptr %list{pc}.text.len.slot\n"));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                            id: pc,
                            element: NativeListElementKind::I64,
                        });
                    } else if let ConstHeapValueData::List(values) = value
                        && !values.is_empty()
                        && values.iter().all(|v| matches!(v, ConstRuntimeValueData::Int(_)))
                    {
                        let n = values.len();
                        ir.push_str(&format!("  store i64 {n}, ptr %list{pc}.len.slot\n"));
                        ir.push_str(&format!("  store i64 0, ptr %list{pc}.text.len.slot\n"));
                        for (i, v) in values.iter().enumerate() {
                            let ConstRuntimeValueData::Int(int_val) = v else {
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
                    } else if matches!(value, ConstHeapValueData::Map(values) if values.is_empty()) {
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
            Opcode::LoadFunction | Opcode::MakeClosure => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let Some(value) = static_callable_value(&artifact.module.functions, instr, &static_regs) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode::NewList
            | Opcode::LoadNil
            | Opcode::LoadInt
            | Opcode::LoadFloat
            | Opcode::LoadBool
            | Opcode::Move
            | Opcode::ToString
            | Opcode::ConcatString
            | Opcode::AddFloat
            | Opcode::SubFloat
            | Opcode::MulFloat
            | Opcode::DivFloat
            | Opcode::ModFloat => {
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
            Opcode::AddInt | Opcode::SubInt | Opcode::MulInt | Opcode::DivInt | Opcode::ModInt => {
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
            Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI => {
                if !emit_int_immediate_block(
                    &mut ir,
                    code,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut static_regs,
                    &mut tmp_index,
                    code.len(),
                ) {
                    return Ok(None);
                }
            }
            Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt => {
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
            opcode if opcode.is_compare_test() => {
                if !emit_compare_test_block(
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
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue | Opcode::BrNil | Opcode::BrNotNil => {
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
            Opcode::Not => {
                if emit_not_block(
                    &mut ir,
                    &mut static_regs,
                    code,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::IsNil | Opcode::IsList | Opcode::IsMap => {
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
            Opcode::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len()) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
            }
            Opcode::ForLoopI => {
                let Some(fact) = function.performance.for_loop(pc) else {
                    return Ok(None);
                };
                if !emit_for_loop_i_block(
                    &mut ir,
                    &mut static_regs,
                    code,
                    pc,
                    instr,
                    register_count,
                    *fact,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode::GetGlobal => {
                if emit_get_global_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    &static_globals,
                    global_names,
                    pc,
                    instr,
                    register_count,
                    global_count,
                    facts,
                    &mut tmp_index,
                    code.len(),
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::SetGlobal => {
                if emit_set_global_block(
                    &mut ir,
                    &mut static_regs,
                    &mut static_globals,
                    pc,
                    instr,
                    register_count,
                    global_count,
                    facts,
                    &mut tmp_index,
                    code.len(),
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::Len => {
                if emit_len_block(
                    &mut ir,
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
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::ToIter => {
                if emit_to_iter_block(
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr,
                    register_count,
                    &mut ir,
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::StringSplit => {
                if !emit_string_split_block(
                    &mut ir,
                    &mut static_regs,
                    code,
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
            Opcode::ListJoin => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(target) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(delimiter) = static_regs.get(instr.c() as usize).and_then(Clone::clone).or_else(|| {
                    crate::llvm::scalar::contains::local_static_string_before(code, strings, pc, instr.c())
                }) else {
                    return Ok(None);
                };
                if let (
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: element @ (NativeListElementKind::Text | NativeListElementKind::StrPtr),
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
                        element: *element,
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
                static_regs[instr.a() as usize] = Some(if let [NativeTextPart::StrPtr(ptr)] = text.as_slice() {
                    NativeStraightlineValue::StringPtr(ptr.clone())
                } else {
                    NativeStraightlineValue::Text(text)
                });
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode::StringStartsWith => {
                if emit_string_starts_with_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    strings,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::GetIndex | Opcode::GetList => {
                if !emit_get_index_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    function,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode::GetFieldK => {
                if !emit_get_field_k_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    function,
                    pc,
                    instr,
                    register_count,
                    &mut tmp_index,
                ) {
                    return Ok(None);
                }
            }
            Opcode::ListPush => {
                if !emit_list_push_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
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
            Opcode::NewObject => {
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
            Opcode::NewRange => {
                let Some(value) =
                    static_int_range_from_registers(&static_regs, code, int_consts, pc, instr, String::new())
                else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode::Contains | Opcode::SliceFrom | Opcode::MapRest => {
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
            Opcode::SetIndex => {
                if !emit_set_index_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    function,
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
            Opcode::SetFieldK => {
                if !emit_set_field_k_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    code,
                    int_consts,
                    strings,
                    function,
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
            Opcode::Call => {
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
                    let direct_instr = Instr::abc(Opcode::CallDirect, instr.a(), function_index, instr.c());
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
                    static_or_recovered_call_target(&static_regs, code, global_names, pc, instr.b())
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
                if builtin == NativeBuiltin::Println
                    && instr.c() == 1
                    && let Some(arg) = instr.b().checked_add(1)
                    && local_register_kind_before(code, pc, arg) == Some(NativeScalarKind::MaybeStrPtr)
                {
                    let present = next_tmp(&mut tmp_index);
                    let cond = next_tmp(&mut tmp_index);
                    let value = next_tmp(&mut tmp_index);
                    let text = next_tmp(&mut tmp_index);
                    ir.push_str(&format!("  {present} = load i64, ptr %r{arg}.present.slot\n"));
                    ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
                    ir.push_str(&format!("  {value} = load ptr, ptr %r{arg}.slot\n"));
                    ir.push_str(&format!("  {text} = select i1 {cond}, ptr {value}, ptr @lk_nil_text\n"));
                    ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {text})\n"));
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                if emit_runtime_formatted_print_call(
                    &mut ir,
                    &mut extra_globals,
                    builtin,
                    instr,
                    code,
                    strings,
                    heap_values,
                    &static_regs,
                    facts,
                    pc,
                    register_count,
                    &mut tmp_index,
                ) {
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                if emit_dynamic_list_builtin_call_from_regs_block(
                    &mut ir,
                    &mut extra_globals,
                    &mut static_regs,
                    instr,
                    builtin,
                    facts,
                    pc,
                    code,
                    heap_values,
                    code.len(),
                    &mut tmp_index,
                ) {
                    continue;
                }
                if let Some(mut args) = static_or_recovered_call_args(
                    &static_regs,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    pc,
                    instr.b(),
                    instr.c(),
                ) {
                    if emit_dynamic_list_method_call_block(
                        &mut ir,
                        &mut static_regs,
                        code,
                        instr,
                        pc,
                        &args,
                        &mut tmp_index,
                    ) {
                        continue;
                    }
                    if emit_dynamic_list_builtin_call_block(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        code,
                        heap_values,
                        code.len(),
                        builtin,
                        &args,
                        &mut tmp_index,
                    ) {
                        continue;
                    }
                    for (offset, arg) in args.iter_mut().enumerate() {
                        if matches!(arg, NativeStraightlineValue::DynamicList { .. })
                            && let Some(reg) = instr
                                .b()
                                .checked_add(1)
                                .and_then(|start| start.checked_add(u8::try_from(offset).ok()?))
                            && let Some(value) = local_static_heap_const_before(code, heap_values, pc, reg)
                                .or_else(|| local_static_container_before(code, heap_values, pc, reg))
                        {
                            *arg = value;
                        }
                    }
                    if let [
                        target,
                        NativeStraightlineValue::String { value: method, .. },
                        method_args,
                    ] = args.as_slice()
                        && let Some(value) = static_object_list_map_method(
                            artifact,
                            target.clone(),
                            method,
                            method_args.clone(),
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
                        && let [
                            callable @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }),
                        ] = elements.as_slice()
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
                    if emit_static_i64_list_zip_arglist_call(
                        &mut static_regs,
                        code,
                        int_consts,
                        strings,
                        heap_values,
                        instr,
                        &args,
                    ) {
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
                    if emit_dynamic_map_get_method_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
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
                        && let ConstRuntimeValueData::Int(index) = elements[0]
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
                    if emit_dynamic_map_set_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        builtin,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_dynamic_map_values_call(
                        &mut ir,
                        &mut static_regs,
                        instr,
                        pc,
                        builtin,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_dynamic_map_get_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        builtin,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_dynamic_map_has_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        builtin,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_dynamic_map_delete_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        builtin,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_dynamic_map_keys_call(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        builtin,
                        &args,
                        &mut tmp_index,
                    )
                    .is_some()
                    {
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    if emit_dynamic_list_builtin_call_block(
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        instr,
                        pc,
                        code,
                        heap_values,
                        code.len(),
                        builtin,
                        &args,
                        &mut tmp_index,
                    ) {
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
                    if !emit_recovered_builtin_call_block(
                        artifact,
                        code,
                        int_consts,
                        strings,
                        heap_values,
                        global_names,
                        facts,
                        &mut ir,
                        &mut extra_globals,
                        &mut static_regs,
                        &mut static_globals,
                        instr,
                        pc,
                        builtin,
                        register_count,
                        &mut tmp_index,
                    ) {
                        return Ok(None);
                    }
                }
            }
            Opcode::CallNamed => {
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
            Opcode::CallDirect => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let callee_index = instr.b();
                if let Some(callee) = artifact.module.functions.get(callee_index as usize)
                    && emit_direct_emit_helper_call(
                        &mut ir,
                        &mut extra_globals,
                        callee,
                        facts,
                        &static_regs,
                        instr,
                        pc,
                        &mut tmp_index,
                    )
                    .is_some()
                {
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
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
                    if function_has_list_return_shape(callee) {
                        if emit_list_direct_call(
                            &mut ir,
                            &mut extra_globals,
                            &mut static_regs,
                            instr,
                            pc,
                            callee_index as usize,
                            facts,
                            &mut tmp_index,
                        )
                        .is_none()
                        {
                            return Ok(None);
                        }
                        additional_subfn_indices.push(u16::from(callee_index));
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
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
            Opcode::Return => {
                if emit_return_block(
                    &mut ir,
                    &mut extra_globals,
                    &static_regs,
                    code,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::StoreCellVal => {
                if emit_store_cell_block(
                    &mut ir,
                    &mut static_regs,
                    code,
                    int_consts,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::LoadCellVal => {
                if emit_load_cell_block(
                    &mut ir,
                    &mut static_regs,
                    code,
                    pc,
                    instr,
                    register_count,
                    facts,
                    &mut tmp_index,
                )
                .is_none()
                {
                    return Ok(None);
                }
            }
            Opcode::Nop | Opcode::Raise => emit_branch_to_next(&mut ir, pc, code.len()),
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
