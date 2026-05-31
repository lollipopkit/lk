use crate::{
    llvm::{
        const_display::llvm_string_constant,
        ir_text::{emit_branch_to_next, llvm_float_literal, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::{
                concat_text_values, emit_static_scalar_value_store_if_needed, local_heap_kind_before,
                local_register_kind_before, local_static_container_before, local_static_i64_before,
                static_register_value_trusted_before, three_regs_in_bounds,
            },
            contains::{
                emit_dynamic_int_list_move, local_static_heap_const_before, local_static_i64_value_before,
                local_static_index_value_before, local_static_iter_zip_before, local_static_map_rest_before,
                local_static_string_before, text_value_from_trusted_reg,
            },
            emit::emit_f64_binary_block,
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{
            NativeListElementKind, NativeStraightlineValue, native_static_f64_binary, native_static_list_from_values,
            native_static_text_string,
        },
    },
    vm::{ConstHeapValue32Data, Instr32, Opcode32},
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
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    match instr.opcode() {
        Opcode32::NewList => emit_new_list(
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
        Opcode32::LoadNil => emit_load_nil(ir, static_regs, code, pc, instr, register_count),
        Opcode32::LoadInt => emit_load_int(ir, static_regs, int_consts, code, pc, instr, register_count),
        Opcode32::LoadFloat => emit_load_float(ir, static_regs, float_consts, code, pc, instr, register_count),
        Opcode32::LoadBool => emit_load_bool(ir, static_regs, code, pc, instr, register_count),
        Opcode32::Move => emit_move(
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
        Opcode32::ToString => emit_to_string(ir, static_regs, code, pc, instr, register_count, facts, tmp_index),
        Opcode32::ConcatString => emit_concat_string(
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
        Opcode32::AddFloat | Opcode32::SubFloat | Opcode32::MulFloat | Opcode32::DivFloat | Opcode32::ModFloat => {
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
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
        static_regs[instr.a() as usize] = Some(value);
        emit_branch_to_next(ir, pc, code.len());
        return true;
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
        .map(|reg| match static_regs.get(reg).cloned().flatten()? {
            value @ NativeStraightlineValue::Object { .. } => Some(value),
            _ => None,
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
        _ => facts.register_kind_before(pc, i as u8) == Some(NativeScalarKind::I64),
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

fn emit_dynamic_i64_list(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    start: usize,
    end: usize,
    pc: usize,
    instr: Instr32,
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
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    start: usize,
    end: usize,
    pc: usize,
    instr: Instr32,
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
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
        )
    ) {
        static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let heap_kind = local_heap_kind_before(code, heap_values, pc, instr.b());
    let local_kind = local_register_kind_before(code, pc, instr.b());
    let kind = if heap_kind == Some(NativeScalarKind::StrPtr) {
        heap_kind
    } else if local_kind == Some(NativeScalarKind::StrPtr) {
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
        let value = next_tmp(tmp_index);
        let ty = kind.llvm_type();
        ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
        if kind == NativeScalarKind::MaybeI64 {
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

fn emit_to_string(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return false;
    }
    let Some(value) = text_value_from_trusted_reg(
        ir,
        code,
        pc,
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
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    let Some(lhs) = text_value_from_trusted_reg(
        ir,
        code,
        pc,
        instr.b(),
        facts.register_kind_before(pc, instr.b()),
        static_regs,
        tmp_index,
    ) else {
        return false;
    };
    let Some(rhs) = text_value_from_trusted_reg(
        ir,
        code,
        pc,
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
    static_regs[instr.a() as usize] = Some(value);
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_float_arithmetic(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
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
