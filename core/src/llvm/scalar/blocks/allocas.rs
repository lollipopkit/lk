use crate::{
    llvm::{
        dynamic_containers::{emit_dynamic_int_list_allocas, emit_dynamic_string_int_map_allocas},
        ir_text::native_scalar_main_header,
        options::LlvmBackendOptions,
    },
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Module32Artifact, Opcode32},
};

pub(super) fn emit_scalar_entry_allocas(
    artifact: &Module32Artifact,
    options: &LlvmBackendOptions,
    register_count: usize,
    global_count: usize,
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
) -> Option<String> {
    let mut ir = native_scalar_main_header(options);
    for reg in 0..register_count {
        ir.push_str(&format!("  %r{reg}.slot = alloca i64\n"));
        ir.push_str(&format!("  %r{reg}.present.slot = alloca i64\n"));
        ir.push_str(&format!("  %r{reg}.text.buf = alloca [4096 x i8]\n"));
    }
    for global in 0..global_count {
        ir.push_str(&format!("  %g{global}.slot = alloca i64\n"));
    }
    for reg in 0..register_count {
        ir.push_str(&format!("  store i64 1, ptr %r{reg}.present.slot\n"));
    }
    let call_register_count = artifact
        .module
        .functions
        .iter()
        .map(|function| function.register_count as usize)
        .max()
        .unwrap_or(register_count)
        .max(register_count);
    for pc in 0..code.len() {
        for reg in 0..call_register_count {
            ir.push_str(&format!("  %call{pc}.r{reg}.slot = alloca i64\n"));
            ir.push_str(&format!("  %call{pc}.r{reg}.present.slot = alloca i64\n"));
        }
    }
    for (pc, instr) in code.iter().copied().enumerate() {
        if instr.opcode() == Opcode32::CallDirect {
            let callee = artifact.module.functions.get(instr.b() as usize)?;
            if function_has_list_return_shape(callee) {
                emit_dynamic_int_list_allocas(&mut ir, &format!("list{pc}"));
            }
            continue;
        }
        if dynamic_map_alloca_needed(heap_values, instr) {
            emit_dynamic_string_int_map_allocas(&mut ir, &format!("map{pc}"));
        }
        if dynamic_list_alloca_needed(heap_values, instr) {
            emit_dynamic_int_list_allocas(&mut ir, &format!("list{pc}"));
        }
    }
    ir.push_str("  br label %bb0\n\n");
    Some(ir)
}

fn function_has_list_return_shape(function: &crate::vm::Function32Data) -> bool {
    function
        .code
        .iter()
        .copied()
        .filter_map(|raw| Instr32::try_from_raw(raw).ok())
        .any(|instr| instr.opcode() == Opcode32::ListPush)
}

fn dynamic_map_alloca_needed(heap_values: &[ConstHeapValue32Data], instr: Instr32) -> bool {
    matches!(instr.opcode(), Opcode32::Call)
        || matches!(instr.opcode(), Opcode32::LoadHeapConst)
            && matches!(heap_values.get(instr.bx() as usize), Some(ConstHeapValue32Data::Map(values)) if values.is_empty())
}

fn dynamic_list_alloca_needed(heap_values: &[ConstHeapValue32Data], instr: Instr32) -> bool {
    matches!(instr.opcode(), Opcode32::Call | Opcode32::NewList | Opcode32::SliceFrom)
        || matches!(instr.opcode(), Opcode32::LoadHeapConst)
            && matches!(heap_values.get(instr.bx() as usize), Some(ConstHeapValue32Data::List(values)) if values.is_empty() || values.iter().all(|v| matches!(v, ConstRuntimeValue32Data::Int(_))))
}
