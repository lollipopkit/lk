//! Diagnostic reasons for unsupported native-lowering shapes.
//!
//! Every function returns a human-readable reason string explaining why a
//! particular `ModuleArtifact` cannot be lowered to true native AOT.  The
//! LLVM backend uses these to reject unsupported shapes instead of falling
//! back to an Instr artifact shell or host launcher.

use crate::stmt::import::ImportStmt;
use crate::vm::{Instr, ModuleArtifact, Opcode};

use super::scalar::facts::native_scalar_block_facts_with_statics_and_functions;
use super::straightline_main::{
    native_direct_call_targets_need_blocks, native_scalar_function_needs_blocks, native_straightline_function_has_call,
};
use super::straightline_value::native_static_global;

pub(crate) fn unsupported_module_artifact_reason(artifact: &ModuleArtifact) -> String {
    let Some(function) = artifact.module.functions.get(artifact.module.entry as usize) else {
        return format!("entry function {} is out of bounds", artifact.module.entry);
    };
    if function.param_count != 0 {
        return format!("entry function has {} parameters", function.param_count);
    }
    if function.capture_count != 0 {
        return format!("entry function has {} captures", function.capture_count);
    }

    let code = match function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(code) => code,
        Err(error) => return format!("entry bytecode decode failed: {error}"),
    };
    if let Some(global_names) = unsupported_runtime_global_names(artifact, &code) {
        return format!(
            "runtime globals are not native-lowerable yet ({})",
            global_names.join(", ")
        );
    }
    let needs_block_lowering =
        native_scalar_function_needs_blocks(&code) || native_direct_call_targets_need_blocks(artifact, &code);
    if needs_block_lowering
        && native_scalar_block_facts_with_statics_and_functions(
            function.register_count as usize,
            artifact.module.globals.len(),
            &artifact.module.globals,
            &function.consts.ints,
            &function.consts.strings,
            &function.consts.heap_values,
            &code,
            Some(&artifact.module.functions),
        )
        .is_none()
    {
        if let Some(reason) = unsupported_scalar_block_opcode_reason(artifact, &code) {
            return reason;
        }
        return "scalar block facts could not classify the entry function".to_string();
    }
    if needs_block_lowering
        && native_straightline_function_has_call(&code)
        && let Some(reason) = unsupported_control_flow_call_reason(artifact, function, &code)
    {
        return reason;
    }
    if let Some(reason) = unsupported_runtime_return_reason(function, artifact.module.globals.len(), &code) {
        return reason;
    }
    "entry function contains an unsupported Instr/native value shape".to_string()
}

/// Check if a Call instruction's target register holds a statically known
/// Function/Closure or Builtin target that the block compiler can handle.
fn call_has_static_target(
    global_names: &[String],
    function: &crate::vm::FunctionData,
    pc: usize,
    instr: Instr,
) -> bool {
    // For Call/CallNamed: a is dest and also callee register (for Call, a == b for self-call;
    // for CallNamed, a holds the callable)
    let target_reg = instr.a();
    // Walk backwards through the bytecode to find the definition of target_reg
    for check_pc in (0..pc).rev() {
        let Ok(check) = Instr::try_from_raw(function.code[check_pc]) else {
            continue;
        };
        let defines_target = match check.opcode() {
            Opcode::Move => check.a() == target_reg,
            Opcode::GetGlobal => check.a() == target_reg,
            Opcode::GetIndex | Opcode::GetList => check.a() == target_reg,
            Opcode::LoadFunction => check.a() == target_reg,
            Opcode::MakeClosure => check.a() == target_reg,
            _ => false,
        };
        if !defines_target {
            continue;
        }
        match check.opcode() {
            Opcode::GetGlobal => {
                let name = global_names.get(check.bx() as usize).map(|s| s.as_str()).unwrap_or("");
                return crate::llvm::straightline_value::native_static_global(name).is_some();
            }
            Opcode::Move => {
                // Recursively trace through Move chains (r5 -> r1 -> r2 -> LoadFunction/MakeClosure).
                // Create a synthetic instr with the source register as target_reg.
                return call_has_static_target(
                    global_names,
                    function,
                    check_pc,
                    Instr::abc(Opcode::Call, check.b(), check.b(), 0),
                );
            }
            Opcode::GetIndex | Opcode::GetList => return true,
            Opcode::LoadFunction | Opcode::MakeClosure => return true,
            _ => return false,
        }
    }
    false
}

