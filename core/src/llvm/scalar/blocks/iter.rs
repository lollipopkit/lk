use crate::{
    llvm::{
        ir_text::emit_branch_to_next, scalar::contains::emit_static_to_iter_block,
        straightline_value::NativeStraightlineValue,
    },
    vm::{ConstHeapValue32Data, Instr32},
};

pub(super) fn emit_to_iter_block(
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    ir: &mut String,
) -> Option<()> {
    emit_static_to_iter_block(
        static_regs,
        register_count,
        code,
        int_consts,
        strings,
        heap_values,
        pc,
        instr,
    )?;
    emit_branch_to_next(ir, pc, code.len());
    Some(())
}
