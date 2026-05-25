use anyhow::{Result, bail};

use crate::{
    stmt::{Program, import::ImportStmt},
    vm::{Compiler32, ConstHeapValue32Data, Instr32, Module32Artifact, Opcode32},
};

use super::{
    callee_eval::{native_straightline_function_return, native_straightline_named_call_args},
    const_display::{native_const_list_display, native_const_map_display, native_string_const_value},
    ir_text::{llvm_float_literal, native_relative_target},
    options::{LlvmBackendOptions, OptLevel},
    output::{emit_native_builtin_call, native_scalar_main_ir, native_straightline_main_ir},
    scalar_blocks::compile_native_scalar_main_blocks,
    scalar_facts::native_scalar_block_facts_with_statics_and_functions,
    straightline_value::{
        NativeStraightlineValue, NativeStringKeyKind, native_runtime_string_key_kind, native_static_alias_symbol,
        native_static_collection_equality_bool, native_static_compare_bool, native_static_container_test,
        native_static_contains, native_static_equality_bool, native_static_f64_binary,
        native_static_f64_divisor_nonzero, native_static_global, native_static_i64_binary,
        native_static_i64_divisor_nonzero, native_static_index, native_static_int_range, native_static_len,
        native_static_list_from_values, native_static_list_join, native_static_list_push, native_static_load_cell,
        native_static_map_from_pairs, native_static_map_rest, native_static_not, native_static_object_from_fields,
        native_static_set_index, native_static_slice_from, native_static_store_cell, native_static_string_split,
        native_static_string_starts_with, native_static_to_iter, native_static_to_string_value, native_static_truthy,
    },
};

pub type LlvmBackendError = anyhow::Error;

/// Metadata for an emitted LLVM module.
#[derive(Debug, Clone)]
pub struct LlvmModule {
    pub name: String,
    pub ir: String,
    pub target_triple: Option<String>,
}

/// Aggregates the raw IR plus optional optimized IR produced by `opt`.
#[derive(Debug, Clone)]
pub struct LlvmModuleArtifact {
    pub module: LlvmModule,
    pub optimised_ir: Option<String>,
    pub opt_level: OptLevel,
}

#[derive(Debug, Default)]
pub struct LlvmBackend {
    options: LlvmBackendOptions,
}

impl LlvmBackend {
    pub fn new(options: LlvmBackendOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &LlvmBackendOptions {
        &self.options
    }

    pub fn with_options(mut self, options: LlvmBackendOptions) -> Self {
        self.options = options;
        self
    }

    pub fn compile_program(&self, program: &Program) -> Result<LlvmModuleArtifact> {
        let module = Compiler32::compile_module(program)?;
        let artifact = Module32Artifact::new(crate::stmt::import::collect_program_imports(program), &module)?;
        compile_module32_artifact_to_llvm(&artifact, self.options.clone())
    }
}

pub fn compile_program_to_llvm(program: &Program, options: LlvmBackendOptions) -> Result<LlvmModuleArtifact> {
    LlvmBackend::new(options).compile_program(program)
}

pub fn compile_module32_artifact_to_llvm(
    artifact: &Module32Artifact,
    options: LlvmBackendOptions,
) -> Result<LlvmModuleArtifact> {
    if let Some(ir) = compile_native_scalar_main_artifact(artifact, &options)? {
        return Ok(LlvmModuleArtifact {
            module: LlvmModule {
                name: options.module_name,
                ir,
                target_triple: options.target_triple,
            },
            optimised_ir: None,
            opt_level: options.opt_level,
        });
    }

    bail!(
        "LLVM native lowering does not support this Module32Artifact shape yet: {}",
        unsupported_module32_artifact_reason(artifact)
    )
}

fn unsupported_module32_artifact_reason(artifact: &Module32Artifact) -> String {
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
        .map(Instr32::try_from_raw)
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
    if needs_block_lowering && native_straightline_function_has_call(&code) {
        return unsupported_control_flow_call_reason(artifact, function, &code);
    }
    if let Some(reason) = unsupported_runtime_return_reason(function, artifact.module.globals.len(), &code) {
        return reason;
    }
    "entry function contains an unsupported Instr32/native value shape".to_string()
}

fn unsupported_control_flow_call_reason(
    artifact: &Module32Artifact,
    function: &crate::vm::Function32Data,
    code: &[Instr32],
) -> String {
    if let Some(reason) = unsupported_control_flow_direct_call_reason(artifact, function, code) {
        return reason;
    }
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode32::Call => {
                return format!(
                    "control-flow dynamic Call at pc {pc} uses callee r{} with {} args; native lowering needs a statically known Function/Closure target and scalar arguments for this call shape",
                    instr.b(),
                    instr.c()
                );
            }
            Opcode32::CallNamed => {
                return format!(
                    "control-flow CallNamed at pc {pc} uses callee r{} with packed args {}; native lowering needs a statically known Function/Closure target and scalar positional/named arguments",
                    instr.a(),
                    instr.bx()
                );
            }
            Opcode32::MakeClosure => {
                return format!(
                    "control-flow MakeClosure at pc {pc} builds function {} from captures starting at r{}; block lowering needs statically known native-lowerable captures",
                    instr.b(),
                    instr.c()
                );
            }
            _ => {}
        }
    }
    "control-flow lowering with direct calls is not native-lowerable yet".to_string()
}

