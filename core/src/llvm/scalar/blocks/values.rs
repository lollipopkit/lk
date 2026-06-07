use crate::{
    llvm::{
        const_display::llvm_string_constant,
        ir_text::{emit_branch_to_next, llvm_float_literal, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::{
                concat_text_values, emit_static_scalar_value_store_if_needed, local_heap_kind_before,
                local_register_kind_before, local_static_container_before, local_static_i64_before,
                static_register_value_trusted_before, text_value_from_reg, three_regs_in_bounds,
            },
            contains::{
                emit_dynamic_int_list_move, local_static_heap_const_before, local_static_i64_value_before,
                local_static_index_value_before, local_static_iter_zip_before, local_static_map_rest_before,
                local_static_object_before, local_static_string_before,
            },
            emit::emit_f64_binary_block,
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{
            NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue, NativeTextPart,
            native_static_f64_binary, native_static_list_from_values, native_static_text_string,
        },
    },
    vm::{ConstHeapValueData, Instr, Opcode},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_value_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    global_names: &[String],
    int_consts: &[i64],
    float_consts: &[f64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    match instr.opcode() {
        Opcode::NewList => emit_new_list(
            ir,
            static_regs,
            int_consts,
            strings,
            heap_values,
            code,
            pc,
            instr,
            register_count,
            facts,
            tmp_index,
        ),
        Opcode::LoadNil => emit_load_nil(ir, static_regs, code, pc, instr, register_count),
        Opcode::LoadInt => emit_load_int(ir, static_regs, int_consts, code, pc, instr, register_count),
        Opcode::LoadFloat => emit_load_float(ir, static_regs, float_consts, code, pc, instr, register_count),
        Opcode::LoadBool => emit_load_bool(ir, static_regs, code, pc, instr, register_count),
        Opcode::Move => emit_move(
            ir,
            static_regs,
            global_names,
            int_consts,
            strings,
            heap_values,
            code,
            pc,
            instr,
            register_count,
            facts,
            tmp_index,
        ),
        Opcode::ToString => emit_to_string(ir, static_regs, code, pc, instr, register_count, facts, tmp_index),
        Opcode::ConcatString => emit_concat_string(
            ir,
            extra_globals,
            static_regs,
            code,
            pc,
            instr,
            register_count,
            facts,
            tmp_index,
        ),
        Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat => {
            emit_float_arithmetic(ir, static_regs, code, pc, instr, register_count, facts, tmp_index)
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_new_list(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
    }
    let start = instr.b() as usize;
    let Some(end) = start.checked_add(instr.c() as usize) else {
        return false;
    };
    if end > register_count {
        return false;
    }
    if instr.c() == 1
        && let Some(value @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. })) =
            static_regs.get(start).cloned().flatten()
    {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList { elements: vec![value] });
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if static_regs.get(start..end).is_some_and(|values| {
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
        static_regs[instr.a() as usize] = (start..end)
            .map(|reg| {
                let reg_u8 = u8::try_from(reg).ok()?;
                static_regs
                    .get(reg)
                    .cloned()
                    .flatten()
                    .or_else(|| local_static_i64_value_before(code, int_consts, strings, heap_values, pc, reg_u8))
                    .or_else(|| local_static_i64_before(code, int_consts, pc, reg_u8))
                    .or_else(|| local_static_heap_const_before(code, heap_values, pc, reg_u8))
                    .or_else(|| local_static_string_before(code, strings, pc, reg_u8))
                    .or_else(|| {
                        recent_dynamic_i64_map_get_before(static_regs, code, pc, reg_u8).then(|| {
                            let value = next_tmp(tmp_index);
                            let present = next_tmp(tmp_index);
                            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
                            ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
                            NativeStraightlineValue::MaybeI64 { value, present }
                        })
                    })
                    .or_else(|| {
                        recent_dynamic_i64_ptr_map_get_before(static_regs, code, pc, reg_u8).then(|| {
                            let value = next_tmp(tmp_index);
                            let present = next_tmp(tmp_index);
                            ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
                            ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
                            NativeStraightlineValue::MaybeStrPtr { value, present }
                        })
                    })
                    .or_else(|| {
                        match facts
                            .register_kind_before(pc, reg_u8)
                            .or_else(|| local_register_kind_before(code, pc, reg_u8))
                        {
                            Some(NativeScalarKind::I64) => {
                                let loaded = next_tmp(tmp_index);
                                ir.push_str(&format!("  {loaded} = load i64, ptr %r{reg}.slot\n"));
                                Some(NativeStraightlineValue::I64(loaded))
                            }
                            Some(NativeScalarKind::MaybeI64) => {
                                let value = next_tmp(tmp_index);
                                let present = next_tmp(tmp_index);
                                ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
                                ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
                                Some(NativeStraightlineValue::MaybeI64 { value, present })
                            }
                            Some(NativeScalarKind::StrPtr) => {
                                let loaded = next_tmp(tmp_index);
                                ir.push_str(&format!("  {loaded} = load ptr, ptr %r{reg}.slot\n"));
                                Some(NativeStraightlineValue::StringPtr(loaded))
                            }
                            Some(NativeScalarKind::MaybeStrPtr) => {
                                let value = next_tmp(tmp_index);
                                let present = next_tmp(tmp_index);
                                ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
                                ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
                                Some(NativeStraightlineValue::MaybeStrPtr { value, present })
                            }
                            _ => None,
                        }
                    })
                    .or_else(|| {
                        (facts
                            .register_kind_before(pc, reg_u8)
                            .or_else(|| local_register_kind_before(code, pc, reg_u8))
                            == Some(NativeScalarKind::F64))
                        .then(|| {
                            let loaded = next_tmp(tmp_index);
                            ir.push_str(&format!("  {loaded} = load double, ptr %r{reg}.slot\n"));
                            NativeStraightlineValue::F64(loaded)
                        })
                    })
                    .or_else(|| {
                        (facts
                            .register_kind_before(pc, reg_u8)
                            .or_else(|| local_register_kind_before(code, pc, reg_u8))
                            == Some(NativeScalarKind::Bool))
                        .then(|| {
                            let loaded = next_tmp(tmp_index);
                            ir.push_str(&format!("  {loaded} = load i64, ptr %r{reg}.slot\n"));
                            NativeStraightlineValue::Bool(loaded)
                        })
                    })
            })
            .collect::<Option<Vec<_>>>()
            .map(|elements| NativeStraightlineValue::ArgList { elements });
        if static_regs[instr.a() as usize].is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
    }
    let static_i64_values = (start..end)
        .map(|reg| {
            static_regs
                .get(reg)
                .cloned()
                .flatten()
                .or_else(|| {
                    local_static_i64_value_before(code, int_consts, strings, heap_values, pc, u8::try_from(reg).ok()?)
                })
                .or_else(|| local_static_i64_before(code, int_consts, pc, u8::try_from(reg).ok()?))
        })
        .collect::<Option<Vec<_>>>();
    if let Some(values) = static_i64_values
        && let Some(value) = native_static_list_from_values(&values, String::new())
    {
        static_regs[instr.a() as usize] = Some(value);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let static_object_values = (start..end)
        .map(|reg| {
            let reg_u8 = u8::try_from(reg).ok()?;
            match static_regs
                .get(reg)
                .cloned()
                .flatten()
                .or_else(|| local_static_object_before(static_regs, code, int_consts, pc, reg_u8))?
            {
                value @ NativeStraightlineValue::Object { .. } => Some(value),
                _ => None,
            }
        })
        .collect::<Option<Vec<_>>>();
    if let Some(elements) = static_object_values {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList { elements });
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let all_i64 = (start..end).all(|i| match static_regs.get(i).and_then(|v| v.as_ref()) {
        Some(NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }) => false,
        Some(NativeStraightlineValue::I64(s)) if !s.starts_with('%') => true,
        _ => {
            facts
                .register_kind_before(pc, i as u8)
                .or_else(|| local_register_kind_before(code, pc, i as u8))
                == Some(NativeScalarKind::I64)
        }
    });
    if all_i64 {
        emit_dynamic_i64_list(ir, static_regs, start, end, pc, instr, tmp_index);
    } else if !emit_static_list(
        ir,
        static_regs,
        int_consts,
        strings,
        heap_values,
        code,
        start,
        end,
        pc,
        instr,
        facts,
        tmp_index,
    ) {
        return false;
    }
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn recent_dynamic_i64_map_get_before(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    reg: u8,
) -> bool {
    recent_dynamic_i64_map_get_before_inner(static_regs, code, pc, reg, 0)
}

fn recent_dynamic_i64_map_get_before_inner(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    reg: u8,
    depth: usize,
) -> bool {
    if depth > 8 {
        return false;
    }
    for prev_pc in (0..pc).rev() {
        let Some(prev) = code.get(prev_pc).copied() else {
            return false;
        };
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::Move if prev.b() != reg => {
                recent_dynamic_i64_map_get_before_inner(static_regs, code, prev_pc, prev.b(), depth + 1)
            }
            Opcode::GetIndex => matches!(
                static_regs.get(prev.b() as usize).and_then(|value| value.as_ref()),
                Some(NativeStraightlineValue::DynamicMap {
                    key: NativeMapKeyKind::I64,
                    value: NativeMapValueKind::I64,
                    ..
                })
            ),
            _ => false,
        };
    }
    false
}

fn recent_dynamic_i64_ptr_map_get_before(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    reg: u8,
) -> bool {
    recent_dynamic_i64_ptr_map_get_before_inner(static_regs, code, pc, reg, 0)
}

fn recent_dynamic_i64_ptr_map_get_before_inner(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    reg: u8,
    depth: usize,
) -> bool {
    if depth > 8 {
        return false;
    }
    for prev_pc in (0..pc).rev() {
        let Some(prev) = code.get(prev_pc).copied() else {
            return false;
        };
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::Move if prev.b() != reg => {
                recent_dynamic_i64_ptr_map_get_before_inner(static_regs, code, prev_pc, prev.b(), depth + 1)
            }
            Opcode::GetIndex => matches!(
                static_regs.get(prev.b() as usize).and_then(|value| value.as_ref()),
                Some(NativeStraightlineValue::DynamicMap {
                    key: NativeMapKeyKind::I64,
                    value: NativeMapValueKind::StrPtr,
                    ..
                })
            ),
            _ => false,
        };
    }
    false
}

fn emit_dynamic_i64_list(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    start: usize,
    end: usize,
    pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) {
    let n = end - start;
    ir.push_str(&format!("  store i64 {n}, ptr %list{pc}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %list{pc}.text.len.slot\n"));
    for (i, reg) in (start..end).enumerate() {
        let slot = next_tmp(tmp_index);
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x i64], ptr %list{pc}.value.slots, i64 0, i64 {i}\n"
        ));
        if let Some(Some(NativeStraightlineValue::I64(s))) = static_regs.get(reg) {
            ir.push_str(&format!("  store i64 {s}, ptr {slot}\n"));
        } else {
            let tmp = next_tmp(tmp_index);
            ir.push_str(&format!("  {tmp} = load i64, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 {tmp}, ptr {slot}\n"));
        }
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::I64,
    });
}

