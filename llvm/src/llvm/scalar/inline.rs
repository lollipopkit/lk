use crate::llvm::{
    callee_eval::native_straightline_function_return,
    const_display::llvm_string_constant,
    ir_text::{llvm_float_literal, native_label, native_relative_target, next_tmp, reg_in_bounds},
    output::emit_native_builtin_call,
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeStraightlineValue, native_static_f64_binary, native_static_global,
        native_static_index, native_static_load_cell, native_static_set_index, native_static_store_cell,
        native_straightline_heap_const_value,
    },
};
use crate::vm::{ConstHeapValueData, FunctionData, Instr, ModuleArtifact, Opcode};

use super::{
    block_helpers::{
        clear_control_flow_static_values, concat_text_values, control_flow_static_boundaries,
        emit_inline_branch_to_next, emit_inline_i64_add_mul_block, emit_inline_i64_add2_block,
        emit_inline_i64_binary_block, emit_inline_scalar_arg_stores, emit_inline_scalar_equality_block,
        emit_inline_scalar_ordered_comparison_block, emit_inline_string_ptr_equality_block,
        emit_mixed_numeric_int_opcode_block, emit_static_formatted_print, emit_static_string_i64_map_get,
        i64_slot_kind, inline_native_label, inline_text_value_from_reg, native_static_string, scalar_named_call_args,
        static_call_args, static_string_i64_map_supported, static_string_value_trusted_at_call,
        store_native_inline_scalar_value, three_regs_in_bounds,
    },
    emit::emit_f64_binary_block,
    facts::{NativeScalarFacts, NativeScalarKind, native_scalar_block_facts_with_initial},
};

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn emit_inline_direct_scalar_call(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    callee: &FunctionData,
    call_pc: usize,
    instr: Instr,
    caller_register_count: usize,
    global_count: usize,
    global_names: &[String],
    caller_code: &[Instr],
    caller_static_regs: &[Option<NativeStraightlineValue>],
    caller_static_globals: &[Option<NativeStraightlineValue>],
    caller_facts: &NativeScalarFacts,
    tmp_index: &mut usize,
    caller_code_len: usize,
) -> Option<()> {
    emit_inline_static_scalar_call(
        ir,
        extra_globals,
        artifact,
        callee,
        call_pc,
        instr,
        caller_register_count,
        global_count,
        global_names,
        caller_code,
        caller_static_regs,
        caller_static_globals,
        caller_facts,
        &[],
        tmp_index,
        caller_code_len,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn emit_inline_static_scalar_call(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    callee: &FunctionData,
    call_pc: usize,
    instr: Instr,
    caller_register_count: usize,
    global_count: usize,
    global_names: &[String],
    caller_code: &[Instr],
    caller_static_regs: &[Option<NativeStraightlineValue>],
    caller_static_globals: &[Option<NativeStraightlineValue>],
    caller_facts: &NativeScalarFacts,
    callee_captures: &[NativeStraightlineValue],
    tmp_index: &mut usize,
    caller_code_len: usize,
) -> Option<()> {
    if callee.capture_count as usize != callee_captures.len() || instr.c() as u16 != callee.param_count {
        return None;
    }
    let arg_start = instr.a() as usize + 1;
    let arg_end = arg_start.checked_add(instr.c() as usize)?;
    if arg_end > caller_register_count || arg_end > caller_static_regs.len() {
        return None;
    }
    let callee_code = callee
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut callee_kinds = vec![None; callee.register_count as usize];
    let mut callee_static_regs = vec![None; callee.register_count as usize];
    for arg in 0..instr.c() as usize {
        let caller_reg = instr.a().checked_add(1)?.checked_add(arg as u8)?;
        let static_value = caller_static_regs.get(arg_start + arg).and_then(Clone::clone);
        if let Some(kind) = caller_facts.register_kind_before(call_pc, caller_reg) {
            callee_kinds[arg] = Some(kind);
        } else if static_value.is_none() {
            return None;
        }
        callee_static_regs[arg] = static_value;
        if matches!(
            callee_static_regs[arg],
            Some(NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_))
        ) && !static_string_value_trusted_at_call(caller_code, call_pc, caller_reg)
        {
            callee_static_regs[arg] = None;
        }
    }
    let caller_global_kinds = caller_facts.global_kinds_before(call_pc)?;
    let callee_facts = native_scalar_block_facts_with_initial(
        callee.register_count as usize,
        global_count,
        global_names,
        &callee.consts.ints,
        &callee.consts.strings,
        &callee.consts.heap_values,
        &callee_code,
        callee_kinds,
        callee_static_regs.clone(),
        caller_global_kinds.to_vec(),
        caller_static_globals.to_vec(),
        Some(&artifact.module.functions),
        callee_captures,
        0,
        &[],
    )?;

    emit_inline_scalar_arg_stores(ir, caller_facts, call_pc, instr, tmp_index)?;
    ir.push_str(&format!("  br label %call{call_pc}.bb0\n\n"));
    emit_inline_direct_scalar_blocks(
        ir,
        extra_globals,
        artifact,
        callee,
        &callee_code,
        &callee_facts,
        call_pc,
        instr.a(),
        global_count,
        global_names,
        &callee_static_regs,
        caller_static_globals,
        callee_captures,
        tmp_index,
        caller_code_len,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_inline_direct_scalar_blocks(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    callee: &FunctionData,
    code: &[Instr],
    facts: &NativeScalarFacts,
    call_pc: usize,
    dst: u8,
    global_count: usize,
    global_names: &[String],
    initial_static_regs: &[Option<NativeStraightlineValue>],
    inherited_static_globals: &[Option<NativeStraightlineValue>],
    callee_captures: &[NativeStraightlineValue],
    tmp_index: &mut usize,
    caller_code_len: usize,
) -> Option<()> {
    let register_count = callee.register_count as usize;
    let mut static_regs: Vec<Option<NativeStraightlineValue>> = initial_static_regs.to_vec();
    if static_regs.len() != register_count {
        return None;
    }
    let mut static_globals = inherited_static_globals.to_vec();
    let static_boundaries = control_flow_static_boundaries(code);
    for (pc, instr) in code.iter().copied().enumerate() {
        if static_boundaries.get(pc).copied().unwrap_or(false) {
            clear_control_flow_static_values(&mut static_regs);
        }
        ir.push_str(&format!("call{call_pc}.bb{pc}:\n"));
        match instr.opcode() {
            Opcode::LoadString => {
                let value = callee.consts.strings.get(instr.bx() as usize)?;
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                let symbol = format!("@lk_call{call_pc}_str_{pc}");
                extra_globals.push_str(&llvm_string_constant(&symbol, value));
                static_regs[instr.a() as usize] = Some(native_static_string(value, symbol.clone()));
                ir.push_str(&format!(
                    "  store ptr {symbol}, ptr %call{call_pc}.r{}.slot\n",
                    instr.a()
                ));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadHeapConst => {
                let value = callee.consts.heap_values.get(instr.bx() as usize)?;
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                if let ConstHeapValueData::LongString(value) = value {
                    let symbol = format!("@lk_call{call_pc}_heap_str_{pc}");
                    extra_globals.push_str(&llvm_string_constant(&symbol, value));
                    static_regs[instr.a() as usize] = Some(native_static_string(value, symbol.clone()));
                    ir.push_str(&format!(
                        "  store ptr {symbol}, ptr %call{call_pc}.r{}.slot\n",
                        instr.a()
                    ));
                } else {
                    static_regs[instr.a() as usize] =
                        Some(native_straightline_heap_const_value(call_pc, instr.bx(), value)?);
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadCapture => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                let value = callee_captures.get(instr.bx() as usize)?.clone();
                if store_native_inline_scalar_value(
                    ir,
                    extra_globals,
                    &mut static_regs,
                    call_pc,
                    instr.a(),
                    value.clone(),
                    tmp_index,
                )
                .is_none()
                {
                    static_regs[instr.a() as usize] = Some(value);
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::StoreCellVal => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                static_regs[instr.a() as usize] = if let (Some(cell), Some(value)) = (
                    static_regs.get(instr.a() as usize).and_then(Clone::clone),
                    static_regs.get(instr.b() as usize).and_then(Clone::clone),
                ) {
                    native_static_store_cell(cell, value)
                } else {
                    None
                };
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadCellVal => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                let value = static_regs
                    .get(instr.b() as usize)
                    .and_then(Clone::clone)
                    .and_then(native_static_load_cell)?;
                if store_native_inline_scalar_value(
                    ir,
                    extra_globals,
                    &mut static_regs,
                    call_pc,
                    instr.a(),
                    value.clone(),
                    tmp_index,
                )
                .is_none()
                {
                    static_regs[instr.a() as usize] = Some(value);
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadNil => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                static_regs[instr.a() as usize] = None;
                ir.push_str(&format!("  store i64 0, ptr %call{call_pc}.r{}.slot\n", instr.a()));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadInt => {
                let value = callee.consts.ints.get(instr.bx() as usize)?;
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                static_regs[instr.a() as usize] = None;
                ir.push_str(&format!(
                    "  store i64 {value}, ptr %call{call_pc}.r{}.slot\n",
                    instr.a()
                ));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadFloat => {
                let value = callee.consts.floats.get(instr.bx() as usize)?;
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(llvm_float_literal(*value)));
                ir.push_str(&format!(
                    "  store double {}, ptr %call{call_pc}.r{}.slot\n",
                    llvm_float_literal(*value),
                    instr.a()
                ));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::LoadBool => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                static_regs[instr.a() as usize] = None;
                ir.push_str(&format!(
                    "  store i64 {}, ptr %call{call_pc}.r{}.slot\n",
                    i64::from(instr.b() != 0),
                    instr.a()
                ));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                if let (Some(NativeStraightlineValue::F64(lhs)), Some(NativeStraightlineValue::F64(rhs))) = (
                    static_regs.get(instr.b() as usize).and_then(Clone::clone),
                    static_regs.get(instr.c() as usize).and_then(Clone::clone),
                ) && let Some(value) = native_static_f64_binary(&lhs, &rhs, instr.opcode())
                {
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(value.clone()));
                    ir.push_str(&format!(
                        "  store double {value}, ptr %call{call_pc}.r{}.slot\n",
                        instr.a()
                    ));
                    emit_inline_branch_to_next(ir, call_pc, pc, code.len());
                    continue;
                }
                static_regs[instr.a() as usize] = None;
                let lhs = facts.register_kind_before(pc, instr.b())?;
                let rhs = facts.register_kind_before(pc, instr.c())?;
                if !matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::F64)
                    || !matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::F64)
                {
                    return None;
                }
                emit_f64_binary_block(ir, instr, lhs, rhs, &format!("call{call_pc}."), tmp_index);
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::AddInt
            | Opcode::SubInt
            | Opcode::MulInt
            | Opcode::DivInt
            | Opcode::ModInt
            | Opcode::MinInt
            | Opcode::MaxInt
            | Opcode::AddMulInt
            | Opcode::Add2Int
            | Opcode::MidInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                static_regs[instr.a() as usize] = None;
                if matches!(instr.opcode(), Opcode::AddMulInt | Opcode::Add2Int) {
                    let acc = facts.register_kind_before(pc, instr.a())?;
                    let lhs = facts.register_kind_before(pc, instr.b())?;
                    let rhs = facts.register_kind_before(pc, instr.c())?;
                    if !i64_slot_kind(acc) || !i64_slot_kind(lhs) || !i64_slot_kind(rhs) {
                        return None;
                    }
                    if instr.opcode() == Opcode::AddMulInt {
                        emit_inline_i64_add_mul_block(ir, call_pc, instr, tmp_index);
                    } else {
                        emit_inline_i64_add2_block(ir, call_pc, instr, tmp_index);
                    }
                    emit_inline_branch_to_next(ir, call_pc, pc, code.len());
                    continue;
                }
                let lhs = facts.register_kind_before(pc, instr.b())?;
                let rhs = facts.register_kind_before(pc, instr.c())?;
                if i64_slot_kind(lhs) && i64_slot_kind(rhs) {
                    emit_inline_i64_binary_block(ir, call_pc, instr, tmp_index);
                } else if matches!(instr.opcode(), Opcode::MinInt | Opcode::MaxInt) {
                    return None;
                } else if lhs.is_numeric() && rhs.is_numeric() {
                    emit_mixed_numeric_int_opcode_block(ir, &format!("call{call_pc}."), instr, lhs, rhs, tmp_index);
                } else {
                    return None;
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                static_regs[instr.a() as usize] = None;
                let lhs = crate::llvm::ir_text::next_tmp(tmp_index);
                let out = crate::llvm::ir_text::next_tmp(tmp_index);
                let op = match instr.opcode() {
                    Opcode::AddIntI => "add",
                    Opcode::MulIntI => "mul",
                    Opcode::ModIntI => "srem",
                    _ => unreachable!("opcode matched above"),
                };
                if instr.opcode() == Opcode::ModIntI && instr.sc() == 0 {
                    return None;
                }
                ir.push_str(&format!("  {lhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.b()));
                ir.push_str(&format!("  {out} = {op} i64 {lhs}, {}\n", instr.sc()));
                ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
                ir.push_str(&format!(
                    "  store i64 1, ptr %call{call_pc}.r{}.present.slot\n",
                    instr.a()
                ));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::ToString => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                let value = inline_text_value_from_reg(
                    ir,
                    call_pc,
                    instr.b(),
                    facts.register_kind_before(pc, instr.b()),
                    &static_regs,
                    tmp_index,
                )?;
                static_regs[instr.a() as usize] = Some(value);
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::ConcatString => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                let lhs = inline_text_value_from_reg(
                    ir,
                    call_pc,
                    instr.b(),
                    facts.register_kind_before(pc, instr.b()),
                    &static_regs,
                    tmp_index,
                )?;
                let rhs = inline_text_value_from_reg(
                    ir,
                    call_pc,
                    instr.c(),
                    facts.register_kind_before(pc, instr.c()),
                    &static_regs,
                    tmp_index,
                )?;
                static_regs[instr.a() as usize] = Some(concat_text_values(lhs, rhs)?);
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::GetIndex | Opcode::GetList => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                let target = static_regs.get(instr.b() as usize).and_then(Clone::clone)?;
                if let NativeStraightlineValue::DynamicList {
                    id,
                    element: NativeListElementKind::I64,
                } = &target
                {
                    let id = *id;
                    let idx = next_tmp(tmp_index);
                    let slot = next_tmp(tmp_index);
                    let val = next_tmp(tmp_index);
                    ir.push_str(&format!("  {idx} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.c()));
                    ir.push_str(&format!(
                        "  {slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {idx}\n"
                    ));
                    ir.push_str(&format!("  {val} = load i64, ptr {slot}\n"));
                    ir.push_str(&format!("  store i64 {val}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = None;
                } else if let NativeStraightlineValue::Map { entries, .. } = &target
                    && (facts.register_kind_before(pc, instr.c()) == Some(NativeScalarKind::StrPtr)
                        || !static_string_value_trusted_at_call(code, pc, instr.c()))
                    && static_string_i64_map_supported(entries)
                {
                    emit_static_string_i64_map_get(
                        ir,
                        extra_globals,
                        entries,
                        &format!("call{call_pc}."),
                        instr.a(),
                        instr.c(),
                        tmp_index,
                    )?;
                    static_regs[instr.a() as usize] = None;
                } else {
                    let key = static_regs.get(instr.c() as usize).and_then(Clone::clone)?;
                    let value = native_static_index(target, key, String::new())?;
                    if store_native_inline_scalar_value(
                        ir,
                        extra_globals,
                        &mut static_regs,
                        call_pc,
                        instr.a(),
                        value,
                        tmp_index,
                    )
                    .is_none()
                    {
                        return None;
                    }
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::Move => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                if let Some(kind) = facts.register_kind_before(pc, instr.b()) {
                    static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
                    let value = next_tmp(tmp_index);
                    let ty = kind.llvm_type();
                    ir.push_str(&format!(
                        "  {value} = load {ty}, ptr %call{call_pc}.r{}.slot\n",
                        instr.b()
                    ));
                    ir.push_str(&format!(
                        "  store {ty} {value}, ptr %call{call_pc}.r{}.slot\n",
                        instr.a()
                    ));
                    if kind == NativeScalarKind::MaybeI64 {
                        let present = next_tmp(tmp_index);
                        ir.push_str(&format!(
                            "  {present} = load i64, ptr %call{call_pc}.r{}.present.slot\n",
                            instr.b()
                        ));
                        ir.push_str(&format!(
                            "  store i64 {present}, ptr %call{call_pc}.r{}.present.slot\n",
                            instr.a()
                        ));
                    }
                    emit_inline_branch_to_next(ir, call_pc, pc, code.len());
                    continue;
                }
                static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::CmpInt | Opcode::CmpNeInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                let lhs = facts.register_kind_before(pc, instr.b())?;
                let rhs = facts.register_kind_before(pc, instr.c())?;
                if lhs == rhs && lhs == NativeScalarKind::StrPtr {
                    emit_inline_string_ptr_equality_block(ir, call_pc, instr, tmp_index);
                } else {
                    emit_inline_scalar_equality_block(ir, call_pc, instr, lhs, rhs, tmp_index)?;
                }
                static_regs[instr.a() as usize] = None;
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::CmpLtInt | Opcode::CmpLeInt | Opcode::CmpGtInt | Opcode::CmpGeInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                let lhs = facts.register_kind_before(pc, instr.b())?;
                let rhs = facts.register_kind_before(pc, instr.c())?;
                if !i64_slot_kind(lhs) || !i64_slot_kind(rhs) {
                    return None;
                }
                emit_inline_scalar_ordered_comparison_block(ir, call_pc, instr, tmp_index);
                static_regs[instr.a() as usize] = None;
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::Not => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                let kind = facts.register_kind_before(pc, instr.b())?;
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(tmp_index);
                        let cond = next_tmp(tmp_index);
                        let out = next_tmp(tmp_index);
                        ir.push_str(&format!(
                            "  {value} = load i64, ptr %call{call_pc}.r{}.slot
",
                            instr.b()
                        ));
                        ir.push_str(&format!(
                            "  {cond} = icmp eq i64 {value}, 0
"
                        ));
                        ir.push_str(&format!(
                            "  {out} = zext i1 {cond} to i64
"
                        ));
                        ir.push_str(&format!(
                            "  store i64 {out}, ptr %call{call_pc}.r{}.slot
",
                            instr.a()
                        ));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!(
                            "  store i64 1, ptr %call{call_pc}.r{}.slot
",
                            instr.a()
                        ));
                    }
                    NativeScalarKind::I64
                    | NativeScalarKind::F64
                    | NativeScalarKind::StrPtr
                    | NativeScalarKind::MaybeI64
                    | NativeScalarKind::MaybeStrPtr => {
                        let value = next_tmp(tmp_index);
                        let cond = next_tmp(tmp_index);
                        let out = next_tmp(tmp_index);
                        ir.push_str(&format!(
                            "  {value} = load i64, ptr %call{call_pc}.r{}.slot
",
                            instr.b()
                        ));
                        ir.push_str(&format!(
                            "  {cond} = icmp eq i64 {value}, 0
"
                        ));
                        ir.push_str(&format!(
                            "  {out} = zext i1 {cond} to i64
"
                        ));
                        ir.push_str(&format!(
                            "  store i64 {out}, ptr %call{call_pc}.r{}.slot
",
                            instr.a()
                        ));
                    }
                }
                static_regs[instr.a() as usize] = None;
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            opcode if opcode.is_compare_test() => {
                if !reg_in_bounds(register_count, instr.a())
                    || (!opcode.is_int_immediate_compare_test() && !reg_in_bounds(register_count, instr.b()))
                {
                    return None;
                }
                let jmp = code.get(pc + 1).copied()?;
                if jmp.opcode() != Opcode::Jmp {
                    return None;
                }
                let taken = native_relative_target(pc + 1, jmp.sj_arg(), code.len())?;
                let fallthrough = pc + 2;
                let pred = compare_test_i64_pred(instr.opcode())?;
                let lhs = next_tmp(tmp_index);
                let cond = next_tmp(tmp_index);
                let branch_cond = next_tmp(tmp_index);
                ir.push_str(&format!("  {lhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.a()));
                let rhs = if opcode.is_int_immediate_compare_test() {
                    i64::from(instr.sc()).to_string()
                } else {
                    let rhs = next_tmp(tmp_index);
                    ir.push_str(&format!("  {rhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.b()));
                    rhs
                };
                ir.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
                let jump_when = if opcode.is_int_immediate_compare_test() {
                    instr.b() != 0
                } else {
                    instr.c() != 0
                };
                if jump_when {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, false\n"));
                } else {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, true\n"));
                }
                ir.push_str(&format!(
                    "  br i1 {branch_cond}, label {}, label {}\n",
                    inline_native_label(call_pc, taken, code.len()),
                    inline_native_label(call_pc, fallthrough, code.len())
                ));
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                let kind = facts.register_kind_before(pc, instr.a())?;
                let fallthrough = pc + 1;
                let relative = match instr.opcode() {
                    Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code.len())?,
                    Opcode::BrFalse | Opcode::BrTrue => native_relative_target(pc, instr.sbx() as i32, code.len())?,
                    _ => return None,
                };
                let truthy_target =
                    if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue {
                        relative
                    } else {
                        fallthrough
                    };
                let falsy_target =
                    if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse {
                        relative
                    } else {
                        fallthrough
                    };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(tmp_index);
                        let cond = next_tmp(tmp_index);
                        ir.push_str(&format!(
                            "  {value} = load i64, ptr %call{call_pc}.r{}.slot\n",
                            instr.a()
                        ));
                        ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                        ir.push_str(&format!(
                            "  br i1 {cond}, label {}, label {}\n",
                            inline_native_label(call_pc, truthy_target, code.len()),
                            inline_native_label(call_pc, falsy_target, code.len())
                        ));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!(
                            "  br label {}\n",
                            inline_native_label(call_pc, falsy_target, code.len())
                        ));
                    }
                    NativeScalarKind::I64
                    | NativeScalarKind::F64
                    | NativeScalarKind::StrPtr
                    | NativeScalarKind::MaybeI64
                    | NativeScalarKind::MaybeStrPtr => {
                        ir.push_str(&format!(
                            "  br label {}\n",
                            inline_native_label(call_pc, truthy_target, code.len())
                        ));
                    }
                }
            }
            Opcode::Jmp => {
                let target = native_relative_target(pc, instr.sj_arg(), code.len())?;
                ir.push_str(&format!(
                    "  br label {}\n",
                    inline_native_label(call_pc, target, code.len())
                ));
            }
            Opcode::GetGlobal => {
                if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
                    return None;
                }
                if let Some(value) = global_names
                    .get(instr.bx() as usize)
                    .and_then(|name| native_static_global(name))
                    .or_else(|| static_globals.get(instr.bx() as usize).and_then(Clone::clone))
                {
                    if store_native_inline_scalar_value(
                        ir,
                        extra_globals,
                        &mut static_regs,
                        call_pc,
                        instr.a(),
                        value.clone(),
                        tmp_index,
                    )
                    .is_none()
                    {
                        static_regs[instr.a() as usize] = Some(value);
                    }
                    emit_inline_branch_to_next(ir, call_pc, pc, code.len());
                    continue;
                }
                let kind = facts.global_kind_before(pc, instr.bx())?;
                static_regs[instr.a() as usize] = None;
                let value = next_tmp(tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!("  {value} = load {ty}, ptr %g{}.slot\n", instr.bx()));
                ir.push_str(&format!(
                    "  store {ty} {value}, ptr %call{call_pc}.r{}.slot\n",
                    instr.a()
                ));
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::SetGlobal => {
                if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
                    return None;
                }
                if let Some(value) = static_regs.get(instr.a() as usize).and_then(Clone::clone) {
                    match &value {
                        NativeStraightlineValue::I64(value) => {
                            ir.push_str(&format!("  store i64 {value}, ptr %g{}.slot\n", instr.bx()));
                        }
                        NativeStraightlineValue::Bool(value) => {
                            ir.push_str(&format!("  store i64 {value}, ptr %g{}.slot\n", instr.bx()));
                        }
                        NativeStraightlineValue::Nil => {
                            ir.push_str(&format!("  store i64 0, ptr %g{}.slot\n", instr.bx()));
                        }
                        _ => return None,
                    }
                    static_globals[instr.bx() as usize] = Some(value);
                    emit_inline_branch_to_next(ir, call_pc, pc, code.len());
                    continue;
                }
                let kind = facts.register_kind_before(pc, instr.a())?;
                let value = next_tmp(tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!(
                    "  {value} = load {ty}, ptr %call{call_pc}.r{}.slot\n",
                    instr.a()
                ));
                ir.push_str(&format!("  store {ty} {value}, ptr %g{}.slot\n", instr.bx()));
                static_globals[instr.bx() as usize] = None;
                static_regs[instr.a() as usize] = None;
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::Call => {
                if instr.a() != instr.b() || !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                if let Some(target @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. })) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                {
                    let (function_index, captures) = static_call_target(target)?;
                    let function_index = u8::try_from(function_index).ok()?;
                    let callee = artifact.module.functions.get(function_index as usize)?;
                    let direct_instr = Instr::abc(Opcode::CallDirect, instr.a(), function_index, instr.c());
                    emit_inline_static_scalar_call(
                        ir,
                        extra_globals,
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
                        tmp_index,
                        code.len(),
                    )?;
                    static_regs[instr.a() as usize] = None;
                    continue;
                }
                let Some(NativeStraightlineValue::Builtin(builtin)) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return None;
                };
                let Some(args) = static_call_args(&static_regs, instr.b(), instr.c()) else {
                    return None;
                };
                // Diverging builtins (panic/abort) terminate the basic block with
                // `unreachable`; we must NOT emit a store or branch after that.
                if matches!(builtin, NativeBuiltin::Panic) {
                    emit_native_builtin_call(ir, builtin, &args, tmp_index)?;
                    static_regs[instr.a() as usize] = None;
                    // The block is terminated by `unreachable`; skip store + branch.
                    continue;
                }
                let value = emit_static_formatted_print(ir, extra_globals, builtin, &args, tmp_index)
                    .or_else(|| emit_native_builtin_call(ir, builtin, &args, tmp_index))?;
                if store_native_inline_scalar_value(
                    ir,
                    extra_globals,
                    &mut static_regs,
                    call_pc,
                    instr.a(),
                    value,
                    tmp_index,
                )
                .is_none()
                {
                    return None;
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::CallNamed => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return None;
                }
                let target = static_regs.get(instr.a() as usize).and_then(Clone::clone)?;
                let (function_index, captures) = static_call_target(target)?;
                let function = artifact.module.functions.get(function_index as usize)?;
                let slot_prefix = format!("call{call_pc}.");
                let args = scalar_named_call_args(
                    function,
                    ir,
                    &slot_prefix,
                    facts,
                    pc,
                    &static_regs,
                    instr.a(),
                    instr.bx() & 0x7f,
                    instr.bx() >> 7,
                    tmp_index,
                )?;
                let value = native_straightline_function_return(
                    artifact,
                    function_index as usize,
                    &args,
                    &captures,
                    &mut static_globals,
                    0,
                    ir,
                    tmp_index,
                )
                .ok()??;
                if store_native_inline_scalar_value(
                    ir,
                    extra_globals,
                    &mut static_regs,
                    call_pc,
                    instr.a(),
                    value,
                    tmp_index,
                )
                .is_none()
                {
                    return None;
                }
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::SetIndex => {
                if !three_regs_in_bounds(register_count, instr) {
                    return None;
                }
                let target = static_regs.get(instr.a() as usize).and_then(Clone::clone)?;
                let key = static_regs.get(instr.b() as usize).and_then(Clone::clone)?;
                let value = static_regs.get(instr.c() as usize).and_then(Clone::clone)?;
                static_regs[instr.a() as usize] = Some(native_static_set_index(target, key, value)?);
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::Len => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return None;
                }
                let target = static_regs.get(instr.b() as usize).and_then(Clone::clone)?;
                match target {
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::I64,
                    } => {
                        let len = next_tmp(tmp_index);
                        ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
                        ir.push_str(&format!("  store i64 {len}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
                    }
                    _ => return None,
                }
                static_regs[instr.a() as usize] = None;
                emit_inline_branch_to_next(ir, call_pc, pc, code.len());
            }
            Opcode::CallDirect => return None,
            opcode if opcode.is_return() => {
                if instr.return_count() == 0 {
                    // void/nil return: store nil into caller dst and jump to next
                    ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
                    ir.push_str(&format!("  br label {}\n", native_label(call_pc + 1, caller_code_len)));
                } else {
                    if instr.return_count() != 1 || !reg_in_bounds(register_count, instr.a()) {
                        return None;
                    }
                    if let Some(value) = static_regs.get(instr.a() as usize).and_then(Clone::clone)
                        && store_native_inline_return_value(ir, extra_globals, dst, call_pc, value, tmp_index).is_some()
                    {
                        ir.push_str(&format!("  br label {}\n", native_label(call_pc + 1, caller_code_len)));
                        continue;
                    }
                    let kind = facts.register_kind_before(pc, instr.a())?;
                    let value = next_tmp(tmp_index);
                    let ty = kind.llvm_type();
                    ir.push_str(&format!(
                        "  {value} = load {ty}, ptr %call{call_pc}.r{}.slot\n",
                        instr.a()
                    ));
                    ir.push_str(&format!("  store {ty} {value}, ptr %r{dst}.slot\n"));
                    ir.push_str(&format!("  br label {}\n", native_label(call_pc + 1, caller_code_len)));
                } // end else (b == 1)
            }
            Opcode::Nop => emit_inline_branch_to_next(ir, call_pc, pc, code.len()),
            _ => return None,
        }
        ir.push('\n');
    }
    ir.push_str(&format!("call{call_pc}.exit:\n"));
    ir.push_str(&format!(
        "  br label {}\n\n",
        native_label(call_pc + 1, caller_code_len)
    ));
    let _ = artifact;
    let _ = &mut static_globals;
    Some(())
}

fn store_native_inline_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    dst: u8,
    call_pc: usize,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::F64(value) => {
            ir.push_str(&format!("  store double {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::Bool(value) => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::Nil => {
            ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            let symbol = if symbol.is_empty() {
                let symbol = format!("@lk_call{call_pc}_ret_str_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                symbol
            } else {
                if symbol.starts_with("@lk_func") || symbol.starts_with("@lk_static_") {
                    extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                }
                symbol
            };
            ir.push_str(&format!("  store ptr {symbol}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::StringPtr(value) => {
            ir.push_str(&format!("  store ptr {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
        }
        _ => return None,
    }
    Some(())
}

fn static_call_target(value: NativeStraightlineValue) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match value {
        NativeStraightlineValue::Function(function_index) => Some((function_index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((function_index, captures)),
        _ => None,
    }
}

fn compare_test_i64_pred(opcode: Opcode) -> Option<&'static str> {
    Some(match opcode {
        Opcode::TestEqInt | Opcode::TestEqIntI => "eq",
        Opcode::TestNeInt | Opcode::TestNeIntI => "ne",
        Opcode::TestLtInt | Opcode::TestLtIntI => "slt",
        Opcode::TestLeInt | Opcode::TestLeIntI => "sle",
        Opcode::TestGtInt | Opcode::TestGtIntI => "sgt",
        Opcode::TestGeInt | Opcode::TestGeIntI => "sge",
        _ => return None,
    })
}