fn unsupported_runtime_return_reason(
    function: &crate::vm::Function32Data,
    global_count: usize,
    code: &[Instr32],
) -> Option<String> {
    let mut callable_regs = vec![None; function.register_count as usize];
    let mut callable_globals = vec![None; global_count];
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode32::LoadFunction => {
                *callable_regs.get_mut(instr.a() as usize)? = Some("function");
            }
            Opcode32::MakeClosure => {
                *callable_regs.get_mut(instr.a() as usize)? = Some("closure");
            }
            Opcode32::Move => {
                let value = *callable_regs.get(instr.b() as usize)?;
                *callable_regs.get_mut(instr.a() as usize)? = value;
            }
            Opcode32::SetGlobal => {
                let value = *callable_regs.get(instr.a() as usize)?;
                *callable_globals.get_mut(instr.bx() as usize)? = value;
            }
            Opcode32::GetGlobal => {
                let value = *callable_globals.get(instr.bx() as usize)?;
                *callable_regs.get_mut(instr.a() as usize)? = value;
            }
            Opcode32::Return if instr.b() == 1 => {
                if let Some(kind) = callable_regs.get(instr.a() as usize).copied().flatten() {
                    return Some(format!(
                        "runtime callable returns are not native-lowerable yet: Return at pc {pc} returns a {kind} value from r{}",
                        instr.a()
                    ));
                }
            }
            Opcode32::Return => {}
            _ => {
                if reg_writes_a(instr.opcode()) {
                    *callable_regs.get_mut(instr.a() as usize)? = None;
                }
            }
        }
    }
    None
}

fn reg_writes_a(opcode: Opcode32) -> bool {
    !matches!(
        opcode,
        Opcode32::SetGlobal
            | Opcode32::SetIndex
            | Opcode32::StoreCellVal
            | Opcode32::TryBegin
            | Opcode32::TryEnd
            | Opcode32::Raise
            | Opcode32::Test
            | Opcode32::Jmp
            | Opcode32::Return
            | Opcode32::Nop
    )
}

