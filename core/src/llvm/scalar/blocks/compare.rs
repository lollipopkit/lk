use crate::{
    llvm::{
        ir_text::emit_branch_to_next,
        scalar::{
            block_helpers::{
                emit_string_ptr_equality_block, emit_text_string_equality_block, local_compare_kind,
                local_heap_kind_before, local_register_kind_before, static_register_value_trusted_before,
                three_regs_in_bounds,
            },
            contains::{
                emit_dynamic_int_list_compare_block, emit_static_collection_compare_block, local_direct_load_nil_before,
            },
            emit::{emit_numeric_compare_block, emit_scalar_equality_block},
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::NativeStraightlineValue,
    },
    vm::{ConstHeapValueData, Instr, Opcode},
};

pub(super) fn emit_compare_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    if emit_get_index_nil_compare(ir, static_regs, code, pc, instr, tmp_index).is_some() {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_static_collection_compare_block(
        ir,
        extra_globals,
        static_regs,
        code,
        int_consts,
        strings,
        heap_values,
        pc,
        instr,
    )
    .is_some()
        || emit_dynamic_int_list_compare_block(
            ir,
            static_regs,
            code,
            int_consts,
            strings,
            heap_values,
            pc,
            instr,
            tmp_index,
        )
        .is_some()
        || emit_text_string_compare(ir, extra_globals, static_regs, pc, instr, tmp_index)
    {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let lhs_heap_kind = local_heap_kind_before(code, heap_values, pc, instr.b());
    let rhs_heap_kind = local_heap_kind_before(code, heap_values, pc, instr.c());
    let lhs_trusted = static_register_value_trusted_before(code, pc, instr.b());
    let rhs_trusted = static_register_value_trusted_before(code, pc, instr.c());
    let raw_lhs_kind = facts
        .register_kind_before(pc, instr.b())
        .or_else(|| static_compare_register_kind(static_regs, instr.b(), lhs_trusted));
    let raw_rhs_kind = facts
        .register_kind_before(pc, instr.c())
        .or_else(|| static_compare_register_kind(static_regs, instr.c(), rhs_trusted));
    let lhs_direct_nil = immediate_load_nil_before(code, pc, instr.b());
    let rhs_direct_nil = immediate_load_nil_before(code, pc, instr.c());
    let lhs_kind = raw_lhs_kind.filter(|kind| *kind != NativeScalarKind::Nil || lhs_trusted || lhs_direct_nil);
    let rhs_kind = raw_rhs_kind.filter(|kind| *kind != NativeScalarKind::Nil || rhs_trusted || rhs_direct_nil);
    let lhs_local_kind = if lhs_kind == Some(NativeScalarKind::Nil) && lhs_direct_nil {
        Some(NativeScalarKind::Nil)
    } else if !lhs_trusted {
        None
    } else {
        local_register_kind_before(code, pc, instr.b())
    };
    let rhs_local_kind = if rhs_kind == Some(NativeScalarKind::Nil) && rhs_direct_nil {
        Some(NativeScalarKind::Nil)
    } else if !rhs_trusted {
        None
    } else {
        local_register_kind_before(code, pc, instr.c())
    };
    let mut kind = local_compare_kind(lhs_kind, lhs_heap_kind, lhs_local_kind);
    let mut rhs_kind = local_compare_kind(rhs_kind, rhs_heap_kind, rhs_local_kind).unwrap_or(NativeScalarKind::I64);
    if kind == Some(NativeScalarKind::MaybeI64) && raw_rhs_kind == Some(NativeScalarKind::Nil) {
        rhs_kind = NativeScalarKind::Nil;
    }
    if kind == Some(NativeScalarKind::Nil) && raw_rhs_kind == Some(NativeScalarKind::MaybeI64) {
        rhs_kind = NativeScalarKind::MaybeI64;
    }
    if kind.is_none()
        && raw_lhs_kind == Some(NativeScalarKind::Nil)
        && !lhs_trusted
        && matches!(rhs_kind, NativeScalarKind::StrPtr | NativeScalarKind::Nil)
    {
        kind = Some(NativeScalarKind::MaybeI64);
    }
    if rhs_kind == NativeScalarKind::I64
        && raw_rhs_kind == Some(NativeScalarKind::Nil)
        && !rhs_trusted
        && matches!(kind, Some(NativeScalarKind::StrPtr | NativeScalarKind::Nil))
    {
        rhs_kind = NativeScalarKind::MaybeI64;
    }
    let Some(kind) = kind else {
        return false;
    };
    if kind.is_numeric() && rhs_kind.is_numeric() {
        emit_numeric_compare_block(ir, instr, kind, rhs_kind, tmp_index);
    } else if kind == rhs_kind
        && kind == NativeScalarKind::StrPtr
        && matches!(
            instr.opcode(),
            Opcode::CmpLtInt | Opcode::CmpLeInt | Opcode::CmpGtInt | Opcode::CmpGeInt
        )
    {
        emit_string_ptr_ordering_block(ir, instr, tmp_index);
    } else if kind == rhs_kind
        && kind == NativeScalarKind::StrPtr
        && !local_direct_load_nil_before(code, pc, instr.b())
        && !local_direct_load_nil_before(code, pc, instr.c())
    {
        emit_string_ptr_equality_block(ir, instr, tmp_index);
    } else if matches!(
        (kind, rhs_kind),
        (NativeScalarKind::MaybeI64, NativeScalarKind::StrPtr) | (NativeScalarKind::StrPtr, NativeScalarKind::MaybeI64)
    ) && matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt)
    {
        emit_optional_string_ptr_equality_block(ir, instr, kind, rhs_kind, tmp_index);
    } else if matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt) {
        emit_scalar_equality_block(ir, instr, kind, rhs_kind, tmp_index);
    } else {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_get_index_nil_compare(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) -> Option<()> {
    if !matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt) {
        return None;
    }
    let maybe_reg = if last_write_opcode_before(code, pc, instr.b()) == Some(Opcode::GetIndex)
        && local_direct_load_nil_before(code, pc, instr.c())
    {
        instr.b()
    } else if last_write_opcode_before(code, pc, instr.c()) == Some(Opcode::GetIndex)
        && local_direct_load_nil_before(code, pc, instr.b())
    {
        instr.c()
    } else {
        return None;
    };
    let present = crate::llvm::ir_text::next_tmp(tmp_index);
    let cmp = crate::llvm::ir_text::next_tmp(tmp_index);
    let out = crate::llvm::ir_text::next_tmp(tmp_index);
    let pred = if instr.opcode() == Opcode::CmpInt { "eq" } else { "ne" };
    ir.push_str(&format!("  {present} = load i64, ptr %r{maybe_reg}.present.slot\n"));
    ir.push_str(&format!("  {cmp} = icmp {pred} i64 {present}, 0\n"));
    ir.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(out));
    Some(())
}

