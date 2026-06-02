use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::{block_helpers::store_native_scalar_call_result, facts::NativeScalarFacts},
        straightline_value::{NativeStraightlineValue, native_static_global},
    },
    vm::Instr32,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_get_global_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &[Option<NativeStraightlineValue>],
    global_names: &[String],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    global_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
    code_len: usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
        return None;
    }
    if let Some(value) = global_names
        .get(instr.bx() as usize)
        .and_then(|name| native_static_global(name))
    {
        if store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value.clone(), tmp_index)
            .is_none()
        {
            static_regs[instr.a() as usize] = Some(value);
        }
        emit_branch_to_next(ir, pc, code_len);
        return Some(());
    }
    if let Some(value) = static_globals.get(instr.bx() as usize).and_then(Clone::clone) {
        if store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value.clone(), tmp_index)
            .is_none()
        {
            static_regs[instr.a() as usize] = Some(value);
        }
        emit_branch_to_next(ir, pc, code_len);
        return Some(());
    }
    let kind = facts.global_kind_before(pc, instr.bx())?;
    static_regs[instr.a() as usize] = None;
    let value = next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %g{}.slot\n", instr.bx()));
    ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
    emit_branch_to_next(ir, pc, code_len);
    Some(())
}

pub(super) fn emit_set_global_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    global_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
    code_len: usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
        return None;
    }
    if let Some(value) = static_regs.get(instr.a() as usize).and_then(Clone::clone) {
        static_globals[instr.bx() as usize] = Some(value);
        emit_branch_to_next(ir, pc, code_len);
        return Some(());
    }
    let kind = facts.register_kind_before(pc, instr.a())?;
    static_globals[instr.bx() as usize] = None;
    static_regs[instr.a() as usize] = None;
    let value = next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.a()));
    ir.push_str(&format!("  store {ty} {value}, ptr %g{}.slot\n", instr.bx()));
    emit_branch_to_next(ir, pc, code_len);
    Some(())
}
