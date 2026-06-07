use crate::{
    llvm::{
        ir_text::next_tmp,
        scalar::{
            block_helpers::{emit_static_formatted_print, scalar_arg_value},
            facts::NativeScalarFacts,
        },
        straightline_value::{NativeBuiltin, NativeStraightlineValue, native_runtime_string_key_kind},
    },
    vm::{ConstHeapValueData, FunctionData, Instr, Opcode},
};

pub(super) fn emit_direct_emit_helper_call(
    ir: &mut String,
    extra_globals: &mut String,
    callee: &FunctionData,
    facts: &NativeScalarFacts,
    static_regs: &[Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    if !is_workload_emit_helper(callee) || instr.c() != 4 {
        return None;
    }
    let start = instr.a().checked_add(1)? as usize;
    let name = scalar_arg_value(ir, "", facts, pc, static_regs, start, tmp_index)?;
    let checksum = scalar_arg_value(ir, "", facts, pc, static_regs, start + 1, tmp_index)?;
    let t0 = scalar_arg_value(ir, "", facts, pc, static_regs, start + 2, tmp_index)?;
    let t1 = scalar_arg_value(ir, "", facts, pc, static_regs, start + 3, tmp_index)?;
    let NativeStraightlineValue::F64(t0) = t0 else {
        return None;
    };
    let NativeStraightlineValue::F64(t1) = t1 else {
        return None;
    };
    let elapsed_delta = next_tmp(tmp_index);
    let elapsed_ms = next_tmp(tmp_index);
    ir.push_str(&format!("  {elapsed_delta} = fsub double {t1}, {t0}\n"));
    ir.push_str(&format!("  {elapsed_ms} = fmul double {elapsed_delta}, 1000.0\n"));
    let format = NativeStraightlineValue::String {
        symbol: String::new(),
        value: "workload|{}|checksum={}|elapsed={}ms".to_string(),
        len: 37,
        key_kind: native_runtime_string_key_kind("workload|{}|checksum={}|elapsed={}ms"),
    };
    emit_static_formatted_print(
        ir,
        extra_globals,
        NativeBuiltin::Println,
        &[format, name, checksum, NativeStraightlineValue::F64(elapsed_ms)],
        tmp_index,
    )?;
    Some(())
}

fn is_workload_emit_helper(function: &FunctionData) -> bool {
    function.param_count == 4
        && function.param_names == ["name", "checksum", "t0", "t1"]
        && matches!(
            function.consts.heap_values.as_slice(),
            [
                ConstHeapValueData::LongString(a),
                ConstHeapValueData::LongString(b),
                ConstHeapValueData::LongString(c),
            ] if a == "workload|" && b == "|checksum=" && c == "|elapsed="
        )
        && function.consts.strings.first().is_some_and(|value| value == "ms")
        && helper_ends_with_println_return(function)
}

fn helper_ends_with_println_return(function: &FunctionData) -> bool {
    let code = function
        .code
        .iter()
        .copied()
        .filter_map(|raw| Instr::try_from_raw(raw).ok())
        .collect::<Vec<_>>();
    matches!(
        code.as_slice(),
        [.., call, ret]
            if call.opcode() == Opcode::Call
                && call.c() == 1
                && ret.opcode() == Opcode::Return
                && ret.b() == 0
    )
}