fn last_write_opcode_before(code: &[Instr], pc: usize, reg: u8) -> Option<Opcode> {
    code.iter()
        .copied()
        .take(pc)
        .rev()
        .find(|instr| instr.a() == reg && !matches!(instr.opcode(), Opcode::Nop | Opcode::Jmp | Opcode::Test))
        .map(|instr| instr.opcode())
}

fn emit_string_ptr_ordering_block(ir: &mut String, instr: Instr, tmp_index: &mut usize) {
    let lhs = crate::llvm::ir_text::next_tmp(tmp_index);
    let rhs = crate::llvm::ir_text::next_tmp(tmp_index);
    let cmp = crate::llvm::ir_text::next_tmp(tmp_index);
    let ok = crate::llvm::ir_text::next_tmp(tmp_index);
    let value = crate::llvm::ir_text::next_tmp(tmp_index);
    let pred = match instr.opcode() {
        Opcode::CmpLtInt => "slt",
        Opcode::CmpLeInt => "sle",
        Opcode::CmpGtInt => "sgt",
        Opcode::CmpGeInt => "sge",
        _ => "eq",
    };
    ir.push_str(&format!("  {lhs} = load ptr, ptr %r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load ptr, ptr %r{}.slot\n", instr.c()));
    ir.push_str(&format!("  {cmp} = call i32 @strcmp(ptr {lhs}, ptr {rhs})\n"));
    ir.push_str(&format!("  {ok} = icmp {pred} i32 {cmp}, 0\n"));
    ir.push_str(&format!("  {value} = zext i1 {ok} to i64\n"));
    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
}

