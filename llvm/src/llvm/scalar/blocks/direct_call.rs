use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::facts::{NativeScalarFacts, NativeScalarKind},
        scalar::{
            block_helpers::{emit_static_direct_call_result, emit_static_named_call},
            inline::emit_inline_direct_scalar_call,
        },
        straightline_value::NativeStraightlineValue,
        subfunction::compile_native_scalar_subfunction,
    },
    vm::{ConstHeapValueData, Instr, ModuleArtifact},
};

use super::{
    asserts::emit_native_assert_direct_call,
    callees::{callee_contains_call, callee_is_native_assert},
    direct_print::emit_direct_emit_helper_call,
    list_direct_calls::emit_list_direct_call,
    list_methods::function_has_list_return_shape,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_direct_call_block(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    recursive_indices: &[u16],
    additional_subfn_indices: &mut Vec<u16>,
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    facts: &NativeScalarFacts,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    register_count: usize,
    global_count: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) {
        return None;
    }
    let callee_index = instr.b();
    if let Some(callee) = artifact.module.functions.get(callee_index as usize)
        && emit_direct_emit_helper_call(ir, extra_globals, callee, facts, static_regs, instr, pc, tmp_index).is_some()
    {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
        emit_branch_to_next(ir, pc, code.len());
        return Some(());
    }
    if emit_static_direct_call_result(
        ir,
        extra_globals,
        artifact,
        code,
        int_consts,
        strings,
        heap_values,
        pc,
        static_regs,
        static_globals,
        instr,
        tmp_index,
    )
    .is_some()
    {
        emit_branch_to_next(ir, pc, code.len());
        return Some(());
    }
    let is_recursive = recursive_indices.contains(&u16::from(callee_index));
    if !is_recursive {
        let callee = artifact.module.functions.get(callee_index as usize)?;
        if function_has_list_return_shape(callee) {
            emit_list_direct_call(
                ir,
                extra_globals,
                static_regs,
                instr,
                pc,
                callee_index as usize,
                facts,
                tmp_index,
            )?;
            additional_subfn_indices.push(u16::from(callee_index));
            emit_branch_to_next(ir, pc, code.len());
            return Some(());
        }
        if callee_is_native_assert(callee) {
            emit_native_assert_direct_call(ir, instr, pc, code.len(), register_count, facts, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
            return Some(());
        }
        let inline_result = if callee_contains_call(callee) {
            None
        } else {
            emit_inline_direct_scalar_call(
                ir,
                extra_globals,
                artifact,
                callee,
                pc,
                instr,
                register_count,
                global_count,
                global_names,
                code,
                static_regs,
                static_globals,
                facts,
                tmp_index,
                code.len(),
            )
        };
        if inline_result.is_none() {
            emit_fallback_direct_subfunction_call(
                ir,
                artifact,
                recursive_indices,
                additional_subfn_indices,
                instr,
                pc,
                code.len(),
                register_count,
                facts,
                tmp_index,
                true,
            )?;
        }
        static_regs[instr.a() as usize] = None;
    } else {
        emit_fallback_direct_subfunction_call(
            ir,
            artifact,
            recursive_indices,
            additional_subfn_indices,
            instr,
            pc,
            code.len(),
            register_count,
            facts,
            tmp_index,
            false,
        )?;
        static_regs[instr.a() as usize] = None;
    }
    Some(())
}

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
