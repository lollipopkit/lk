//! Straight-line (branch-free) LLVM IR lowering for scalar entry functions.
//!
//! Walks `Instr` opcodes that can be resolved to pure LLVM values without
//! branches or phi nodes.  Unsupported opcodes or value shapes cause the
//! lowering to return `None`, which the backend interprets as "unsupported"
//! and reports via the diagnostic module.

use anyhow::Result;

use crate::vm::{ConstHeapValueData, Instr, ModuleArtifact, Opcode};

use super::callee_eval::{native_straightline_function_return, native_straightline_named_call_args};
use super::const_display::{native_const_list_display, native_const_map_display, native_string_const_value};
use super::ir_text::{llvm_float_literal, native_relative_target};
use super::known_key::native_known_string_key;
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
    native_static_store_cell, native_static_string_split, native_static_to_iter, native_static_to_string_value,
    native_static_truthy,
};

pub(super) fn compile_native_scalar_main_artifact(
    artifact: &ModuleArtifact,
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
        .map(Instr::try_from_raw)
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
            Opcode::LoadNil => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
            }
            Opcode::LoadBool => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(i64::from(instr.b() != 0).to_string()));
            }
            Opcode::LoadInt => {
                let Some(value) = function.consts.ints.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(value.to_string()));
            }
            Opcode::LoadFloat => {
                let Some(value) = function.consts.floats.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(llvm_float_literal(*value)));
            }
            Opcode::LoadString => {
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
            Opcode::LoadHeapConst => {
                let Some(value) = function.consts.heap_values.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = match value {
                    ConstHeapValueData::LongString(value) => {
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
                    ConstHeapValueData::List(values) => {
                        let Some(value) = native_const_list_display(values) else {
                            return Ok(None);
                        };
                        Some(NativeStraightlineValue::List {
                            symbol: format!("@lk_const_heap_list_{}", instr.bx()),
                            value,
                            elements: values.clone(),
                        })
                    }
                    ConstHeapValueData::Map(values) => {
                        let Some(value) = native_const_map_display(values) else {
                            return Ok(None);
                        };
                        Some(NativeStraightlineValue::Map {
                            symbol: format!("@lk_const_heap_map_{}", instr.bx()),
                            value,
                            entries: values.clone(),
                        })
                    }
                    ConstHeapValueData::UpvalCell(value) => Some(NativeStraightlineValue::Cell {
                        symbol: format!("@lk_const_heap_cell_{}", instr.bx()),
                        value: Box::new(match value.as_ref() {
                            crate::vm::ConstRuntimeValueData::Nil => NativeStraightlineValue::Nil,
                            crate::vm::ConstRuntimeValueData::Bool(value) => {
                                NativeStraightlineValue::Bool(i64::from(*value).to_string())
                            }
                            crate::vm::ConstRuntimeValueData::Int(value) => {
                                NativeStraightlineValue::I64(value.to_string())
                            }
                            crate::vm::ConstRuntimeValueData::Float(value) => {
                                NativeStraightlineValue::F64(llvm_float_literal(*value))
                            }
                            _ => return Ok(None),
                        }),
                    }),
                };
            }
            Opcode::LoadFunction => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Function(instr.bx()));
            }
            Opcode::MakeClosure => {
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
            Opcode::LoadCellVal => {
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
            Opcode::StoreCellVal => {
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
            Opcode::Move => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
            }
            Opcode::Move2 => {
                let Some(first) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() || instr.b() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(first);
                let Some(second) = regs.get(instr.c() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                regs[instr.b() as usize] = Some(second);
            }
            Opcode::SetGlobal => {
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(global) = globals.get_mut(instr.bx() as usize) else {
                    return Ok(None);
                };
                *global = Some(value);
            }
            Opcode::GetGlobal => {
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
            Opcode::AddInt
            | Opcode::SubInt
            | Opcode::MulInt
            | Opcode::DivInt
            | Opcode::ModInt
            | Opcode::MinInt
            | Opcode::MaxInt
            | Opcode::AddMulInt
            | Opcode::Add2Int
            | Opcode::MidInt => {
                if matches!(instr.opcode(), Opcode::AddMulInt | Opcode::Add2Int | Opcode::MidInt) {
                    if instr.opcode() == Opcode::MidInt {
                        let Some(NativeStraightlineValue::I64(lhs)) =
                            regs.get(instr.b() as usize).and_then(Clone::clone)
                        else {
                            return Ok(None);
                        };
                        let Some(NativeStraightlineValue::I64(rhs)) =
                            regs.get(instr.c() as usize).and_then(Clone::clone)
                        else {
                            return Ok(None);
                        };
                        let name = if let (Ok(lhs), Ok(rhs)) = (lhs.parse::<i64>(), rhs.parse::<i64>()) {
                            lhs.wrapping_add(rhs).wrapping_div(2).to_string()
                        } else {
                            let sum = format!("%r{}_{}", instr.a(), ssa_index);
                            ssa_index += 1;
                            let name = format!("%r{}_{}", instr.a(), ssa_index);
                            ssa_index += 1;
                            body.push_str(&format!("  {sum} = add i64 {lhs}, {rhs}\n"));
                            body.push_str(&format!("  {name} = sdiv i64 {sum}, 2\n"));
                            name
                        };
                        if instr.a() as usize >= regs.len() {
                            return Ok(None);
                        }
                        regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(name));
                        continue;
                    }
                    let Some(NativeStraightlineValue::I64(acc)) = regs.get(instr.a() as usize).and_then(Clone::clone)
                    else {
                        return Ok(None);
                    };
                    let Some(NativeStraightlineValue::I64(lhs)) = regs.get(instr.b() as usize).and_then(Clone::clone)
                    else {
                        return Ok(None);
                    };
                    let Some(NativeStraightlineValue::I64(rhs)) = regs.get(instr.c() as usize).and_then(Clone::clone)
                    else {
                        return Ok(None);
                    };
                    let name = if let (Ok(acc), Ok(lhs), Ok(rhs)) =
                        (acc.parse::<i64>(), lhs.parse::<i64>(), rhs.parse::<i64>())
                    {
                        if instr.opcode() == Opcode::AddMulInt {
                            acc.wrapping_add(lhs.wrapping_mul(rhs)).to_string()
                        } else {
                            acc.wrapping_add(lhs).wrapping_add(rhs).to_string()
                        }
                    } else {
                        let partial = format!("%r{}_{}", instr.a(), ssa_index);
                        ssa_index += 1;
                        let name = format!("%r{}_{}", instr.a(), ssa_index);
                        ssa_index += 1;
                        if instr.opcode() == Opcode::AddMulInt {
                            body.push_str(&format!("  {partial} = mul i64 {lhs}, {rhs}\n"));
                            body.push_str(&format!("  {name} = add i64 {acc}, {partial}\n"));
                        } else {
                            body.push_str(&format!("  {partial} = add i64 {acc}, {lhs}\n"));
                            body.push_str(&format!("  {name} = add i64 {partial}, {rhs}\n"));
                        }
                        name
                    };
                    if instr.a() as usize >= regs.len() {
                        return Ok(None);
                    }
                    regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(name));
                    continue;
                }
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
                    if matches!(instr.opcode(), Opcode::DivInt | Opcode::ModInt)
                        && !native_static_i64_divisor_nonzero(&rhs).unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    let op = match instr.opcode() {
                        Opcode::AddInt => "add",
                        Opcode::SubInt => "sub",
                        Opcode::MulInt => "mul",
                        Opcode::DivInt => "sdiv",
                        Opcode::ModInt => "srem",
                        Opcode::MinInt | Opcode::MaxInt => "select",
                        _ => unreachable!("opcode matched above"),
                    };
                    let name = format!("%r{}_{}", instr.a(), ssa_index);
                    ssa_index += 1;
                    if matches!(instr.opcode(), Opcode::MinInt | Opcode::MaxInt) {
                        let pred = if instr.opcode() == Opcode::MinInt { "slt" } else { "sgt" };
                        let cond = format!("%r{}_{}", instr.a(), ssa_index);
                        ssa_index += 1;
                        body.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
                        body.push_str(&format!("  {name} = {op} i1 {cond}, i64 {lhs}, i64 {rhs}\n"));
                    } else {
                        body.push_str(&format!("  {name} = {op} i64 {lhs}, {rhs}\n"));
                    }
                    name
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(name));
            }
            Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI => {
                let Some(NativeStraightlineValue::I64(lhs)) = regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let rhs = instr.sc() as i64;
                if instr.opcode() == Opcode::ModIntI && rhs == 0 {
                    return Ok(None);
                }
                let name = if lhs.starts_with('%') {
                    let op = match instr.opcode() {
                        Opcode::AddIntI => "add",
                        Opcode::MulIntI => "mul",
                        Opcode::ModIntI => "srem",
                        _ => unreachable!("opcode matched above"),
                    };
                    let name = format!("%r{}_{}", instr.a(), ssa_index);
                    ssa_index += 1;
                    body.push_str(&format!("  {name} = {op} i64 {lhs}, {rhs}\n"));
                    name
                } else if let Ok(lhs) = lhs.parse::<i64>() {
                    match instr.opcode() {
                        Opcode::AddIntI => lhs.wrapping_add(rhs),
                        Opcode::MulIntI => lhs.wrapping_mul(rhs),
                        Opcode::ModIntI => lhs.wrapping_rem(rhs),
                        _ => unreachable!("opcode matched above"),
                    }
                    .to_string()
                } else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(name));
            }
            Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat => {
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
                    if matches!(instr.opcode(), Opcode::DivFloat | Opcode::ModFloat)
                        && !native_static_f64_divisor_nonzero(&rhs).unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    let op = match instr.opcode() {
                        Opcode::AddFloat => "fadd",
                        Opcode::SubFloat => "fsub",
                        Opcode::MulFloat => "fmul",
                        Opcode::DivFloat => "fdiv",
                        Opcode::ModFloat => "frem",
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
            Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt => {
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
                            Opcode::CmpInt => "icmp eq",
                            Opcode::CmpNeInt => "icmp ne",
                            Opcode::CmpLtInt => "icmp slt",
                            Opcode::CmpLeInt => "icmp sle",
                            Opcode::CmpGtInt => "icmp sgt",
                            Opcode::CmpGeInt => "icmp sge",
                            _ => unreachable!("opcode matched above"),
                        };
                        (op, "i64", lhs, rhs)
                    }
                    (NativeStraightlineValue::F64(lhs), NativeStraightlineValue::F64(rhs)) => {
                        let op = match instr.opcode() {
                            Opcode::CmpInt => "fcmp oeq",
                            Opcode::CmpNeInt => "fcmp une",
                            Opcode::CmpLtInt => "fcmp olt",
                            Opcode::CmpLeInt => "fcmp ole",
                            Opcode::CmpGtInt => "fcmp ogt",
                            Opcode::CmpGeInt => "fcmp oge",
                            _ => unreachable!("opcode matched above"),
                        };
                        (op, "double", lhs, rhs)
                    }
                    (NativeStraightlineValue::Bool(lhs), NativeStraightlineValue::Bool(rhs))
                        if matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt) =>
                    {
                        let op = match instr.opcode() {
                            Opcode::CmpInt => "icmp eq",
                            Opcode::CmpNeInt => "icmp ne",
                            _ => unreachable!("opcode matched by guard"),
                        };
                        (op, "i64", lhs, rhs)
                    }
                    (
                        NativeStraightlineValue::String { value: lhs, .. },
                        NativeStraightlineValue::String { value: rhs, .. },
                    ) if matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt) => {
                        if instr.a() as usize >= regs.len() {
                            return Ok(None);
                        }
                        regs[instr.a() as usize] = Some(native_static_equality_bool(lhs == rhs, instr.opcode()));
                        pc = next_pc;
                        continue;
                    }
                    (NativeStraightlineValue::Nil, NativeStraightlineValue::Nil)
                        if matches!(instr.opcode(), Opcode::CmpInt | Opcode::CmpNeInt) =>
                    {
                        let value = i64::from(instr.opcode() == Opcode::CmpInt).to_string();
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
            Opcode::Not => {
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
            Opcode::IsNil => {
                if regs.get(instr.b() as usize).and_then(Clone::clone).is_none() || instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let is_nil = matches!(regs.get(instr.b() as usize), Some(Some(NativeStraightlineValue::Nil)));
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(i64::from(is_nil).to_string()));
            }
            Opcode::IsList | Opcode::IsMap => {
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
            Opcode::Len => {
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
            Opcode::ToIter => {
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
            Opcode::GetIndex | Opcode::GetList => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) = native_known_string_key(function, pc, format!("@lk_known_key_{pc}"))
                    .or_else(|| regs.get(instr.c() as usize).and_then(Clone::clone))
                else {
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
            Opcode::GetFieldK => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key_text) = function.consts.strings.get(instr.c() as usize) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let Some(key_value) = native_string_const_value(key_text) else {
                    return Ok(None);
                };
                let key = NativeStraightlineValue::String {
                    symbol: String::new(),
                    value: key_value,
                    len: key_text.len(),
                    key_kind: NativeStringKeyKind::Short,
                };
                let symbol = format!("@lk_field_k_str_{}", ssa_index);
                ssa_index += 1;
                let Some(value) = native_static_index(target, key, symbol) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = Some(value);
            }
            Opcode::SetIndex => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) = native_known_string_key(function, pc, format!("@lk_known_set_key_{pc}"))
                    .or_else(|| regs.get(instr.b() as usize).and_then(Clone::clone))
                else {
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
            Opcode::SetFieldK => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key_text) = function.consts.strings.get(instr.c() as usize) else {
                    return Ok(None);
                };
                let Some(key_value) = native_string_const_value(key_text) else {
                    return Ok(None);
                };
                let key = NativeStraightlineValue::String {
                    symbol: String::new(),
                    value: key_value,
                    len: key_text.len(),
                    key_kind: NativeStringKeyKind::Short,
                };
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(value) = native_static_set_index(target, key, value) else {
                    return Ok(None);
                };
                native_replace_static_aliases(&mut regs, &mut globals, instr.a() as usize, &value);
                regs[instr.a() as usize] = Some(value);
            }
            Opcode::ListPush => {
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
            Opcode::Contains => {
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
            Opcode::SliceFrom => {
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
            Opcode::MapRest => {
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
            Opcode::NewList => {
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
            Opcode::NewMap => {
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
            Opcode::NewRange => {
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
            Opcode::NewObject => {
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
            Opcode::ToString => {
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
            Opcode::ConcatString => {
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
            Opcode::ConcatN => {
                // N-ary concat: collect string parts from consecutive registers.
                let count = instr.c() as usize;
                let mut parts = Vec::new();
                for i in 0..count {
                    let reg_idx = instr.b() as usize + i;
                    let Some(NativeStraightlineValue::String { value: part, .. }) =
                        regs.get(reg_idx).and_then(Clone::clone)
                    else {
                        return Ok(None);
                    };
                    parts.push(part);
                }
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_concat_str_{}", ssa_index);
                ssa_index += 1;
                let value = parts.join("");
                regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                    symbol,
                    len: value.chars().count(),
                    key_kind: native_runtime_string_key_kind(&value),
                    value,
                });
            }
            Opcode::StringSplit => {
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
            Opcode::ListJoin => {
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
            Opcode::Call => {
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
            Opcode::CallDirect => {
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
            Opcode::CallNamed => {
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
            opcode if opcode.is_compare_test() => {
                let Some(lhs) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(rhs) = compare_test_rhs_static_value(opcode, instr, &regs) else {
                    return Ok(None);
                };
                let Some(compare_opcode) = compare_test_compare_opcode(instr.opcode()) else {
                    return Ok(None);
                };
                let Some(value) = native_static_compare_bool(&lhs, &rhs, compare_opcode) else {
                    return Ok(None);
                };
                let jmp = code.get(pc + 1).copied().filter(|instr| instr.opcode() == Opcode::Jmp);
                let Some(jmp) = jmp else {
                    return Ok(None);
                };
                let Some(target) = native_relative_target(pc + 1, jmp.sj_arg(), code.len()) else {
                    return Ok(None);
                };
                next_pc = if value == compare_test_jump_when(opcode, instr) {
                    target
                } else {
                    pc + 2
                };
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(truthy) = native_static_truthy(&value) else {
                    return Ok(None);
                };
                let fallthrough = pc + 1;
                let Some(relative) = (match instr.opcode() {
                    Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code.len()),
                    Opcode::BrFalse | Opcode::BrTrue => native_relative_target(pc, instr.sbx() as i32, code.len()),
                    _ => None,
                }) else {
                    return Ok(None);
                };
                let truthy_target =
                    if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue {
                        relative
                    } else {
                        fallthrough
                    };
                let falsy_target =
                    if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse {
                        relative
                    } else {
                        fallthrough
                    };
                next_pc = if truthy { truthy_target } else { falsy_target };
            }
            Opcode::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len()) else {
                    return Ok(None);
                };
                next_pc = target;
            }
            Opcode::TryBegin => {
                let Some(catch_pc) = native_relative_target(pc, instr.sbx() as i32, code.len()) else {
                    return Ok(None);
                };
                handlers.push((instr.a(), catch_pc));
            }
            Opcode::TryEnd => {
                if handlers.pop().is_none() {
                    return Ok(None);
                }
            }
            Opcode::Raise => {
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
            opcode if opcode.is_return() => {
                if instr.return_count() == 0 {
                    return Ok(Some(native_scalar_main_ir(options, &body, None)));
                }
                if instr.return_count() != 1 {
                    return Ok(None);
                }
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                return Ok(Some(native_straightline_main_ir(options, &body, Some(&value))));
            }
            Opcode::Nop => {}
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

pub(super) fn native_scalar_function_needs_blocks(code: &[Instr]) -> bool {
    code.iter().any(|instr| {
        matches!(
            instr.opcode(),
            Opcode::LoadFloat
                | Opcode::AddFloat
                | Opcode::SubFloat
                | Opcode::MulFloat
                | Opcode::DivFloat
                | Opcode::ModFloat
                | Opcode::CmpInt
                | Opcode::CmpNeInt
                | Opcode::CmpLtInt
                | Opcode::CmpLeInt
                | Opcode::CmpGtInt
                | Opcode::CmpGeInt
                | Opcode::Not
                | Opcode::IsNil
                | Opcode::SetIndex
                | Opcode::ListPush
                | Opcode::Test
                | Opcode::BrFalse
                | Opcode::BrTrue
                | Opcode::BrNil
                | Opcode::BrNotNil
                | Opcode::BrEqZeroInt
                | Opcode::BrNeZeroInt
                | Opcode::BrEqIntI4
                | Opcode::BrNeIntI4
                | Opcode::BrModEqZeroIntI4
                | Opcode::BrModNeZeroIntI4
                | Opcode::Jmp
        )
    })
}

pub(super) fn native_direct_call_targets_need_blocks(artifact: &ModuleArtifact, code: &[Instr]) -> bool {
    code.iter().copied().any(|instr| {
        instr.opcode() == Opcode::CallDirect
            && artifact
                .module
                .functions
                .get(instr.b() as usize)
                .and_then(|function| {
                    let code = function
                        .code
                        .iter()
                        .copied()
                        .map(Instr::try_from_raw)
                        .collect::<Result<Vec<_>, _>>()
                        .ok()?;
                    Some(native_scalar_function_needs_blocks(&code) || native_straightline_function_has_call(&code))
                })
                .unwrap_or(false)
    })
}

pub(super) fn native_straightline_function_has_call(code: &[Instr]) -> bool {
    code.iter().any(|instr| {
        matches!(
            instr.opcode(),
            Opcode::LoadFunction | Opcode::MakeClosure | Opcode::Call | Opcode::CallDirect | Opcode::CallNamed
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

fn collect_self_recursive_indices(functions: &[crate::vm::FunctionData]) -> Vec<u16> {
    let mut indices = Vec::new();
    for (idx, function) in functions.iter().enumerate() {
        let Ok(code) = function
            .code
            .iter()
            .copied()
            .map(Instr::try_from_raw)
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        for instr in code.iter().copied() {
            if instr.opcode() == Opcode::CallDirect && instr.b() as usize == idx {
                indices.push(idx as u16);
                break;
            }
        }
    }
    indices
}

fn compare_test_compare_opcode(opcode: Opcode) -> Option<Opcode> {
    Some(match opcode {
        Opcode::TestEqInt | Opcode::TestEqIntI => Opcode::CmpInt,
        Opcode::TestNeInt | Opcode::TestNeIntI => Opcode::CmpNeInt,
        Opcode::TestLtInt | Opcode::TestLtIntI => Opcode::CmpLtInt,
        Opcode::TestLeInt | Opcode::TestLeIntI => Opcode::CmpLeInt,
        Opcode::TestGtInt | Opcode::TestGtIntI => Opcode::CmpGtInt,
        Opcode::TestGeInt | Opcode::TestGeIntI => Opcode::CmpGeInt,
        _ => return None,
    })
}

fn compare_test_rhs_static_value(
    opcode: Opcode,
    instr: Instr,
    regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    if opcode.is_int_immediate_compare_test() {
        Some(NativeStraightlineValue::I64(i64::from(instr.sc()).to_string()))
    } else {
        regs.get(instr.b() as usize).and_then(Clone::clone)
    }
}

fn compare_test_jump_when(opcode: Opcode, instr: Instr) -> bool {
    if opcode.is_int_immediate_compare_test() {
        instr.b() != 0
    } else {
        instr.c() != 0
    }
}