fn unsupported_control_flow_call_reason(
    artifact: &ModuleArtifact,
    function: &crate::vm::FunctionData,
    code: &[Instr],
) -> Option<String> {
    if let Some(reason) = unsupported_control_flow_direct_call_reason(artifact, function, code) {
        return Some(reason);
    }
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::Call => {
                // Check if the target register holds a statically known Builtin (println, panic)
                // or Function/Closure target. In that case the block compiler handles it.
                if !call_has_static_target(artifact.module.globals.as_slice(), function, pc, instr) {
                    return Some(format!(
                        "control-flow dynamic Call at pc {pc} uses callee r{} with {} args; native lowering needs a statically known Function/Closure target and scalar arguments for this call shape",
                        instr.b(),
                        instr.c()
                    ));
                }
            }
            Opcode::CallNamed => {
                // Check if the target register holds a statically known Function/Closure.
                // The block compiler can handle this via emit_static_named_call.
                if !call_has_static_target(artifact.module.globals.as_slice(), function, pc, instr) {
                    return Some(format!(
                        "control-flow CallNamed at pc {pc} uses callee r{} with packed args {}; native lowering needs a statically known Function/Closure target and scalar positional/named arguments",
                        instr.a(),
                        instr.bx()
                    ));
                }
            }
            Opcode::MakeClosure => {}
            _ => {}
        }
    }
    None
}

fn unsupported_runtime_return_reason(
    function: &crate::vm::FunctionData,
    global_count: usize,
    code: &[Instr],
) -> Option<String> {
    let mut callable_regs = vec![None; function.register_count as usize];
    let mut callable_globals = vec![None; global_count];
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::LoadFunction => {
                *callable_regs.get_mut(instr.a() as usize)? = Some("function");
            }
            Opcode::MakeClosure => {
                *callable_regs.get_mut(instr.a() as usize)? = Some("closure");
            }
            Opcode::Move => {
                let value = *callable_regs.get(instr.b() as usize)?;
                *callable_regs.get_mut(instr.a() as usize)? = value;
            }
            Opcode::SetGlobal => {
                let value = *callable_regs.get(instr.a() as usize)?;
                *callable_globals.get_mut(instr.bx() as usize)? = value;
            }
            Opcode::GetGlobal => {
                let value = *callable_globals.get(instr.bx() as usize)?;
                *callable_regs.get_mut(instr.a() as usize)? = value;
            }
            Opcode::Return if instr.b() == 1 => {
                if let Some(kind) = callable_regs.get(instr.a() as usize).copied().flatten() {
                    return Some(format!(
                        "runtime callable returns are not native-lowerable yet: Return at pc {pc} returns a {kind} value from r{}",
                        instr.a()
                    ));
                }
            }
            Opcode::Return => {}
            _ => {
                if reg_writes_a(instr.opcode()) {
                    *callable_regs.get_mut(instr.a() as usize)? = None;
                }
            }
        }
    }
    None
}

fn reg_writes_a(opcode: Opcode) -> bool {
    !matches!(
        opcode,
        Opcode::SetGlobal
            | Opcode::SetIndex
            | Opcode::SetFieldK
            | Opcode::StoreCellVal
            | Opcode::TryBegin
            | Opcode::TryEnd
            | Opcode::Raise
            | Opcode::Test
            | Opcode::BrFalse
            | Opcode::BrTrue
            | Opcode::Jmp
            | Opcode::Return
            | Opcode::Nop
    )
}