#[allow(clippy::too_many_arguments)]
fn emit_static_list(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    start: usize,
    end: usize,
    pc: usize,
    instr: Instr,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    let Some(elems_slice) = static_regs.get(start..end) else {
        return false;
    };
    let elems = elems_slice
        .iter()
        .enumerate()
        .map(|(offset, value)| match value {
            Some(NativeStraightlineValue::DynamicList {
                element: NativeListElementKind::I64,
                ..
            })
            | None => local_static_i64_value_before(
                code,
                int_consts,
                strings,
                heap_values,
                pc,
                u8::try_from(start + offset).ok()?,
            )
            .or_else(|| local_static_i64_before(code, int_consts, pc, u8::try_from(start + offset).ok()?))
            .or_else(|| local_static_heap_const_before(code, heap_values, pc, u8::try_from(start + offset).ok()?))
            .or_else(|| local_static_string_before(code, strings, pc, u8::try_from(start + offset).ok()?))
            .or_else(|| {
                let reg = u8::try_from(start + offset).ok()?;
                if facts.register_kind_before(pc, reg) == Some(NativeScalarKind::F64) {
                    let loaded = next_tmp(tmp_index);
                    ir.push_str(&format!("  {loaded} = load double, ptr %r{reg}.slot\n"));
                    return Some(NativeStraightlineValue::F64(loaded));
                }
                if facts.register_kind_before(pc, reg) == Some(NativeScalarKind::I64) {
                    let loaded = next_tmp(tmp_index);
                    ir.push_str(&format!("  {loaded} = load i64, ptr %r{reg}.slot\n"));
                    return Some(NativeStraightlineValue::I64(loaded));
                }
                if facts.register_kind_before(pc, reg) == Some(NativeScalarKind::MaybeI64) {
                    let value = next_tmp(tmp_index);
                    let present = next_tmp(tmp_index);
                    ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
                    ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
                    return Some(NativeStraightlineValue::MaybeI64 { value, present });
                }
                if facts.register_kind_before(pc, reg) == Some(NativeScalarKind::Bool) {
                    let loaded = next_tmp(tmp_index);
                    ir.push_str(&format!("  {loaded} = load i64, ptr %r{reg}.slot\n"));
                    return Some(NativeStraightlineValue::Bool(loaded));
                }
                if facts.register_kind_before(pc, reg) == Some(NativeScalarKind::MaybeStrPtr) {
                    let value = next_tmp(tmp_index);
                    let present = next_tmp(tmp_index);
                    ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
                    ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
                    return Some(NativeStraightlineValue::MaybeStrPtr { value, present });
                }
                (facts.register_kind_before(pc, reg) == Some(NativeScalarKind::StrPtr)).then(|| {
                    let loaded = next_tmp(tmp_index);
                    ir.push_str(&format!("  {loaded} = load ptr, ptr %r{reg}.slot\n"));
                    NativeStraightlineValue::StringPtr(loaded)
                })
            })
            .or_else(|| value.clone()),
            _ => value.clone(),
        })
        .collect::<Vec<_>>();
    let Some(values) = elems.into_iter().collect::<Option<Vec<_>>>() else {
        return false;
    };
    let value = native_static_list_from_values(&values, String::new())
        .unwrap_or(NativeStraightlineValue::ArgList { elements: values });
    static_regs[instr.a() as usize] = Some(value);
    let _ = ir;
    true
}

