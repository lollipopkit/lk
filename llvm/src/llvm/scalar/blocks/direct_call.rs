use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::block_helpers::emit_static_named_call,
        scalar::facts::{NativeScalarFacts, NativeScalarKind},
        straightline_value::NativeStraightlineValue,
        subfunction::compile_native_scalar_subfunction,
    },
    vm::{Instr, ModuleArtifact},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_named_call_block(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    facts: &NativeScalarFacts,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    register_count: usize,
    tmp_index: &mut usize,
    code_len: usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) {
        return None;
    }
    emit_static_named_call(
        ir,
        extra_globals,
        artifact,
        facts,
        pc,
        static_regs,
        static_globals,
        instr,
        tmp_index,
    )?;
    emit_branch_to_next(ir, pc, code_len);
    Some(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_fallback_direct_subfunction_call(
    ir: &mut String,
    artifact: &ModuleArtifact,
    recursive_indices: &[u16],
    additional_subfn_indices: &mut Vec<u16>,
    instr: Instr,
    pc: usize,
    code_len: usize,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
    record_additional_subfn: bool,
) -> Option<()> {
    let callee_index = instr.b();
    let all_recursive = recursive_indices.to_vec();
    if compile_native_scalar_subfunction(artifact, callee_index as usize, &all_recursive)
        .ok()
        .flatten()
        .is_none()
    {
        return None;
    }
    let return_kind = facts
        .register_kind_before(pc + 1, instr.a())
        .unwrap_or(NativeScalarKind::I64);
    let return_ty = return_kind.llvm_type();
    let mut call_args = String::new();
    for i in 0..instr.c() as usize {
        let arg_reg = instr.a() as usize + 1 + i;
        if arg_reg >= register_count {
            return None;
        }
        let arg_kind = facts
            .register_kind_before(pc, arg_reg as u8)
            .unwrap_or(NativeScalarKind::I64);
        let arg_ty = arg_kind.llvm_type();
        let arg_tmp = next_tmp(tmp_index);
        ir.push_str(&format!("  {arg_tmp} = load {arg_ty}, ptr %r{arg_reg}.slot\n"));
        if i > 0 {
            call_args.push_str(", ");
        }
        call_args.push_str(&format!("{arg_ty} {arg_tmp}"));
    }
    let result = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {result} = call {return_ty} @lk_fn_{callee_index}({call_args})\n"
    ));
    ir.push_str(&format!("  store {return_ty} {result}, ptr %r{}.slot\n", instr.a()));
    if record_additional_subfn {
        additional_subfn_indices.push(u16::from(callee_index));
    }
    emit_branch_to_next(ir, pc, code_len);
    Some(())
}