fn emit_text_string_compare(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    _pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) -> bool {
    let (Some(NativeStraightlineValue::Text(parts)), Some(NativeStraightlineValue::String { value, .. })) = (
        static_regs.get(instr.b() as usize).and_then(Clone::clone),
        static_regs.get(instr.c() as usize).and_then(Clone::clone),
    ) else {
        return false;
    };
    if !matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt)
        || emit_text_string_equality_block(
            ir,
            extra_globals,
            &parts,
            &value,
            instr.a(),
            instr.opcode() == Opcode::CmpNeInt,
            tmp_index,
        )
        .is_none()
    {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    true
}

fn emit_optional_string_ptr_equality_block(
    ir: &mut String,
    instr: Instr,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) {
    let maybe_reg = if lhs_kind == NativeScalarKind::MaybeI64 {
        instr.b()
    } else {
        instr.c()
    };
    let str_reg = if rhs_kind == NativeScalarKind::StrPtr {
        instr.c()
    } else {
        instr.b()
    };
    let present = crate::llvm::ir_text::next_tmp(tmp_index);
    let maybe_ptr = crate::llvm::ir_text::next_tmp(tmp_index);
    let safe_maybe_ptr = crate::llvm::ir_text::next_tmp(tmp_index);
    let str_ptr = crate::llvm::ir_text::next_tmp(tmp_index);
    let cmp_value = crate::llvm::ir_text::next_tmp(tmp_index);
    let value_equal = crate::llvm::ir_text::next_tmp(tmp_index);
    let present_ok = crate::llvm::ir_text::next_tmp(tmp_index);
    let equal = crate::llvm::ir_text::next_tmp(tmp_index);
    let out = crate::llvm::ir_text::next_tmp(tmp_index);
    ir.push_str(&format!("  {present} = load i64, ptr %r{maybe_reg}.present.slot\n"));
    ir.push_str(&format!("  {maybe_ptr} = load ptr, ptr %r{maybe_reg}.slot\n"));
    ir.push_str(&format!("  {str_ptr} = load ptr, ptr %r{str_reg}.slot\n"));
    ir.push_str(&format!("  {present_ok} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!(
        "  {safe_maybe_ptr} = select i1 {present_ok}, ptr {maybe_ptr}, ptr {str_ptr}\n"
    ));
    ir.push_str(&format!(
        "  {cmp_value} = call i32 @strcmp(ptr {safe_maybe_ptr}, ptr {str_ptr})\n"
    ));
    ir.push_str(&format!("  {value_equal} = icmp eq i32 {cmp_value}, 0\n"));
    ir.push_str(&format!("  {equal} = and i1 {present_ok}, {value_equal}\n"));
    if instr.opcode() == Opcode::CmpNeInt {
        let ne = crate::llvm::ir_text::next_tmp(tmp_index);
        ir.push_str(&format!("  {ne} = xor i1 {equal}, true\n"));
        ir.push_str(&format!("  {out} = zext i1 {ne} to i64\n"));
    } else {
        ir.push_str(&format!("  {out} = zext i1 {equal} to i64\n"));
    }
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
}

fn immediate_load_nil_before(code: &[Instr], pc: usize, reg: u8) -> bool {
    pc.checked_sub(1)
        .and_then(|prev_pc| code.get(prev_pc).copied())
        .is_some_and(|prev| prev.a() == reg && prev.opcode() == Opcode::LoadNil)
}

fn static_compare_register_kind(
    static_regs: &[Option<NativeStraightlineValue>],
    reg: u8,
    trusted: bool,
) -> Option<NativeScalarKind> {
    if !trusted {
        return None;
    }
    match static_regs.get(reg as usize).and_then(|value| value.as_ref())? {
        NativeStraightlineValue::I64(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::MaybeI64 { .. } | NativeStraightlineValue::MaybeBool { .. } => {
            Some(NativeScalarKind::MaybeI64)
        }
        NativeStraightlineValue::F64(_) => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Bool(_) => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::Nil => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::DynamicJoinedText { .. } => Some(NativeScalarKind::StrPtr),
        _ => None,
    }
}
