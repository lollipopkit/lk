use anyhow::Result;

use crate::vm::{ConstHeapValue32Data, Instr32, Module32Artifact, Opcode32};

use super::{
    const_display::llvm_string_constant,
    dynamic_containers::{
        emit_dynamic_int_list_allocas, emit_dynamic_int_list_get, emit_dynamic_int_list_push,
        emit_dynamic_joined_text_len, emit_dynamic_string_int_map_allocas, emit_dynamic_string_int_map_get,
        emit_dynamic_string_int_map_set, emit_dynamic_text_len, emit_dynamic_text_list_push,
        native_dynamic_container_helpers,
    },
    ir_text::{
        emit_branch_to_next, llvm_float_literal, native_label, native_relative_target, native_scalar_main_header,
        next_tmp, reg_in_bounds,
    },
    options::LlvmBackendOptions,
    output::emit_native_builtin_call,
    scalar_block_helpers::{
        clear_control_flow_static_values, concat_text_values, control_flow_static_boundaries,
        emit_dynamic_string_starts_with, emit_mixed_numeric_int_opcode_block, emit_native_block_core_call_method,
        emit_static_named_call, emit_static_scalar_value_store_if_needed, emit_static_string_i64_map_get,
        emit_string_ptr_equality_block, i64_slot_kind, mark_static_untaken_return_path, native_static_string,
        static_call_args, static_call_target, static_callable_value, static_string_i64_map_supported,
        static_string_value_trusted_at_call, store_native_scalar_call_result, text_value_from_reg,
        three_regs_in_bounds,
    },
    scalar_emit::{
        emit_f64_binary_block, emit_i64_binary_block, emit_native_return_print, emit_numeric_compare_block,
        emit_scalar_equality_block,
    },
    scalar_facts::{NativeScalarFacts, NativeScalarKind},
    scalar_inline::{emit_inline_direct_scalar_call, emit_inline_static_scalar_call},
    straightline_value::{
        NativeStraightlineValue, native_static_f64_binary, native_static_global, native_static_index,
        native_static_list_from_values, native_static_set_index, native_static_truthy,
        native_straightline_heap_const_value,
    },
};