fn emit_load_nil(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
    ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
    ir.push_str(&format!("  store i64 0, ptr %r{}.present.slot\n", instr.a()));
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_load_int(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    int_consts: &[i64],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
) -> bool {
    let Some(value) = int_consts.get(instr.bx() as usize) else {
        return false;
    };
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
    ir.push_str(&format!("  store i64 1, ptr %r{}.present.slot\n", instr.a()));
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_load_float(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    float_consts: &[f64],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
) -> bool {
    let Some(value) = float_consts.get(instr.bx() as usize) else {
        return false;
    };
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(llvm_float_literal(*value)));
    ir.push_str(&format!(
        "  store double {}, ptr %r{}.slot\n",
        llvm_float_literal(*value),
        instr.a()
    ));
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_load_bool(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
    }
    let value = i64::from(instr.b() != 0);
    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(value.to_string()));
    emit_branch_to_next(ir, pc, code.len());
    true
}

#[allow(clippy::too_many_arguments)]
fn emit_move(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    global_names: &[String],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return false;
    }
    let Some(moved) = emit_dynamic_int_list_move(ir, static_regs, code, heap_values, pc, instr, tmp_index) else {
        return false;
    };
    if moved {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if let Some(
        value @ (NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)),
    ) = static_regs.get(instr.b() as usize).and_then(Clone::clone)
    {
        let ptr = next_tmp(tmp_index);
        ir.push_str(&format!("  {ptr} = load ptr, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  store ptr {ptr}, ptr %r{}.slot\n", instr.a()));
        ir.push_str(&format!("  store i64 1, ptr %r{}.present.slot\n", instr.a()));
        static_regs[instr.a() as usize] = Some(value);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if let Some(NativeStraightlineValue::F64(value)) = static_regs.get(instr.b() as usize).and_then(Clone::clone) {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(value.clone()));
        ir.push_str(&format!("  store double {value}, ptr %r{}.slot\n", instr.a()));
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if matches!(
        static_regs.get(instr.b() as usize).and_then(|value| value.as_ref()),
        Some(
            NativeStraightlineValue::Builtin(_)
                | NativeStraightlineValue::Module(_)
                | NativeStraightlineValue::Function(_)
                | NativeStraightlineValue::Closure { .. }
                | NativeStraightlineValue::Channel { .. }
                | NativeStraightlineValue::List { .. }
                | NativeStraightlineValue::Map { .. }
                | NativeStraightlineValue::DynamicMap { .. }
                | NativeStraightlineValue::DynamicList { .. }
                | NativeStraightlineValue::Object { .. }
                | NativeStraightlineValue::ArgList { .. }
        )
    ) {
        static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if let Some(
        value @ (NativeStraightlineValue::I64(_) | NativeStraightlineValue::Bool(_) | NativeStraightlineValue::F64(_)),
    ) = static_regs.get(instr.b() as usize).and_then(Clone::clone)
    {
        emit_static_scalar_value_store_if_needed(ir, instr.a(), &value);
        static_regs[instr.a() as usize] = Some(value);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let heap_kind = local_heap_kind_before(code, heap_values, pc, instr.b());
    let local_kind = if register_last_written_by_call_before(code, pc, instr.b()) {
        None
    } else {
        local_register_kind_before(code, pc, instr.b())
    };
    let kind = if heap_kind == Some(NativeScalarKind::StrPtr) {
        heap_kind
    } else if matches!(
        local_kind,
        Some(NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr)
    ) {
        local_kind
    } else {
        facts.register_kind_before(pc, instr.b()).or(local_kind)
    };
    if let Some(kind) = kind {
        static_regs[instr.a() as usize] = static_regs
            .get(instr.b() as usize)
            .and_then(Clone::clone)
            .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
            .or_else(|| {
                static_register_value_trusted_before(code, pc, instr.b())
                    .then(|| local_static_string_before(code, strings, pc, instr.b()))
                    .flatten()
            })
            .or_else(|| {
                (kind == NativeScalarKind::I64 && static_register_value_trusted_before(code, pc, instr.b()))
                    .then(|| local_static_i64_before(code, int_consts, pc, instr.b()))
                    .flatten()
            })
            .or_else(|| {
                local_static_iter_zip_before(
                    global_names,
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    pc,
                    instr.b(),
                    static_regs,
                )
            });
        if kind == NativeScalarKind::MaybeStrPtr {
            static_regs[instr.a() as usize] = None;
        }
        let value = next_tmp(tmp_index);
        let ty = kind.llvm_type();
        ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
        if kind == NativeScalarKind::StrPtr {
            ir.push_str(&format!("  store i64 1, ptr %r{}.present.slot\n", instr.a()));
        }
        if matches!(kind, NativeScalarKind::MaybeI64 | NativeScalarKind::MaybeStrPtr) {
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.b()));
            ir.push_str(&format!("  store i64 {present}, ptr %r{}.present.slot\n", instr.a()));
        }
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let Some(value) = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
        .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, instr.b()))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, instr.b()))
    else {
        return false;
    };
    emit_static_scalar_value_store_if_needed(ir, instr.a(), &value);
    static_regs[instr.a() as usize] = Some(value);
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn register_last_written_by_call_before(code: &[Instr], pc: usize, reg: u8) -> bool {
    code.iter()
        .take(pc)
        .rev()
        .find(|instr| instr.a() == reg)
        .is_some_and(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect | Opcode::CallNamed))
}

