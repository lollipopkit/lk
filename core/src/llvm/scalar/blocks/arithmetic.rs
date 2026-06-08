use crate::{
    llvm::{
        const_display::native_const_list_display,
        ir_text::{emit_branch_to_next, next_tmp},
        scalar::{
            block_helpers::{
                concat_text_values, emit_mixed_numeric_int_opcode_block, i64_slot_kind, local_register_kind_before,
                three_regs_in_bounds,
            },
            contains::{local_static_heap_const_before, text_value_from_trusted_reg},
            emit::{emit_i64_add_mul_block, emit_i64_binary_block, emit_i64_immediate_block},
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{NativeStraightlineValue, native_static_i64_binary},
    },
    vm::{ConstHeapValueData, Instr, Opcode},
};

pub(super) fn emit_int_arithmetic_block(
    ir: &mut String,
    code: &[Instr],
    _int_consts: &[i64],
    _strings: &[String],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    static_regs: &mut [Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    if instr.opcode() == Opcode::AddMulInt {
        emit_i64_add_mul_block(ir, instr, tmp_index);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_static_list_concat_block(code, heap_values, pc, instr, static_regs) {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let Some(lhs) = local_register_kind_before(code, pc, instr.b())
        .or_else(|| static_reg_kind(static_regs, instr.b()))
        .or_else(|| facts.register_kind_before(pc, instr.b()))
        .or_else(|| local_arithmetic_index_kind_before(code, pc, instr.b()))
    else {
        return false;
    };
    let Some(rhs) = local_register_kind_before(code, pc, instr.c())
        .or_else(|| static_reg_kind(static_regs, instr.c()))
        .or_else(|| facts.register_kind_before(pc, instr.c()))
        .or_else(|| local_arithmetic_index_kind_before(code, pc, instr.c()))
    else {
        return false;
    };
    if emit_string_add_block(ir, code, pc, instr, lhs, rhs, facts, static_regs, tmp_index) {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let static_lhs = static_regs.get(instr.b() as usize).and_then(Clone::clone);
    let static_rhs = static_regs.get(instr.c() as usize).and_then(Clone::clone);
    if let (Some(NativeStraightlineValue::I64(lhs)), Some(NativeStraightlineValue::I64(rhs))) = (static_lhs, static_rhs)
        && let Some(value) = native_static_i64_binary(&lhs, &rhs, instr.opcode())
    {
        ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(value));
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if i64_slot_kind(lhs) && i64_slot_kind(rhs) {
        emit_i64_binary_block(ir, instr, tmp_index);
    } else if matches!(instr.opcode(), Opcode::MinInt | Opcode::MaxInt) {
        return false;
    } else if lhs.is_numeric() && rhs.is_numeric() {
        emit_mixed_numeric_int_opcode_block(ir, "", instr, lhs, rhs, tmp_index);
    } else {
        return false;
    }
    emit_branch_to_next(ir, pc, code.len());
    true
}

pub(super) fn emit_int_immediate_block(
    ir: &mut String,
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    static_regs: &mut [Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
    code_len: usize,
) -> bool {
    if instr.a() as usize >= register_count
        || instr.b() as usize >= register_count
        || !matches!(
            local_register_kind_before(code, pc, instr.b())
                .or_else(|| facts.register_kind_before(pc, instr.b()))
                .or_else(|| static_reg_kind(static_regs, instr.b())),
            Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
        )
    {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    emit_i64_immediate_block(ir, instr, tmp_index);
    emit_branch_to_next(ir, pc, code_len);
    true
}

fn static_reg_kind(static_regs: &[Option<NativeStraightlineValue>], reg: u8) -> Option<NativeScalarKind> {
    match static_regs.get(reg as usize).and_then(Clone::clone)? {
        NativeStraightlineValue::I64(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::MaybeI64 { .. } => Some(NativeScalarKind::MaybeI64),
        NativeStraightlineValue::F64(_) => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Bool(_) => Some(NativeScalarKind::Bool),
        _ => None,
    }
}

fn local_arithmetic_index_kind_before(code: &[Instr], pc: usize, reg: u8) -> Option<NativeScalarKind> {
    let start = pc.saturating_sub(8);
    for prev in code.get(start..pc)?.iter().copied().rev() {
        if prev.a() != reg {
            continue;
        }
        return matches!(prev.opcode(), Opcode::GetIndex | Opcode::GetList).then_some(NativeScalarKind::MaybeI64);
    }
    None
}

fn emit_static_list_concat_block(
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    instr: Instr,
    static_regs: &mut [Option<NativeStraightlineValue>],
) -> bool {
    if instr.opcode() != Opcode::AddInt {
        return false;
    }
    let (
        Some(NativeStraightlineValue::List { elements: lhs, .. }),
        Some(NativeStraightlineValue::List { elements: rhs, .. }),
    ) = (
        static_or_heap_list(code, heap_values, pc, instr.b(), static_regs),
        static_or_heap_list(code, heap_values, pc, instr.c(), static_regs),
    )
    else {
        return false;
    };
    let mut elements = lhs;
    elements.extend(rhs);
    let Some(value) = native_const_list_display(&elements) else {
        return false;
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::List {
        symbol: format!("@lk_concat_list_{pc}"),
        value,
        elements,
    });
    true
}

fn static_or_heap_list(
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    reg: u8,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    if let Some(value @ NativeStraightlineValue::List { .. }) = static_regs.get(reg as usize).and_then(Clone::clone) {
        return Some(value);
    }
    match local_static_heap_const_before(code, heap_values, pc, reg) {
        Some(value @ NativeStraightlineValue::List { .. }) => Some(value),
        _ => None,
    }
}

fn emit_string_add_block(
    ir: &mut String,
    code: &[Instr],
    pc: usize,
    instr: Instr,
    lhs: NativeScalarKind,
    rhs: NativeScalarKind,
    facts: &NativeScalarFacts,
    static_regs: &mut [Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
) -> bool {
    if instr.opcode() != Opcode::AddInt
        || (!matches!(lhs, NativeScalarKind::StrPtr) && !matches!(rhs, NativeScalarKind::StrPtr))
    {
        return false;
    }
    if matches!(lhs, NativeScalarKind::StrPtr) && matches!(rhs, NativeScalarKind::StrPtr) {
        let lhs_ptr = load_str_ptr(ir, instr.b(), tmp_index);
        let rhs_ptr = load_str_ptr(ir, instr.c(), tmp_index);
        let out = emit_string_concat_to_reg(ir, instr.a(), &lhs_ptr, &rhs_ptr, pc, tmp_index);
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(out));
        return true;
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
    static_regs[instr.a() as usize] = Some(value);
    true
}

fn load_str_ptr(ir: &mut String, reg: u8, tmp_index: &mut usize) -> String {
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
    value
}

fn emit_string_concat_to_reg(
    ir: &mut String,
    dst: u8,
    lhs: &str,
    rhs: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> String {
    let lhs_len = next_tmp(tmp_index);
    let rhs_len = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_len} = call i64 @strlen(ptr {lhs})\n"));
    ir.push_str(&format!("  {rhs_len} = call i64 @strlen(ptr {rhs})\n"));
    ir.push_str(&format!(
        "  {out} = getelementptr [4096 x i8], ptr %r{dst}.text.buf, i64 0, i64 0\n"
    ));
    emit_copy_loop(ir, pc, "lhs", lhs, &out, "0", lhs_len.as_str(), tmp_index);
    emit_copy_loop(ir, pc, "rhs", rhs, &out, lhs_len.as_str(), rhs_len.as_str(), tmp_index);
    let total = next_tmp(tmp_index);
    let zero_slot = next_tmp(tmp_index);
    ir.push_str(&format!("  {total} = add i64 {lhs_len}, {rhs_len}\n"));
    ir.push_str(&format!("  {zero_slot} = getelementptr i8, ptr {out}, i64 {total}\n"));
    ir.push_str(&format!("  store i8 0, ptr {zero_slot}\n"));
    ir.push_str(&format!("  store ptr {out}, ptr %r{dst}.slot\n"));
    out
}

fn emit_copy_loop(
    ir: &mut String,
    pc: usize,
    name: &str,
    src: &str,
    dst: &str,
    dst_offset: &str,
    len: &str,
    tmp_index: &mut usize,
) {
    let idx_slot = next_tmp(tmp_index);
    let loop_label = format!("lk_concat_{pc}_{name}_loop_{}", *tmp_index);
    let body_label = format!("lk_concat_{pc}_{name}_body_{}", *tmp_index);
    let done_label = format!("lk_concat_{pc}_{name}_done_{}", *tmp_index);
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