fn unsupported_scalar_block_opcode_reason(artifact: &ModuleArtifact, code: &[Instr]) -> Option<String> {
    unsupported_scalar_block_opcode_reason_in_code(code).or_else(|| {
        for instr in code.iter().copied() {
            if instr.opcode() != Opcode::CallDirect {
                continue;
            }
            let function = artifact.module.functions.get(instr.b() as usize)?;
            let callee_code = function
                .code
                .iter()
                .copied()
                .map(Instr::try_from_raw)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            if let Some(reason) = unsupported_scalar_block_opcode_reason_in_code(&callee_code) {
                return Some(format!(
                    "direct callee function {} is not native-lowerable yet: {reason}",
                    instr.b()
                ));
            }
        }
        None
    })
}

fn unsupported_scalar_block_opcode_reason_in_code(code: &[Instr]) -> Option<String> {
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::NewMap => {
                return Some(format!(
                    "control-flow NewMap at pc {pc} needs native typed container construction lowering"
                ));
            }
            Opcode::NewRange => {
                return Some(format!(
                    "control-flow {:?} at pc {pc} needs native container/string lowering",
                    instr.opcode()
                ));
            }
            _ => {}
        }
    }
    None
}

fn unsupported_control_flow_direct_call_reason(
    artifact: &ModuleArtifact,
    function: &crate::vm::FunctionData,
    code: &[Instr],
) -> Option<String> {
    for (pc, instr) in code.iter().copied().enumerate() {
        if instr.opcode() != Opcode::CallDirect {
            continue;
        }
        let callee = instr.b() as usize;
        let arg_start = instr.a() as usize + 1;
        let arg_count = instr.c() as usize;
        let arg_end = arg_start.saturating_add(arg_count);
        let Some(target) = artifact.module.functions.get(callee) else {
            return Some(format!(
                "control-flow CallDirect at pc {pc} targets missing function {callee}"
            ));
        };
        if arg_end > function.register_count as usize {
            return Some(format!(
                "control-flow CallDirect at pc {pc} argument window r{arg_start}..r{arg_end} exceeds entry register count {}",
                function.register_count
            ));
        }
        if target.capture_count == 0 && target.param_count == arg_count as u16 {
            continue;
        }
        return Some(format!(
            "control-flow CallDirect at pc {pc} targets function {callee} with {} params, {} captures, {} entry regs; block lowering needs native function ABI for this call shape",
            target.param_count, target.capture_count, target.register_count
        ));
    }
    None
}

fn unsupported_runtime_global_names(artifact: &ModuleArtifact, code: &[Instr]) -> Option<Vec<String>> {
    let mut seeded_globals = vec![false; artifact.module.globals.len()];
    let imported_names = imported_global_names(&artifact.imports);
    let mut names = Vec::new();
    for instr in code {
        match instr.opcode() {
            Opcode::SetGlobal => {
                if let Some(seeded) = seeded_globals.get_mut(instr.bx() as usize) {
                    *seeded = true;
                }
            }
            Opcode::GetGlobal => {
                let index = instr.bx() as usize;
                if seeded_globals.get(index).copied().unwrap_or(false) {
                    continue;
                }
                let Some(name) = artifact.module.globals.get(index) else {
                    continue;
                };
                if native_static_global(name).is_some() {
                    continue;
                }
                if imported_names.is_empty() || imported_names.iter().any(|imported| imported == name) {
                    names.push(name.clone());
                }
            }
            _ => {}
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() { None } else { Some(names) }
}

fn imported_global_names(imports: &[ImportStmt]) -> Vec<String> {
    let mut names = Vec::new();
    for import in imports {
        match import {
            ImportStmt::Module { module } => names.push(module.clone()),
            ImportStmt::File { path } => names.push(import_file_global_name(path)),
            ImportStmt::Items { items, .. } => {
                for item in items {
                    names.push(item.alias.clone().unwrap_or_else(|| item.name.clone()));
                }
            }
            ImportStmt::Namespace { alias, .. } | ImportStmt::ModuleAlias { alias, .. } => names.push(alias.clone()),
        }
    }
    names.sort();
    names.dedup();
    names
}

fn import_file_global_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module")
        .to_string()
}