fn unsupported_scalar_block_opcode_reason(artifact: &Module32Artifact, code: &[Instr32]) -> Option<String> {
    unsupported_scalar_block_opcode_reason_in_code(code).or_else(|| {
        for instr in code.iter().copied() {
            if instr.opcode() != Opcode32::CallDirect {
                continue;
            }
            let function = artifact.module.functions.get(instr.b() as usize)?;
            let callee_code = function
                .code
                .iter()
                .copied()
                .map(Instr32::try_from_raw)
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

fn unsupported_scalar_block_opcode_reason_in_code(code: &[Instr32]) -> Option<String> {
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode32::NewMap => {
                return Some(format!(
                    "control-flow NewMap at pc {pc} needs native typed container construction lowering"
                ));
            }
            Opcode32::NewRange | Opcode32::ToIter | Opcode32::Contains | Opcode32::SliceFrom | Opcode32::MapRest => {
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
    artifact: &Module32Artifact,
    function: &crate::vm::Function32Data,
    code: &[Instr32],
) -> Option<String> {
    for (pc, instr) in code.iter().copied().enumerate() {
        if instr.opcode() != Opcode32::CallDirect {
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

fn unsupported_runtime_global_names(artifact: &Module32Artifact, code: &[Instr32]) -> Option<Vec<String>> {
    let mut seeded_globals = vec![false; artifact.module.globals.len()];
    let imported_names = imported_global_names(&artifact.imports);
    let mut names = Vec::new();
    for instr in code {
        match instr.opcode() {
            Opcode32::SetGlobal => {
                if let Some(seeded) = seeded_globals.get_mut(instr.bx() as usize) {
                    *seeded = true;
                }
            }
            Opcode32::GetGlobal => {
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

fn compile_native_scalar_main_artifact(
    artifact: &Module32Artifact,
    options: &LlvmBackendOptions,
) -> Result<Option<String>> {
    let Some(function) = artifact.module.functions.get(artifact.module.entry as usize) else {
        return Ok(None);
    };
    if function.param_count != 0 || function.capture_count != 0 {
        return Ok(None);
    }

    let code = function
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()?;
    let body = String::new();
    if code.is_empty() {
        return Ok(Some(native_scalar_main_ir(options, &body, None)));
    }
    let needs_block_lowering =
        native_scalar_function_needs_blocks(&code) || native_direct_call_targets_need_blocks(artifact, &code);
    if needs_block_lowering
        && let Some(scalar_facts) = native_scalar_block_facts_with_statics_and_functions(
            function.register_count as usize,
            artifact.module.globals.len(),
            &artifact.module.globals,
            &function.consts.strings,
            &function.consts.heap_values,
            &code,
            Some(&artifact.module.functions),
        )
    {
        return compile_native_scalar_main_blocks(
            artifact,
            options,
            function.register_count as usize,
            artifact.module.globals.len(),
            &artifact.module.globals,
            &function.consts.ints,
            &function.consts.floats,
            &function.consts.strings,
            &function.consts.heap_values,
            &code,
            &scalar_facts,
        );
    }

    let mut regs: Vec<Option<NativeStraightlineValue>> = vec![None; function.register_count as usize];
    let mut globals: Vec<Option<NativeStraightlineValue>> = vec![None; artifact.module.globals.len()];
    let mut body = String::new();
    let mut ssa_index = 0usize;
    let mut pc = 0usize;
    let mut steps = 0usize;
    let mut handlers: Vec<(u8, usize)> = Vec::new();
    while pc < code.len() {
        steps += 1;
        if steps > code.len().saturating_mul(16) {
            return Ok(None);
        }
        let instr = code[pc];
        let mut next_pc = pc + 1;
        match instr.opcode() {
            Opcode32::LoadNil => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
            }
            Opcode32::LoadBool => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(i64::from(instr.b() != 0).to_string()));
            }
            Opcode32::LoadInt => {
                let Some(value) = function.consts.ints.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(value.to_string()));
            }
            Opcode32::LoadFloat => {
                let Some(value) = function.consts.floats.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(llvm_float_literal(*value)));
            }
            Opcode32::LoadString => {
                let Some(value) = function.consts.strings.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let Some(value) = native_string_const_value(value) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                    symbol: format!("@lk_const_str_{}", instr.bx()),
                    len: value.chars().count(),
                    key_kind: NativeStringKeyKind::Short,
                    value,
                });
            }
            Opcode32::LoadHeapConst => {
                let Some(value) = function.consts.heap_values.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = match value {
                    ConstHeapValue32Data::LongString(value) => {
                        let Some(value) = native_string_const_value(value) else {
                            return Ok(None);
                        };
                        Some(NativeStraightlineValue::String {
                            symbol: format!("@lk_const_heap_str_{}", instr.bx()),
                            len: value.chars().count(),
                            key_kind: NativeStringKeyKind::Heap,
                            value,
                        })
                    }
                    ConstHeapValue32Data::List(values) => {
                        let Some(value) = native_const_list_display(values) else {
                            return Ok(None);
                        };
                        Some(NativeStraightlineValue::List {
                            symbol: format!("@lk_const_heap_list_{}", instr.bx()),
                            value,
                            elements: values.clone(),
                        })
                    }
                    ConstHeapValue32Data::Map(values) => {
                        let Some(value) = native_const_map_display(values) else {
                            return Ok(None);
                        };
                        Some(NativeStraightlineValue::Map {
                            symbol: format!("@lk_const_heap_map_{}", instr.bx()),
                            value,
                            entries: values.clone(),
                        })
                    }
                    ConstHeapValue32Data::UpvalCell(value) => Some(NativeStraightlineValue::Cell {
                        symbol: format!("@lk_const_heap_cell_{}", instr.bx()),
                        value: Box::new(match value.as_ref() {
                            crate::vm::ConstRuntimeValue32Data::Nil => NativeStraightlineValue::Nil,
                            crate::vm::ConstRuntimeValue32Data::Bool(value) => {
                                NativeStraightlineValue::Bool(i64::from(*value).to_string())
                            }
                            crate::vm::ConstRuntimeValue32Data::Int(value) => {
                                NativeStraightlineValue::I64(value.to_string())
                            }
                            crate::vm::ConstRuntimeValue32Data::Float(value) => {
                                NativeStraightlineValue::F64(llvm_float_literal(*value))
                            }
                            _ => return Ok(None),
                        }),
                    }),
                };
            }
            Opcode32::LoadFunction => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Function(instr.bx()));
            }
            Opcode32::MakeClosure => {
                let Some(function) = artifact.module.functions.get(instr.b() as usize) else {
                    return Ok(None);
                };
                let start = instr.c() as usize;
                let Some(end) = start.checked_add(function.capture_count as usize) else {
                    return Ok(None);
                };
                let Some(values) = regs.get(start..end) else {
                    return Ok(None);
                };
                let Some(captures) = values.iter().cloned().collect::<Option<Vec<_>>>() else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Closure {
                    function_index: instr.b() as u16,
                    captures,
                });
            }
            Opcode32::LoadCellVal => {
                let Some(cell) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = native_static_load_cell(cell) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::StoreCellVal => {
                let Some(cell) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = native_static_store_cell(cell, value) else {
                    return Ok(None);
                };
                native_replace_static_aliases(&mut regs, &mut globals, instr.a() as usize, &value);
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::Move => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::SetGlobal => {
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(global) = globals.get_mut(instr.bx() as usize) else {
                    return Ok(None);
                };
                *global = Some(value);
            }
            Opcode32::GetGlobal => {
                let value = globals.get(instr.bx() as usize).and_then(Clone::clone).or_else(|| {
                    artifact
                        .module
                        .globals
                        .get(instr.bx() as usize)
                        .and_then(|name| native_static_global(name))
                });
                let Some(value) = value else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
                let Some(NativeStraightlineValue::I64(lhs)) = regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let Some(NativeStraightlineValue::I64(rhs)) = regs.get(instr.c() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let name = if let Some(value) = native_static_i64_binary(&lhs, &rhs, instr.opcode()) {
                    value
                } else {
                    if matches!(instr.opcode(), Opcode32::DivInt | Opcode32::ModInt)
                        && !native_static_i64_divisor_nonzero(&rhs).unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    let op = match instr.opcode() {
                        Opcode32::AddInt => "add",
                        Opcode32::SubInt => "sub",
                        Opcode32::MulInt => "mul",
                        Opcode32::DivInt => "sdiv",
                        Opcode32::ModInt => "srem",
                        _ => unreachable!("opcode matched above"),
                    };
                    let name = format!("%r{}_{}", instr.a(), ssa_index);
                    ssa_index += 1;
                    body.push_str(&format!("  {name} = {op} i64 {lhs}, {rhs}\n"));
                    name
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(name));
            }
            Opcode32::AddFloat | Opcode32::SubFloat | Opcode32::MulFloat | Opcode32::DivFloat | Opcode32::ModFloat => {
                let Some(NativeStraightlineValue::F64(lhs)) = regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let Some(NativeStraightlineValue::F64(rhs)) = regs.get(instr.c() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let name = if let Some(value) = native_static_f64_binary(&lhs, &rhs, instr.opcode()) {
                    value
                } else {
                    if matches!(instr.opcode(), Opcode32::DivFloat | Opcode32::ModFloat)
                        && !native_static_f64_divisor_nonzero(&rhs).unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    let op = match instr.opcode() {
                        Opcode32::AddFloat => "fadd",
                        Opcode32::SubFloat => "fsub",
                        Opcode32::MulFloat => "fmul",
                        Opcode32::DivFloat => "fdiv",
                        Opcode32::ModFloat => "frem",
                        _ => unreachable!("opcode matched above"),
                    };
                    let name = format!("%r{}_{}", instr.a(), ssa_index);
                    ssa_index += 1;
                    body.push_str(&format!("  {name} = {op} double {lhs}, {rhs}\n"));
                    name
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(name));
            }
            Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt => {
                let Some(lhs) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(rhs) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if let Some(value) = native_static_compare_bool(&lhs, &rhs, instr.opcode()) {
                    if instr.a() as usize >= regs.len() {
                        return Ok(None);
                    }
                    regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(i64::from(value).to_string()));
                    pc = next_pc;
                    continue;
                }
                let (op, ty, lhs, rhs) = match (lhs, rhs) {
                    (NativeStraightlineValue::I64(lhs), NativeStraightlineValue::I64(rhs)) => {
                        let op = match instr.opcode() {
                            Opcode32::CmpInt => "icmp eq",
                            Opcode32::CmpNeInt => "icmp ne",
                            Opcode32::CmpLtInt => "icmp slt",
                            Opcode32::CmpLeInt => "icmp sle",
                            Opcode32::CmpGtInt => "icmp sgt",
                            Opcode32::CmpGeInt => "icmp sge",
                            _ => unreachable!("opcode matched above"),
                        };
                        (op, "i64", lhs, rhs)
                    }
                    (NativeStraightlineValue::F64(lhs), NativeStraightlineValue::F64(rhs)) => {
                        let op = match instr.opcode() {
                            Opcode32::CmpInt => "fcmp oeq",
                            Opcode32::CmpNeInt => "fcmp une",
                            Opcode32::CmpLtInt => "fcmp olt",
                            Opcode32::CmpLeInt => "fcmp ole",
                            Opcode32::CmpGtInt => "fcmp ogt",
                            Opcode32::CmpGeInt => "fcmp oge",
                            _ => unreachable!("opcode matched above"),
                        };
                        (op, "double", lhs, rhs)
                    }
                    (NativeStraightlineValue::Bool(lhs), NativeStraightlineValue::Bool(rhs))
                        if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) =>
                    {
                        let op = match instr.opcode() {
                            Opcode32::CmpInt => "icmp eq",
                            Opcode32::CmpNeInt => "icmp ne",
                            _ => unreachable!("opcode matched by guard"),
                        };
                        (op, "i64", lhs, rhs)
                    }
                    (
                        NativeStraightlineValue::String { value: lhs, .. },
                        NativeStraightlineValue::String { value: rhs, .. },
                    ) if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) => {
                        if instr.a() as usize >= regs.len() {
                            return Ok(None);
                        }
                        regs[instr.a() as usize] = Some(native_static_equality_bool(lhs == rhs, instr.opcode()));
                        pc = next_pc;
                        continue;
                    }
                    (NativeStraightlineValue::Nil, NativeStraightlineValue::Nil)
                        if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) =>
                    {
                        let value = i64::from(instr.opcode() == Opcode32::CmpInt).to_string();
                        if instr.a() as usize >= regs.len() {
                            return Ok(None);
                        }
                        regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(value));
                        pc = next_pc;
                        continue;
                    }
                    (lhs, rhs) => {
                        let Some(value) = native_static_collection_equality_bool(&lhs, &rhs, instr.opcode()) else {
                            return Ok(None);
                        };
                        if instr.a() as usize >= regs.len() {
                            return Ok(None);
                        }
                        regs[instr.a() as usize] = Some(value);
                        pc = next_pc;
                        continue;
                    }
                };
                let cmp = format!("%cmp{}_{}", instr.a(), ssa_index);
                ssa_index += 1;
                let out = format!("%bool{}_{}", instr.a(), ssa_index);
                ssa_index += 1;
                body.push_str(&format!("  {cmp} = {op} {ty} {lhs}, {rhs}\n"));
                body.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(out));
            }
            Opcode32::Not => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = if let Some(value) = native_static_not(&value) {
                    Some(value)
                } else {
                    match value {
                        NativeStraightlineValue::Nil => Some(NativeStraightlineValue::Bool("1".to_string())),
                        NativeStraightlineValue::Bool(value) => {
                            let cmp = format!("%not{}_{}", instr.a(), ssa_index);
                            ssa_index += 1;
                            let out = format!("%bool{}_{}", instr.a(), ssa_index);
                            ssa_index += 1;
                            body.push_str(&format!("  {cmp} = icmp eq i64 {value}, 0\n"));
                            body.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
                            Some(NativeStraightlineValue::Bool(out))
                        }
                        _ => return Ok(None),
                    }
                };
            }
            Opcode32::IsNil => {
                if regs.get(instr.b() as usize).and_then(Clone::clone).is_none() || instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let is_nil = matches!(regs.get(instr.b() as usize), Some(Some(NativeStraightlineValue::Nil)));
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(i64::from(is_nil).to_string()));
            }
            Opcode32::IsList | Opcode32::IsMap => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let Some(value) = native_static_container_test(value, instr.opcode()) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::Len => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let Some(value) = native_static_len(value) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::ToIter => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_to_iter_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_to_iter(value, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::GetIndex => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_index_str_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_index(target, key, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::SetIndex => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = native_static_set_index(target, key, value) else {
                    return Ok(None);
                };
                native_replace_static_aliases(&mut regs, &mut globals, instr.a() as usize, &value);
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::ListPush => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = native_static_list_push(target, value) else {
                    return Ok(None);
                };
                native_replace_static_aliases(&mut regs, &mut globals, instr.a() as usize, &value);
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::Contains => {
                let Some(needle) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(haystack) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let Some(value) = native_static_contains(needle, haystack) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::SliceFrom => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(start) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_slice_str_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_slice_from(target, start, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::MapRest => {
                let start = instr.b() as usize;
                let Some(width) = 1usize.checked_add(instr.c() as usize) else {
                    return Ok(None);
                };
                let Some(end) = start.checked_add(width) else {
                    return Ok(None);
                };
                let Some(values) = regs.get(start..end) else {
                    return Ok(None);
                };
                let Some(target) = values[0].clone() else {
                    return Ok(None);
                };
                let Some(keys) = values[1..].iter().cloned().collect::<Option<Vec<_>>>() else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_map_rest_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_map_rest(target, &keys, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::NewList => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let start = instr.b() as usize;
                let Some(end) = start.checked_add(instr.c() as usize) else {
                    return Ok(None);
                };
                let Some(values) = regs.get(start..end) else {
                    return Ok(None);
                };
                let Some(values) = values.iter().cloned().collect::<Option<Vec<_>>>() else {
                    return Ok(None);
                };
                let symbol = format!("@lk_new_list_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_list_from_values(&values, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::NewMap => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let start = instr.b() as usize;
                let Some(width) = (instr.c() as usize).checked_mul(2) else {
                    return Ok(None);
                };
                let Some(end) = start.checked_add(width) else {
                    return Ok(None);
                };
                let Some(values) = regs.get(start..end) else {
                    return Ok(None);
                };
                let mut pairs = Vec::with_capacity(instr.c() as usize);
                for pair in values.chunks_exact(2) {
                    let Some(key) = pair[0].clone() else {
                        return Ok(None);
                    };
                    let Some(value) = pair[1].clone() else {
                        return Ok(None);
                    };
                    pairs.push((key, value));
                }
                let symbol = format!("@lk_new_map_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_map_from_pairs(&pairs, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::NewRange => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let start = instr.b() as usize;
                let Some(end) = start.checked_add(3) else {
                    return Ok(None);
                };
                let Some(values) = regs.get(start..end) else {
                    return Ok(None);
                };
                let Some(start) = values[0].clone() else {
                    return Ok(None);
                };
                let Some(end) = values[1].clone() else {
                    return Ok(None);
                };
                let Some(step) = values[2].clone() else {
                    return Ok(None);
                };
                let symbol = format!("@lk_new_range_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_int_range(start, end, step, instr.c() != 0, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::NewObject => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let start = instr.b() as usize;
                let Some(width) = (instr.c() as usize)
                    .checked_mul(2)
                    .and_then(|width| width.checked_add(1))
                else {
                    return Ok(None);
                };
                let Some(end) = start.checked_add(width) else {
                    return Ok(None);
                };
                let Some(values) = regs.get(start..end) else {
                    return Ok(None);
                };
                let Some(values) = values.iter().cloned().collect::<Option<Vec<_>>>() else {
                    return Ok(None);
                };
                let symbol = format!("@lk_new_object_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_object_from_fields(&values, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::ToString => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_tostring_str_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_to_string_value(value, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::ConcatString => {
                let Some(NativeStraightlineValue::String { value: lhs, .. }) =
                    regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let Some(NativeStraightlineValue::String { value: rhs, .. }) =
                    regs.get(instr.c() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_concat_str_{}", ssa_index);
                ssa_index += 1;
                let value = format!("{lhs}{rhs}");
                regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                    symbol,
                    len: value.chars().count(),
                    key_kind: native_runtime_string_key_kind(&value),
                    value,
                });
            }
            Opcode32::StringStartsWith => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(prefix) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let Some(value) = native_static_string_starts_with(target, prefix) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::StringSplit => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(delimiter) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_split_list_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_string_split(target, delimiter, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::ListJoin => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(separator) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_join_str_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_list_join(target, separator, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::Call => {
                if instr.a() != instr.b() {
                    return Ok(None);
                }
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(args) = native_straightline_call_args(&regs, instr.b(), instr.c()) else {
                    return Ok(None);
                };
                if let NativeStraightlineValue::Builtin(builtin) = target {
                    if instr.a() as usize >= regs.len() {
                        return Ok(None);
                    }
                    regs[instr.a() as usize] = emit_native_builtin_call(&mut body, builtin, &args, &mut ssa_index);
                    if regs[instr.a() as usize].is_none() {
                        return Ok(None);
                    }
                    pc = next_pc;
                    continue;
                }
                let Some((function_index, captures)) = native_straightline_call_target(target) else {
                    return Ok(None);
                };
                let Some(value) = native_straightline_function_return(
                    artifact,
                    function_index as usize,
                    &args,
                    &captures,
                    &mut globals,
                    0,
                    &mut body,
                    &mut ssa_index,
                )?
                else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::CallDirect => {
                let Some(args) = native_straightline_call_args(&regs, instr.a(), instr.c()) else {
                    return Ok(None);
                };
                let Some(value) = native_straightline_function_return(
                    artifact,
                    instr.b() as usize,
                    &args,
                    &[],
                    &mut globals,
                    0,
                    &mut body,
                    &mut ssa_index,
                )?
                else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::CallNamed => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some((function_index, captures)) = native_straightline_call_target(target) else {
                    return Ok(None);
                };
                let Some(function) = artifact.module.functions.get(function_index as usize) else {
                    return Ok(None);
                };
                let positional_count = instr.bx() & 0x7f;
                let named_count = instr.bx() >> 7;
                let Some(args) =
                    native_straightline_named_call_args(function, &regs, instr.a(), positional_count, named_count)
                else {
                    return Ok(None);
                };
                let Some(value) = native_straightline_function_return(
                    artifact,
                    function_index as usize,
                    &args,
                    &captures,
                    &mut globals,
                    0,
                    &mut body,
                    &mut ssa_index,
                )?
                else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode32::Test => {
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(truthy) = native_static_truthy(&value) else {
                    return Ok(None);
                };
                let fallthrough = pc + 1;
                let Some(relative) = native_relative_target(pc, instr.c() as i8 as i32, code.len()) else {
                    return Ok(None);
                };
                let truthy_target = if instr.b() != 0 { fallthrough } else { relative };
                let falsy_target = if instr.b() != 0 { relative } else { fallthrough };
                next_pc = if truthy { truthy_target } else { falsy_target };
            }
            Opcode32::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len()) else {
                    return Ok(None);
                };
                next_pc = target;
            }
            Opcode32::TryBegin => {
                let Some(catch_pc) = native_relative_target(pc, instr.sbx() as i32, code.len()) else {
                    return Ok(None);
                };
                handlers.push((instr.a(), catch_pc));
            }
            Opcode32::TryEnd => {
                if handlers.pop().is_none() {
                    return Ok(None);
                }
            }
            Opcode32::Raise => {
                let Some(_) = function.consts.strings.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                let Some((catch_reg, catch_pc)) = handlers.pop() else {
                    return Ok(None);
                };
                if catch_reg as usize >= regs.len() {
                    return Ok(None);
                }
                regs[catch_reg as usize] = Some(NativeStraightlineValue::Error {
                    symbol: format!("@lk_static_error_{}", ssa_index),
                });
                ssa_index += 1;
                next_pc = catch_pc;
            }
            Opcode32::Return => {
                if instr.b() == 0 {
                    return Ok(Some(native_scalar_main_ir(options, &body, None)));
                }
                if instr.b() != 1 {
                    return Ok(None);
                }
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if matches!(
                    value,
                    NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }
                ) {
                    return Ok(None);
                }
                return Ok(Some(native_straightline_main_ir(options, &body, Some(&value))));
            }
            Opcode32::Nop => {}
            _ => return Ok(None),
        }
        pc = next_pc;
    }
    Ok(Some(native_scalar_main_ir(options, &body, None)))
}

fn native_straightline_call_args(
    regs: &[Option<NativeStraightlineValue>],
    callee: u8,
    count: u8,
) -> Option<Vec<NativeStraightlineValue>> {
    let start = callee as usize + 1;
    let end = start.checked_add(count as usize)?;
    if end > regs.len() {
        return None;
    }
    regs[start..end].iter().cloned().collect()
}

fn native_straightline_call_target(value: NativeStraightlineValue) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match value {
        NativeStraightlineValue::Function(function_index) => Some((function_index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((function_index, captures)),
        _ => None,
    }
}

fn native_scalar_function_needs_blocks(code: &[Instr32]) -> bool {
    code.iter().any(|instr| {
        matches!(
            instr.opcode(),
            Opcode32::LoadBool
                | Opcode32::LoadNil
                | Opcode32::LoadFloat
                | Opcode32::AddFloat
                | Opcode32::SubFloat
                | Opcode32::MulFloat
                | Opcode32::DivFloat
                | Opcode32::ModFloat
                | Opcode32::CmpInt
                | Opcode32::CmpNeInt
                | Opcode32::CmpLtInt
                | Opcode32::CmpLeInt
                | Opcode32::CmpGtInt
                | Opcode32::CmpGeInt
                | Opcode32::Not
                | Opcode32::IsNil
                | Opcode32::Test
                | Opcode32::Jmp
        )
    })
}

fn native_direct_call_targets_need_blocks(artifact: &Module32Artifact, code: &[Instr32]) -> bool {
    code.iter().copied().any(|instr| {
        instr.opcode() == Opcode32::CallDirect
            && artifact
                .module
                .functions
                .get(instr.b() as usize)
                .and_then(|function| {
                    let code = function
                        .code
                        .iter()
                        .copied()
                        .map(Instr32::try_from_raw)
                        .collect::<Result<Vec<_>, _>>()
                        .ok()?;
                    Some(native_scalar_function_needs_blocks(&code) || native_straightline_function_has_call(&code))
                })
                .unwrap_or(false)
    })
}

fn native_straightline_function_has_call(code: &[Instr32]) -> bool {
    code.iter().any(|instr| {
        matches!(
            instr.opcode(),
            Opcode32::LoadFunction
                | Opcode32::MakeClosure
                | Opcode32::Call
                | Opcode32::CallDirect
                | Opcode32::CallNamed
        )
    })
}

fn native_replace_static_aliases(
    regs: &mut [Option<NativeStraightlineValue>],
    globals: &mut [Option<NativeStraightlineValue>],
    target: usize,
    value: &NativeStraightlineValue,
) {
    let Some(Some(old)) = regs.get(target) else {
        return;
    };
    let Some(alias) = native_static_alias_symbol(old).map(str::to_string) else {
        return;
    };
    for reg in regs.iter_mut() {
        if reg
            .as_ref()
            .and_then(native_static_alias_symbol)
            .is_some_and(|symbol| symbol == alias)
        {
            *reg = Some(value.clone());
        }
    }
    for global in globals.iter_mut() {
        if global
            .as_ref()
            .and_then(native_static_alias_symbol)
            .is_some_and(|symbol| symbol == alias)
        {
            *global = Some(value.clone());
        }
    }
}
