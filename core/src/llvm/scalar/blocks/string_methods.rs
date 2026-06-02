use crate::{
    llvm::{
        ir_text::emit_branch_to_next,
        scalar::{
            block_helpers::{
                emit_dynamic_string_starts_with, static_string_value_trusted_at_call, store_native_scalar_call_result,
                three_regs_in_bounds,
            },
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::NativeStraightlineValue,
    },
    vm::Instr32,
};

pub(super) fn emit_string_starts_with_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if !three_regs_in_bounds(register_count, instr) {
        return None;
    }
    let NativeStraightlineValue::String { value: prefix, .. } =
        static_regs.get(instr.c() as usize).and_then(Clone::clone)?
    else {
        return None;
    };
    if let Some(NativeStraightlineValue::String { value: target, .. }) =
        static_regs.get(instr.b() as usize).and_then(Clone::clone)
        && static_string_value_trusted_at_call(code, pc, instr.b())
    {
        let value = i64::from(target.starts_with(&prefix));
        let result = NativeStraightlineValue::Bool(value.to_string());
        store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), result, tmp_index)?;
    } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
        emit_dynamic_string_starts_with(ir, extra_globals, "", instr.a(), instr.b(), &prefix, tmp_index);
        static_regs[instr.a() as usize] = None;
    } else {
        return None;
    }
    emit_branch_to_next(ir, pc, code.len());
    Some(())
}
