use crate::llvm::{
    ir_text::emit_branch_to_next,
    scalar::facts::NativeScalarFacts,
    straightline_value::{NativeBuiltin, NativeStraightlineValue},
};
use crate::vm::{ConstHeapValue32Data, Instr32};

use super::{
    i64_list_methods::{emit_dynamic_i64_list_builtin_call, emit_dynamic_i64_list_builtin_call_from_regs},
    list_methods::{
        emit_dynamic_f64_list_builtin_call, emit_dynamic_f64_list_builtin_call_from_regs,
        emit_dynamic_ptr_list_builtin_call, emit_dynamic_ptr_list_builtin_call_from_regs,
    },
};

pub(super) fn emit_dynamic_list_builtin_call_from_regs_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    builtin: NativeBuiltin,
    facts: &NativeScalarFacts,
    pc: usize,
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    code_len: usize,
    tmp_index: &mut usize,
) -> bool {
    let emitted = emit_dynamic_ptr_list_builtin_call_from_regs(
        ir,
        extra_globals,
        static_regs,
        instr,
        builtin,
        facts,
        pc,
        tmp_index,
    )
    .or_else(|| {
        emit_dynamic_i64_list_builtin_call_from_regs(
            ir,
            static_regs,
            code,
            heap_values,
            instr,
            builtin,
            facts,
            pc,
            tmp_index,
        )
    })
    .or_else(|| emit_dynamic_f64_list_builtin_call_from_regs(ir, static_regs, instr, builtin, facts, pc, tmp_index));
    if emitted.is_none() {
        return false;
    }
    emit_branch_to_next(ir, pc, code_len);
    true
}

pub(super) fn emit_dynamic_list_builtin_call_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    code_len: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> bool {
    let emitted =
        emit_dynamic_ptr_list_builtin_call(ir, extra_globals, static_regs, instr, pc, builtin, args, tmp_index)
            .or_else(|| {
                emit_dynamic_i64_list_builtin_call(
                    ir,
                    static_regs,
                    code,
                    heap_values,
                    instr,
                    pc,
                    builtin,
                    args,
                    tmp_index,
                )
            })
            .or_else(|| emit_dynamic_f64_list_builtin_call(ir, static_regs, instr, pc, builtin, args, tmp_index));
    if emitted.is_none() {
        return false;
    }
    emit_branch_to_next(ir, pc, code_len);
    true
}
