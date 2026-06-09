use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::{block_helpers::local_static_i64_before, facts::NativeScalarFacts},
        straightline_value::{NativeStraightlineValue, native_static_load_cell, native_static_store_cell},
    },
    vm::{Instr, Opcode},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_cell_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    match instr.opcode() {
        Opcode::StoreCellVal => emit_store_cell_block(
            ir,
            static_regs,
            code,
            int_consts,
            pc,
            instr,
            register_count,
            facts,
            tmp_index,
        ),
        Opcode::LoadCellVal => emit_load_cell_block(ir, static_regs, code, pc, instr, register_count, facts, tmp_index),
        _ => None,
    }
}

pub(super) fn emit_store_cell_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return None;
    }
    if let Some(kind) = facts.register_kind_before(pc, instr.b()) {
        let value = next_tmp(tmp_index);
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
    emit_branch_to_next(ir, pc, code.len());
    Some(())
}

pub(super) fn emit_load_cell_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return None;
    }
    if let Some(kind) = facts.register_kind_before(pc, instr.b()) {
        let value = next_tmp(tmp_index);
        let ty = kind.llvm_type();
        ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
    } else {
        let value = next_tmp(tmp_index);
        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
    }
    static_regs[instr.a() as usize] = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .and_then(native_static_load_cell);
    emit_branch_to_next(ir, pc, code.len());
    Some(())
}
