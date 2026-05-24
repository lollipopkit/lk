use anyhow::{Result, bail};

use crate::{
    stmt::{
        Program,
        import::{ImportSource, ImportStmt},
    },
    vm::{Compiler32, ConstHeapValue32Data, Instr32, Module32Artifact, Opcode32},
};

use super::{
    callee_eval::{native_straightline_function_return, native_straightline_named_call_args},
    const_display::{
        llvm_string_constant, native_const_list_display, native_const_map_display, native_string_const_value,
    },
    ir_text::{
        emit_branch_to_next, llvm_float_literal, native_label, native_relative_target, native_scalar_main_header,
        next_tmp, reg_in_bounds,
    },
    options::{LlvmBackendOptions, OptLevel},
    scalar_emit::{
        emit_f64_binary_block, emit_i64_binary_block, emit_native_return_print, emit_numeric_compare_block,
        emit_scalar_equality_block,
    },
    scalar_facts::{NativeScalarFacts, NativeScalarKind, native_scalar_block_facts},
    straightline_value::{
        NativeStraightlineValue, NativeStringKeyKind, native_runtime_string_key_kind, native_static_alias_symbol,
        native_static_collection_equality_bool, native_static_compare_bool, native_static_container_test,
        native_static_contains, native_static_equality_bool, native_static_f64_binary,
        native_static_f64_divisor_nonzero, native_static_i64_binary, native_static_i64_divisor_nonzero,
        native_static_index, native_static_int_range, native_static_len, native_static_list_from_values,
        native_static_load_cell, native_static_map_from_pairs, native_static_map_rest, native_static_not,
        native_static_object_from_fields, native_static_set_index, native_static_slice_from, native_static_store_cell,
        native_static_to_iter, native_static_to_string_value, native_static_truthy,
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
    if !artifact.imports.is_empty() {
        return format!(
            "imports are not native-lowerable yet ({})",
            artifact
                .imports
                .iter()
                .map(import_stmt_label)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

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
    if native_scalar_function_needs_blocks(&code) && native_straightline_function_has_call(&code) {
        return "control-flow lowering with direct calls is not native-lowerable yet".to_string();
    }
    if native_scalar_function_needs_blocks(&code)
        && native_scalar_block_facts(function.register_count as usize, artifact.module.globals.len(), &code).is_none()
    {
        return "scalar block facts could not classify the entry function".to_string();
    }
    "entry function contains an unsupported Instr32/native value shape".to_string()
}

fn import_stmt_label(import: &ImportStmt) -> String {
    match import {
        ImportStmt::Module { module } => module.clone(),
        ImportStmt::File { path } => format!("file:{path}"),
        ImportStmt::Items { source, .. } => import_source_label(source),
        ImportStmt::Namespace { alias, source } => format!("{} as {alias}", import_source_label(source)),
        ImportStmt::ModuleAlias { module, alias } => format!("{module} as {alias}"),
    }
}

fn import_source_label(source: &ImportSource) -> String {
    match source {
        ImportSource::Module(module) => module.clone(),
        ImportSource::File(path) => format!("file:{path}"),
    }
}

fn compile_native_scalar_main_artifact(
    artifact: &Module32Artifact,
    options: &LlvmBackendOptions,
) -> Result<Option<String>> {
    if !artifact.imports.is_empty() {
        return Ok(None);
    }
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
    if native_scalar_function_needs_blocks(&code)
        && !native_straightline_function_has_call(&code)
        && let Some(scalar_facts) =
            native_scalar_block_facts(function.register_count as usize, artifact.module.globals.len(), &code)
    {
        return compile_native_scalar_main_blocks(
            options,
            function.register_count as usize,
            artifact.module.globals.len(),
            &function.consts.ints,
            &function.consts.floats,
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
                let Some(value) = globals.get(instr.bx() as usize).and_then(Clone::clone) else {
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
            Opcode32::Call => {
                if instr.a() != instr.b() {
                    return Ok(None);
                }
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some((function_index, captures)) = native_straightline_call_target(target) else {
                    return Ok(None);
                };
                let Some(args) = native_straightline_call_args(&regs, instr.b(), instr.c()) else {
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
                | Opcode32::GetGlobal
        )
    })
}

fn native_straightline_function_has_call(code: &[Instr32]) -> bool {
    code.iter()
        .any(|instr| matches!(instr.opcode(), Opcode32::LoadFunction | Opcode32::Call))
}

fn compile_native_scalar_main_blocks(
    options: &LlvmBackendOptions,
    register_count: usize,
    global_count: usize,
    int_consts: &[i64],
    float_consts: &[f64],
    code: &[Instr32],
    scalar_facts: &NativeScalarFacts,
) -> Result<Option<String>> {
    let mut ir = native_scalar_main_header(options);
    for reg in 0..register_count {
        ir.push_str(&format!("  %r{reg}.slot = alloca i64\n"));
    }
    for global in 0..global_count {
        ir.push_str(&format!("  %g{global}.slot = alloca i64\n"));
    }
    ir.push_str("  br label %bb0\n\n");

    let mut tmp_index = 0usize;
    for (pc, instr) in code.iter().copied().enumerate() {
        ir.push_str(&format!("bb{pc}:\n"));
        match instr.opcode() {
            Opcode32::LoadString | Opcode32::LoadHeapConst => return Ok(None),
            Opcode32::LoadNil => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadInt => {
                let Some(value) = int_consts.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadFloat => {
                let Some(value) = float_consts.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                ir.push_str(&format!(
                    "  store double {}, ptr %r{}.slot\n",
                    llvm_float_literal(*value),
                    instr.a()
                ));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::LoadBool => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let value = i64::from(instr.b() != 0);
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Move => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let value = next_tmp(&mut tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.b()));
                ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                emit_i64_binary_block(&mut ir, instr, &mut tmp_index);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::AddFloat | Opcode32::SubFloat | Opcode32::MulFloat | Opcode32::DivFloat | Opcode32::ModFloat => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                emit_f64_binary_block(&mut ir, instr, &mut tmp_index);
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt => {
                if !three_regs_in_bounds(register_count, instr) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let Some(rhs_kind) = scalar_facts.register_kind_before(pc, instr.c()) else {
                    return Ok(None);
                };
                if kind == rhs_kind && kind.is_numeric() {
                    emit_numeric_compare_block(&mut ir, instr, kind, &mut tmp_index);
                } else if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt) {
                    emit_scalar_equality_block(&mut ir, instr, kind, rhs_kind, &mut tmp_index);
                } else {
                    return Ok(None);
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Test => {
                if !reg_in_bounds(register_count, instr.a()) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.a()) else {
                    return Ok(None);
                };
                let fallthrough = pc + 1;
                let Some(relative) = native_relative_target(pc, instr.c() as i8 as i32, code.len()) else {
                    return Ok(None);
                };
                let truthy_target = if instr.b() != 0 { fallthrough } else { relative };
                let falsy_target = if instr.b() != 0 { relative } else { fallthrough };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
                        ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                        ir.push_str(&format!(
                            "  br i1 {cond}, label {}, label {}\n",
                            native_label(truthy_target, code.len()),
                            native_label(falsy_target, code.len())
                        ));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!("  br label {}\n", native_label(falsy_target, code.len())));
                    }
                    NativeScalarKind::I64 | NativeScalarKind::F64 => {
                        ir.push_str(&format!("  br label {}\n", native_label(truthy_target, code.len())));
                    }
                }
            }
            Opcode32::Not => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        let out = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                        ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                        ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
                        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
                    }
                    _ => return Ok(None),
                }
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::IsNil => {
                if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let value = i64::from(kind == NativeScalarKind::Nil);
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len()) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
            }
            Opcode32::GetGlobal => {
                if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.global_kind_before(pc, instr.bx()) else {
                    return Ok(None);
                };
                let value = next_tmp(&mut tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!("  {value} = load {ty}, ptr %g{}.slot\n", instr.bx()));
                ir.push_str(&format!("  store {ty} {value}, ptr %r{}.slot\n", instr.a()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::SetGlobal => {
                if !reg_in_bounds(register_count, instr.a()) || instr.bx() as usize >= global_count {
                    return Ok(None);
                }
                let Some(kind) = scalar_facts.register_kind_before(pc, instr.a()) else {
                    return Ok(None);
                };
                let value = next_tmp(&mut tmp_index);
                let ty = kind.llvm_type();
                ir.push_str(&format!("  {value} = load {ty}, ptr %r{}.slot\n", instr.a()));
                ir.push_str(&format!("  store {ty} {value}, ptr %g{}.slot\n", instr.bx()));
                emit_branch_to_next(&mut ir, pc, code.len());
            }
            Opcode32::Return => {
                if instr.b() == 0 {
                    ir.push_str("  ret i32 0\n");
                } else if instr.b() == 1 && reg_in_bounds(register_count, instr.a()) {
                    let Some(kind) = scalar_facts.register_kind_before(pc, instr.a()) else {
                        return Ok(None);
                    };
                    emit_native_return_print(&mut ir, pc, instr.a(), kind, &mut tmp_index);
                    ir.push_str("  ret i32 0\n");
                } else {
                    return Ok(None);
                }
            }
            Opcode32::Nop => emit_branch_to_next(&mut ir, pc, code.len()),
            _ => return Ok(None),
        }
        ir.push('\n');
    }
    ir.push_str("exit:\n");
    ir.push_str("  ret i32 0\n");
    ir.push_str("lk_divisor_zero:\n");
    ir.push_str("  ret i32 1\n");
    ir.push_str("}\n");
    Ok(Some(ir))
}

fn three_regs_in_bounds(register_count: usize, instr: Instr32) -> bool {
    reg_in_bounds(register_count, instr.a())
        && reg_in_bounds(register_count, instr.b())
        && reg_in_bounds(register_count, instr.c())
}

fn native_scalar_main_ir(options: &LlvmBackendOptions, body: &str, return_value: Option<&str>) -> String {
    let mut ir = native_scalar_main_header(options);
    ir.push_str(body);
    if let Some(value) = return_value {
        ir.push_str(&format!(
            "  %print = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
        ));
    }
    ir.push_str("  ret i32 0\n");
    ir.push_str("}\n");
    ir
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

fn native_straightline_main_ir(
    options: &LlvmBackendOptions,
    body: &str,
    return_value: Option<&NativeStraightlineValue>,
) -> String {
    let mut ir = native_scalar_main_header(options);
    ir.push_str(body);
    let mut globals = String::new();
    if let Some(value) = return_value {
        match value {
            NativeStraightlineValue::I64(value) => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
                ));
            }
            NativeStraightlineValue::F64(value) => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_f64_fmt, double {value})\n"
                ));
            }
            NativeStraightlineValue::Bool(value) => {
                ir.push_str(&format!("  %bool.text = icmp ne i64 {value}, 0\n"));
                ir.push_str("  %bool.ptr = select i1 %bool.text, ptr @lk_bool_true, ptr @lk_bool_false\n");
                ir.push_str("  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %bool.ptr)\n");
            }
            NativeStraightlineValue::Nil => {
                ir.push_str("  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_nil_text)\n");
            }
            NativeStraightlineValue::String { symbol, value, .. }
            | NativeStraightlineValue::List { symbol, value, .. }
            | NativeStraightlineValue::Map { symbol, value, .. }
            | NativeStraightlineValue::Object { symbol, value, .. } => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
                ));
                globals.push_str(&llvm_string_constant(symbol, value));
            }
            NativeStraightlineValue::Error { symbol } => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
                ));
                globals.push_str(&llvm_string_constant(symbol, "<value>"));
            }
            NativeStraightlineValue::Function(_)
            | NativeStraightlineValue::Closure { .. }
            | NativeStraightlineValue::Cell { .. } => {}
        }
    }
    ir.push_str("  ret i32 0\n");
    ir.push_str("}\n");
    ir.push_str(&globals);
    ir
}
