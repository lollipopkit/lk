use crate::{
    llvm::{
        ir_text::emit_branch_to_next,
        scalar::{
            block_helpers::{
                emit_string_ptr_equality_block, emit_text_string_equality_block, local_compare_kind,
                local_heap_kind_before, local_register_kind_before, three_regs_in_bounds,
            },
            contains::{
                emit_dynamic_int_list_compare_block, emit_static_collection_compare_block, local_direct_load_nil_before,
            },
            emit::{emit_numeric_compare_block, emit_scalar_equality_block},
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::NativeStraightlineValue,
    },
    vm::{ConstHeapValue32Data, Instr32, Opcode32},
};

pub(super) fn emit_compare_block(
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
    if emit_static_collection_compare_block(ir, static_regs, code, int_consts, strings, heap_values, pc, instr)
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
    let lhs_kind = facts.register_kind_before(pc, instr.b());
    let rhs_kind = facts.register_kind_before(pc, instr.c());
    let lhs_local_kind = if lhs_kind == Some(NativeScalarKind::Nil) && local_direct_load_nil_before(code, pc, instr.b())
    {
        Some(NativeScalarKind::Nil)
    } else {
        local_register_kind_before(code, pc, instr.b())
    };
    let rhs_local_kind = if rhs_kind == Some(NativeScalarKind::Nil) && local_direct_load_nil_before(code, pc, instr.c())
    {
        Some(NativeScalarKind::Nil)
    } else {
        local_register_kind_before(code, pc, instr.c())
    };
    let Some(kind) = local_compare_kind(lhs_kind, lhs_heap_kind, lhs_local_kind) else {
        return false;
    };
    let rhs_kind = local_compare_kind(rhs_kind, rhs_heap_kind, rhs_local_kind).unwrap_or(NativeScalarKind::I64);
    if kind.is_numeric() && rhs_kind.is_numeric() {
        emit_numeric_compare_block(ir, instr, kind, rhs_kind, tmp_index);
    } else if kind == rhs_kind
        && kind == NativeScalarKind::StrPtr
        && !local_direct_load_nil_before(code, pc, instr.b())
        && !local_direct_load_nil_before(code, pc, instr.c())
    {
        emit_string_ptr_equality_block(ir, instr, tmp_index);
    } else if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) {
        emit_scalar_equality_block(ir, instr, kind, rhs_kind, tmp_index);
    } else {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn emit_text_string_compare(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    _pc: usize,
    instr: Instr32,
    tmp_index: &mut usize,
) -> bool {
    let (Some(NativeStraightlineValue::Text(parts)), Some(NativeStraightlineValue::String { value, .. })) = (
        static_regs.get(instr.b() as usize).and_then(Clone::clone),
        static_regs.get(instr.c() as usize).and_then(Clone::clone),
    ) else {
        return false;
    };
    if !matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt)
        || emit_text_string_equality_block(
            ir,
            extra_globals,
            &parts,
            &value,
            instr.a(),
            instr.opcode() == Opcode32::CmpNeInt,
            tmp_index,
        )
        .is_none()
    {
        return false;
    }
    static_regs[instr.a() as usize] = None;
    true
}
