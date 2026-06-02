use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::local_register_kind_before,
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::NativeStraightlineValue,
    },
    vm::Instr32,
};

pub(super) fn emit_not_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return None;
    }
    let kind = facts
        .register_kind_before(pc, instr.b())
        .or_else(|| local_register_kind_before(code, pc, instr.b()))?;
    match kind {
        NativeScalarKind::Bool
        | NativeScalarKind::I64
        | NativeScalarKind::F64
        | NativeScalarKind::StrPtr
        | NativeScalarKind::MaybeI64
        | NativeScalarKind::MaybeStrPtr => {
            let value = next_tmp(tmp_index);
            let cond = next_tmp(tmp_index);
            let out = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
            ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
            ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
            ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
        }
        NativeScalarKind::Nil => {
            ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
        }
    }
    static_regs[instr.a() as usize] = None;
    emit_branch_to_next(ir, pc, code.len());
    Some(())
}