fn emit_to_string(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return false;
    }
    let Some(value) = text_value_from_reg(
        ir,
        instr.b(),
        facts.register_kind_before(pc, instr.b()),
        static_regs,
        tmp_index,
    ) else {
        return false;
    };
    static_regs[instr.a() as usize] = Some(value);
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_concat_string(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    let Some(lhs) = text_value_from_reg(
        ir,
        instr.b(),
        facts.register_kind_before(pc, instr.b()),
        static_regs,
        tmp_index,
    ) else {
        return false;
    };
    let Some(rhs) = text_value_from_reg(
        ir,
        instr.c(),
        facts.register_kind_before(pc, instr.c()),
        static_regs,
        tmp_index,
    ) else {
        return false;
    };
    let Some(value) = concat_text_values(lhs, rhs) else {
        return false;
    };
    if let NativeStraightlineValue::Text(parts) = &value
        && let Some(text) = native_static_text_string(parts)
    {
        let symbol = format!("@lk_concat_str_{pc}");
        extra_globals.push_str(&llvm_string_constant(&symbol, &text));
        ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
            symbol,
            len: text.chars().count(),
            key_kind: crate::llvm::straightline_value::native_runtime_string_key_kind(&text),
            value: text,
        });
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_dynamic_text_to_reg(ir, extra_globals, instr.a(), &value, pc, tmp_index).is_some() {
        static_regs[instr.a() as usize] = Some(value);
    } else {
        static_regs[instr.a() as usize] = Some(value);
    }
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_dynamic_text_to_reg(
    ir: &mut String,
    extra_globals: &mut String,
    dst: u8,
    value: &NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<String> {
    let NativeStraightlineValue::Text(parts) = value else {
        return None;
    };
    if native_static_text_string(parts).is_some() {
        return None;
    }
    let out = next_tmp(tmp_index);
    let offset_slot = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {out} = getelementptr [4096 x i8], ptr %r{dst}.text.buf, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {offset_slot} = call ptr @malloc(i64 8)\n"));
    ir.push_str(&format!("  store i64 0, ptr {offset_slot}\n"));
    for (index, part) in parts.iter().enumerate() {
        emit_text_part_to_buffer(ir, extra_globals, dst, pc, index, part, &out, &offset_slot, tmp_index)?;
    }
    let offset = next_tmp(tmp_index);
    let zero_slot = next_tmp(tmp_index);
    ir.push_str(&format!("  {offset} = load i64, ptr {offset_slot}\n"));
    ir.push_str(&format!("  {zero_slot} = getelementptr i8, ptr {out}, i64 {offset}\n"));
    ir.push_str(&format!("  store i8 0, ptr {zero_slot}\n"));
    ir.push_str(&format!("  store ptr {out}, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    Some(out)
}

#[allow(clippy::too_many_arguments)]
fn emit_text_part_to_buffer(
    ir: &mut String,
    extra_globals: &mut String,
    dst: u8,
    pc: usize,
    index: usize,
    part: &NativeTextPart,
    out: &str,
    offset_slot: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    match part {
        NativeTextPart::String { symbol, value } => {
            let symbol = if symbol.is_empty() {
                let generated = format!("@lk_text_part_{pc}_{index}");
                extra_globals.push_str(&llvm_string_constant(&generated, value));
                generated
            } else {
                symbol.clone()
            };
            emit_append_ptr_to_buffer(ir, pc, index, &symbol, out, offset_slot, tmp_index);
        }
        NativeTextPart::StrPtr(value) => emit_append_ptr_to_buffer(ir, pc, index, value, out, offset_slot, tmp_index),
        NativeTextPart::I64(value) => {
            emit_append_formatted_to_buffer(ir, dst, value, "@lk_i64_raw_fmt", out, offset_slot, tmp_index);
        }
        NativeTextPart::F64(value) => {
            emit_append_formatted_to_buffer(ir, dst, value, "@lk_f64_raw_fmt", out, offset_slot, tmp_index);
        }
        NativeTextPart::Bool(value) => {
            let cond = next_tmp(tmp_index);
            let ptr = next_tmp(tmp_index);
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
            ir.push_str(&format!(
                "  {ptr} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            emit_append_ptr_to_buffer(ir, pc, index, &ptr, out, offset_slot, tmp_index);
        }
        NativeTextPart::Nil => emit_append_ptr_to_buffer(ir, pc, index, "@lk_nil_text", out, offset_slot, tmp_index),
    }
    Some(())
}

fn emit_append_formatted_to_buffer(
    ir: &mut String,
    dst: u8,
    value: &str,
    fmt: &str,
    out: &str,
    offset_slot: &str,
    tmp_index: &mut usize,
) {
    let offset = next_tmp(tmp_index);
    let dst_ptr = next_tmp(tmp_index);
    let remaining = next_tmp(tmp_index);
    let written_i = next_tmp(tmp_index);
    let written = next_tmp(tmp_index);
    let next = next_tmp(tmp_index);
    ir.push_str(&format!("  {offset} = load i64, ptr {offset_slot}\n"));
    ir.push_str(&format!("  {dst_ptr} = getelementptr i8, ptr {out}, i64 {offset}\n"));
    ir.push_str(&format!("  {remaining} = sub i64 4096, {offset}\n"));
    let llvm_ty = if fmt == "@lk_f64_raw_fmt" { "double" } else { "i64" };
    ir.push_str(&format!(
        "  {written_i} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {dst_ptr}, i64 {remaining}, ptr {fmt}, {llvm_ty} {value})\n"
    ));
    ir.push_str(&format!("  {written} = sext i32 {written_i} to i64\n"));
    ir.push_str(&format!("  {next} = add i64 {offset}, {written}\n"));
    ir.push_str(&format!("  store i64 {next}, ptr {offset_slot}\n"));
    ir.push_str(&format!("  store ptr {out}, ptr %r{dst}.slot\n"));
}

fn emit_append_ptr_to_buffer(
    ir: &mut String,
    pc: usize,
    index: usize,
    src: &str,
    out: &str,
    offset_slot: &str,
    tmp_index: &mut usize,
) {
    let offset = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    ir.push_str(&format!("  {offset} = load i64, ptr {offset_slot}\n"));
    ir.push_str(&format!("  {len} = call i64 @strlen(ptr {src})\n"));
    emit_copy_loop(ir, pc, index, src, out, &offset, &len, tmp_index);
    let next = next_tmp(tmp_index);
    ir.push_str(&format!("  {next} = add i64 {offset}, {len}\n"));
    ir.push_str(&format!("  store i64 {next}, ptr {offset_slot}\n"));
}

fn emit_copy_loop(
    ir: &mut String,
    pc: usize,
    index: usize,
    src: &str,
    dst: &str,
    dst_offset: &str,
    len: &str,
    tmp_index: &mut usize,
) {
    let idx_slot = next_tmp(tmp_index);
    let loop_label = format!("lk_text_{pc}_{index}_loop_{}", *tmp_index);
    let body_label = format!("lk_text_{pc}_{index}_body_{}", *tmp_index);
    let done_label = format!("lk_text_{pc}_{index}_done_{}", *tmp_index);
    ir.push_str(&format!("  {idx_slot} = call ptr @malloc(i64 8)\n"));
    ir.push_str(&format!("  store i64 0, ptr {idx_slot}\n"));
    ir.push_str(&format!("  br label %{loop_label}\n"));
    ir.push_str(&format!("{loop_label}:\n"));
    let idx = next_tmp(tmp_index);
    let keep_going = next_tmp(tmp_index);
    ir.push_str(&format!("  {idx} = load i64, ptr {idx_slot}\n"));
    ir.push_str(&format!("  {keep_going} = icmp ult i64 {idx}, {len}\n"));
    ir.push_str(&format!(
        "  br i1 {keep_going}, label %{body_label}, label %{done_label}\n"
    ));
    ir.push_str(&format!("{body_label}:\n"));
    let src_slot = next_tmp(tmp_index);
    let ch = next_tmp(tmp_index);
    let shifted = next_tmp(tmp_index);
    let dst_slot = next_tmp(tmp_index);
    let next = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_slot} = getelementptr i8, ptr {src}, i64 {idx}\n"));
    ir.push_str(&format!("  {ch} = load i8, ptr {src_slot}\n"));
    ir.push_str(&format!("  {shifted} = add i64 {dst_offset}, {idx}\n"));
    ir.push_str(&format!("  {dst_slot} = getelementptr i8, ptr {dst}, i64 {shifted}\n"));
    ir.push_str(&format!("  store i8 {ch}, ptr {dst_slot}\n"));
    ir.push_str(&format!("  {next} = add i64 {idx}, 1\n"));
    ir.push_str(&format!("  store i64 {next}, ptr {idx_slot}\n"));
    ir.push_str(&format!("  br label %{loop_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
}

fn emit_float_arithmetic(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    if let (Some(NativeStraightlineValue::F64(lhs)), Some(NativeStraightlineValue::F64(rhs))) = (
        static_regs.get(instr.b() as usize).and_then(Clone::clone),
        static_regs.get(instr.c() as usize).and_then(Clone::clone),
    ) && let Some(value) = native_static_f64_binary(&lhs, &rhs, instr.opcode())
    {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(value.clone()));
        ir.push_str(&format!("  store double {value}, ptr %r{}.slot\n", instr.a()));
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    static_regs[instr.a() as usize] = None;
    let Some(lhs) = facts.register_kind_before(pc, instr.b()) else {
        return false;
    };
    let Some(rhs) = facts.register_kind_before(pc, instr.c()) else {
        return false;
    };
    if !matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::F64)
        || !matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::F64)
    {
        return false;
    }
    emit_f64_binary_block(ir, instr, lhs, rhs, "", tmp_index);
    emit_branch_to_next(ir, pc, code.len());
    true
}
