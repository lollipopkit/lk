//! Straight-line (branch-free) LLVM IR lowering for scalar entry functions.
//!
//! Walks `Instr32` opcodes that can be resolved to pure LLVM values without
//! branches or phi nodes.  Unsupported opcodes or value shapes cause the
//! lowering to return `None`, which the backend interprets as "unsupported"
//! and reports via the diagnostic module.

use anyhow::Result;

use crate::vm::{ConstHeapValue32Data, Instr32, Module32Artifact, Opcode32};

use super::callee_eval::{native_straightline_function_return, native_straightline_named_call_args};
use super::const_display::{native_const_list_display, native_const_map_display, native_string_const_value};
use super::ir_text::{llvm_float_literal, native_relative_target};
use super::options::LlvmBackendOptions;
use super::output::{emit_native_builtin_call, native_scalar_main_ir, native_straightline_main_ir};
use super::scalar::blocks::compile_native_scalar_main_blocks;
use super::scalar::contains::static_iter_builtin_call;
use super::scalar::facts::native_scalar_block_facts_with_statics_and_functions;
use super::straightline_value::{
    NativeStraightlineValue, NativeStringKeyKind, native_runtime_string_key_kind, native_static_alias_symbol,
    native_static_collection_equality_bool, native_static_compare_bool, native_static_container_test,
    native_static_contains, native_static_equality_bool, native_static_f64_binary, native_static_f64_divisor_nonzero,
    native_static_global, native_static_i64_binary, native_static_i64_divisor_nonzero, native_static_index,
    native_static_int_range, native_static_len, native_static_list_from_values, native_static_list_join,
    native_static_list_push, native_static_load_cell, native_static_map_from_pairs, native_static_map_rest,
    native_static_not, native_static_object_from_fields, native_static_set_index, native_static_slice_from,
    native_static_store_cell, native_static_string_split, native_static_string_starts_with, native_static_to_iter,
    native_static_to_string_value, native_static_truthy,
};

pub(super) fn compile_native_scalar_main_artifact(
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
    let needs_block_lowering = native_scalar_function_needs_blocks(&code)
        || native_direct_call_targets_need_blocks(artifact, &code)
        || native_straightline_function_has_call(&code);
    if needs_block_lowering
        && let Some(scalar_facts) = native_scalar_block_facts_with_statics_and_functions(
            function.register_count as usize,
            artifact.module.globals.len(),
            &artifact.module.globals,
            &function.consts.ints,
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
            &collect_self_recursive_indices(&artifact.module.functions),
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
        if steps > code.len().saturating_mul(65536) {
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
                    regs[instr.a() as usize] = static_iter_builtin_call(
                        artifact,
                        &code,
                        &function.consts.ints,
                        &function.consts.strings,
                        &function.consts.heap_values,
                        builtin,
                        &args,
                        &mut globals,
                        &mut body,
                        &mut ssa_index,
                    )
                    .or_else(|| emit_native_builtin_call(&mut body, builtin, &args, &mut ssa_index));
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
                return Ok(Some(native_straightline_main_ir(options, &body, Some(&value))));
            }
            Opcode32::Nop => {}
            _ => return Ok(None),
        }
        pc = next_pc;
    }
    Ok(Some(native_scalar_main_ir(options, &body, None)))
}

pub(super) fn native_straightline_call_args(
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

pub(super) fn native_straightline_call_target(
    value: NativeStraightlineValue,
) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match value {
        NativeStraightlineValue::Function(function_index) => Some((function_index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((function_index, captures)),
        _ => None,
    }
}

pub(super) fn native_scalar_function_needs_blocks(code: &[Instr32]) -> bool {
    code.iter().any(|instr| {
        matches!(
            instr.opcode(),
            Opcode32::LoadFloat
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
                | Opcode32::SetIndex
                | Opcode32::ListPush
                | Opcode32::Test
                | Opcode32::Jmp
        )
    })
}

pub(super) fn native_direct_call_targets_need_blocks(artifact: &Module32Artifact, code: &[Instr32]) -> bool {
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

pub(super) fn native_straightline_function_has_call(code: &[Instr32]) -> bool {
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

fn collect_self_recursive_indices(functions: &[crate::vm::Function32Data]) -> Vec<u16> {
    let mut indices = Vec::new();
    for (idx, function) in functions.iter().enumerate() {
        let Ok(code) = function
            .code
            .iter()
            .copied()
            .map(Instr32::try_from_raw)
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        for instr in code.iter().copied() {
            if instr.opcode() == Opcode32::CallDirect && instr.b() as usize == idx {
                indices.push(idx as u16);
                break;
            }
        }
    }
    indices
}