pub(super) fn compile_native_scalar_main_blocks(
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
    scalar_facts: &NativeScalarFacts,
) -> Result<Option<String>> {
    let mut ir = native_scalar_main_header(options);
    for reg in 0..register_count {
        ir.push_str(&format!("  %r{reg}.slot = alloca i64\n"));
        ir.push_str(&format!("  %r{reg}.present.slot = alloca i64\n"));
    }
    for global in 0..global_count {
        ir.push_str(&format!("  %g{global}.slot = alloca i64\n"));
    }
    for (pc, instr) in code.iter().copied().enumerate() {
        if instr.opcode() != Opcode32::CallDirect {
            if matches!(instr.opcode(), Opcode32::LoadHeapConst)
                && matches!(heap_values.get(instr.bx() as usize), Some(ConstHeapValue32Data::Map(values)) if values.is_empty())
            {
                emit_dynamic_string_int_map_allocas(&mut ir, &format!("map{pc}"));
            } else if matches!(instr.opcode(), Opcode32::LoadHeapConst)
                && matches!(heap_values.get(instr.bx() as usize), Some(ConstHeapValue32Data::List(values)) if values.is_empty())
            {
                emit_dynamic_int_list_allocas(&mut ir, &format!("list{pc}"));
            }
            continue;
        }
        let Some(function) = artifact.module.functions.get(instr.b() as usize) else {
            return Ok(None);
        };
        for reg in 0..function.register_count {
            ir.push_str(&format!("  %call{pc}.r{reg}.slot = alloca i64\n"));
            ir.push_str(&format!("  %call{pc}.r{reg}.present.slot = alloca i64\n"));
        }
    }
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
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicIntList { id: pc });
                    } else if matches!(value, ConstHeapValue32Data::Map(values) if values.is_empty()) {
                        ir.push_str(&format!("  store i64 0, ptr %map{pc}.len.slot\n"));
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicStringIntMap { id: pc });
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
            Opcode32::NewList => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let start = instr.b() as usize;
                let Some(end) = start.checked_add(instr.c() as usize) else {
                    return Ok(None);
                };
                let Some(values) = static_regs.get(start..end) else {
                    return Ok(None);
                };
                let Some(values) = values.iter().cloned().collect::<Option<Vec<_>>>() else {
                    return Ok(None);
                };
                let Some(value) = native_static_list_from_values(&values, String::new()) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadNil => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
                ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadInt => {
                let Some(value) = int_consts.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadFloat => {
                let Some(value) = float_consts.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(llvm_float_literal(*value)));
                ir.push_str(&format!(
                    "  store double {}, ptr %r{}.slot\n",
                    llvm_float_literal(*value),
                    instr.a()
                ));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadBool => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
                let value = i64::from(instr.b() != 0);
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Move => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                if let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) {
                    static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
                    let value = next_tmp(&mut tmp_index);
                    let ty = kind.llvm_type();
                    ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
                    ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
                    if kind == NativeScalarKind::MaybeI64 {
                        let present = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.b()));
                        ir.push_str(&format!("  store i64 {present}, ptr %r{}.present.slot\n", instr.a()));
                    }
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                if let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone) {
                    static_regs[instr.a() as usize] = Some(value);
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                return Ok(None);
            }
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
                let Some(lhs) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let Some(rhs) = scalar_facts.register_kind_before(pc, instr.c()) else {
                    return Ok(None);
                };
                if i64_slot_kind(lhs) && i64_slot_kind(rhs) {
                    emit_i64_binary_block(&mut ir, instr, &mut tmp_index);
                } else if lhs.is_numeric() && rhs.is_numeric() {
                    emit_mixed_numeric_int_opcode_block(&mut ir, "", instr, lhs, rhs, &mut tmp_index);
                } else {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::ToString => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(value) = text_value_from_reg(
                    &mut ir,
                    instr.b(),
                    scalar_facts.register_kind_before(pc, instr.b()),
                    &static_regs,
                    &mut tmp_index,
                ) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::ConcatString => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(lhs) = text_value_from_reg(
                    &mut ir,
                    instr.b(),
                    scalar_facts.register_kind_before(pc, instr.b()),
                    &static_regs,
                    &mut tmp_index,
                ) else {
                    return Ok(None);
                };
                let Some(rhs) = text_value_from_reg(
                    &mut ir,
                    instr.c(),
                    scalar_facts.register_kind_before(pc, instr.c()),
                    &static_regs,
                    &mut tmp_index,
                ) else {
                    return Ok(None);
                };
                let Some(value) = concat_text_values(lhs, rhs) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::AddFloat | Opcode32::SubFloat | Opcode32::MulFloat | Opcode32::DivFloat | Opcode32::ModFloat => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                if let (Some(NativeStraightlineValue::F64(lhs)), Some(NativeStraightlineValue::F64(rhs))) = (
                    static_regs.get(instr.b() as usize).and_then(Clone::clone),
                    static_regs.get(instr.c() as usize).and_then(Clone::clone),
                ) && let Some(value) = native_static_f64_binary(&lhs, &rhs, instr.opcode())
                {
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(value.clone()));
                    ir.push_str(&format!("  store double {value}, ptr %r{}.slot\n", instr.a()));
                    emit_branch_to_next(&mut ir, pc, code.len());
                    continue;
                }
                static_regs[instr.a() as usize] = None;
                emit_f64_binary_block(&mut ir, instr, &mut tmp_index);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let Some(rhs_kind) = scalar_facts.register_kind_before(pc, instr.c()) else {
                    return Ok(None);
                };
                if kind == rhs_kind && kind.is_numeric() {
                    emit_numeric_compare_block(&mut ir, instr, kind, &mut tmp_index);
                } else if kind == rhs_kind && kind == NativeScalarKind::StrPtr {
                    emit_string_ptr_equality_block(&mut ir, instr, &mut tmp_index);
                } else if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) {
                    emit_scalar_equality_block(&mut ir, instr, kind, rhs_kind, &mut tmp_index);
                } else {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Test => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                if let Some(value) = static_regs
                    .get(instr.a() as usize)
                    .and_then(|value| value.as_ref())
                    .and_then(native_static_truthy)
                {
                    let fallthrough = pc + 1;
                    let Some(relative) = native_relative_target(pc, instr.c() as i8 as i32, code.len()) else {
                        return Ok(None);
                    };
                    let truthy_target = if instr.b() != 0 { fallthrough } else { relative };
                    let falsy_target = if instr.b() != 0 { relative } else { fallthrough };
                    let target = if value { truthy_target } else { falsy_target };
                    let untaken = if value { falsy_target } else { truthy_target };
                    mark_static_untaken_return_path(&mut skip_static_pcs, &static_boundaries, code, untaken);
                    ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
                    continue;
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.a()) else {
                    return Ok(None);
                };
                let fallthrough = pc + 1;
                let Some(relative) = native_relative_target(pc, instr.c() as i8 as i32, code.len()) else {
                    return Ok(None);
                };
                let truthy_target = if instr.b() != 0 { fallthrough } else { relative };
                let falsy_target = if instr.b() != 0 { relative } else { fallthrough };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
                        ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                        ir.push_str(&format!(
                            "  br i1 {cond}, label {}, label {}\n",
                            native_label(truthy_target, code.len()),
                            native_label(falsy_target, code.len())
                        ));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!("  br label {}\n", native_label(falsy_target, code.len())));
                    }
                    NativeScalarKind::I64
                    | NativeScalarKind::F64
                    | NativeScalarKind::StrPtr
                    | NativeScalarKind::MaybeI64 => {
                        ir.push_str(&format!("  br label {}\n", native_label(truthy_target, code.len())));
                    }
                }
            }
            Opcode32::Not => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
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
                    _ => return Ok(None),
                }
                static_regs[instr.a() as usize] = None;
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::IsNil => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let value = i64::from(kind == NativeScalarKind::Nil);
                static_regs[instr.a() as usize] = None;
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
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
                let Some(kind) = scalar_facts.global_kind_before(pc, instr.bx()) else {
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
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.a()) else {
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
                let Some(target) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                match target {
                    NativeStraightlineValue::String { value, .. } if value.is_ascii() => {
                        ir.push_str(&format!("  store i64 {}, ptr %r{}.slot\n", value.len(), instr.a()));
                    }
                    NativeStraightlineValue::Text(parts) => {
                        if emit_dynamic_text_len(&mut ir, instr.a(), &parts, &mut tmp_index).is_none() {
                            return Ok(None);
                        }
                    }
                    NativeStraightlineValue::DynamicTextChar => {
                        ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeStraightlineValue::DynamicJoinedText { id, delimiter_len } => {
                        if emit_dynamic_joined_text_len(&mut ir, instr.a(), id, delimiter_len, &mut tmp_index).is_none()
                        {
                            return Ok(None);
                        }
                    }
                    _ => return Ok(None),
                }
                static_regs[instr.a() as usize] = None;
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
                    NativeStraightlineValue::DynamicTextList { id },
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
                } else if scalar_facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
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
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(target) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if let NativeStraightlineValue::Text(_) = target {
                    if scalar_facts.register_kind_before(pc, instr.c()) != Some(NativeScalarKind::I64) {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicTextChar);
                } else if let NativeStraightlineValue::DynamicIntList { id } = target {
                    if scalar_facts.register_kind_before(pc, instr.c()) != Some(NativeScalarKind::I64)
                        || emit_dynamic_int_list_get(&mut ir, id, instr.a(), instr.c(), &mut tmp_index).is_none()
                    {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = None;
                } else if let NativeStraightlineValue::DynamicStringIntMap { id } = target {
                    let Some(key) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
                        return Ok(None);
                    };
                    if emit_dynamic_string_int_map_get(&mut ir, &mut extra_globals, id, instr.a(), key, &mut tmp_index)
                        .is_none()
                    {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = None;
                } else if let NativeStraightlineValue::Map { entries, .. } = &target
                    && (scalar_facts.register_kind_before(pc, instr.c()) == Some(NativeScalarKind::StrPtr)
                        || !static_string_value_trusted_at_call(code, pc, instr.c()))
                    && static_string_i64_map_supported(entries)
                {
                    if emit_static_string_i64_map_get(
                        &mut ir,
                        &mut extra_globals,
                        entries,
                        "",
                        instr.a(),
                        instr.c(),
                        &mut tmp_index,
                    )
                    .is_none()
                    {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = None;
                } else {
                    let Some(key) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
                        return Ok(None);
                    };
                    let Some(value) = native_static_index(target, key, String::new()) else {
                        return Ok(None);
                    };
                    if emit_static_scalar_value_store_if_needed(&mut ir, instr.a(), &value).is_none() {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(value);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::ListPush => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(NativeStraightlineValue::DynamicIntList { id }) =
                    static_regs.get(instr.a() as usize).and_then(Clone::clone)
                else {
                    if let Some(NativeStraightlineValue::DynamicTextList { id }) =
                        static_regs.get(instr.a() as usize).and_then(Clone::clone)
                    {
                        let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                            return Ok(None);
                        };
                        if emit_dynamic_text_list_push(&mut ir, id, value, &mut tmp_index).is_none() {
                            return Ok(None);
                        }
                        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicTextList { id });
                        emit_branch_to_next(&mut ir, pc, code.len());
                        continue;
                    }
                    return Ok(None);
                };
                if scalar_facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::I64) {
                    if emit_dynamic_int_list_push(&mut ir, id, instr.b(), &mut tmp_index).is_none() {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicIntList { id });
                } else {
                    let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                        return Ok(None);
                    };
                    if emit_dynamic_text_list_push(&mut ir, id, value, &mut tmp_index).is_none() {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicTextList { id });
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::SetIndex => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(target) = static_regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if let NativeStraightlineValue::DynamicStringIntMap { id } = target {
                    let Some(value_kind) = scalar_facts.register_kind_before(pc, instr.c()) else {
                        return Ok(None);
                    };
                    if value_kind != NativeScalarKind::I64 {
                        return Ok(None);
                    }
                    if emit_dynamic_string_int_map_set(&mut ir, &mut extra_globals, id, instr.c(), key, &mut tmp_index)
                        .is_none()
                    {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicStringIntMap { id });
                } else {
                    let Some(value) = static_regs.get(instr.c() as usize).and_then(Clone::clone) else {
                        return Ok(None);
                    };
                    let Some(value) = native_static_set_index(target, key, value) else {
                        return Ok(None);
                    };
                    static_regs[instr.a() as usize] = Some(value);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Call => {
                if instr.a() != instr.b() || !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
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
                        scalar_facts,
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
                let Some(args) = static_call_args(&static_regs, instr.b(), instr.c()) else {
                    return Ok(None);
                };
                let value = if let Some(value) =
                    emit_native_block_core_call_method(&mut ir, &mut extra_globals, builtin, &args, &mut tmp_index)
                {
                    value
                } else if let Some(value) = emit_native_builtin_call(&mut ir, builtin, &args, &mut tmp_index) {
                    value
                } else {
                    return Ok(None);
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
            }
            Opcode32::CallNamed => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                if emit_static_named_call(
                    &mut ir,
                    &mut extra_globals,
                    artifact,
                    scalar_facts,
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
                let Some(callee) = artifact.module.functions.get(instr.b() as usize) else {
                    return Ok(None);
                };
                if emit_inline_direct_scalar_call(
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
                    scalar_facts,
                    &mut tmp_index,
                    code.len(),
                )
                .is_none()
                {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
            }
            Opcode32::Return => {
                if instr.b() == 0 {
                    ir.push_str("  ret i32 0\n");
                } else if instr.b() == 1 && reg_in_bounds(register_count, instr.a()) {
                    let Some(kind) = scalar_facts.register_kind_before(pc, instr.a()) else {
                        return Ok(None);
                    };
                    emit_native_return_print(&mut ir, pc, instr.a(), kind, &mut tmp_index);
                    ir.push_str("  ret i32 0\n");
                } else {
                    return Ok(None);
                }
            }
            Opcode32::Nop => emit_branch_to_next(&mut ir, pc, code.len()),
            _ => return Ok(None),
        }
        ir.push('\n');
    }
    ir.push_str("exit:\n");
    ir.push_str("  ret i32 0\n");
    ir.push_str("lk_divisor_zero:\n");
    ir.push_str("  ret i32 1\n");
    ir.push_str("}\n");
    ir.push_str(native_dynamic_container_helpers());
    ir.push_str(&extra_globals);
    Ok(Some(ir))
}
