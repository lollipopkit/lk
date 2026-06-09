use anyhow::Result;

use crate::vm::{ConstHeapValueData, ConstRuntimeValueData, FunctionData, Instr, ModuleArtifact, Opcode};

use super::{
    const_display::native_string_const_value,
    ir_text::{llvm_float_literal, native_relative_target},
    known_key::native_known_string_key,
    output::emit_native_builtin_call,
    straightline_value::{
        NativeStraightlineValue, NativeStringKeyKind, native_runtime_string_key_kind, native_static_alias_symbol,
        native_static_collection_equality_bool, native_static_compare_bool, native_static_container_test,
        native_static_contains, native_static_equality_bool, native_static_f64_binary,
        native_static_f64_divisor_nonzero, native_static_global, native_static_i64_binary,
        native_static_i64_divisor_nonzero, native_static_index, native_static_int_range, native_static_len,
        native_static_list_from_values, native_static_list_join, native_static_list_push, native_static_load_cell,
        native_static_map_from_pairs, native_static_map_rest, native_static_not, native_static_object_from_fields,
        native_static_set_index, native_static_slice_from, native_static_store_cell, native_static_string_split,
        native_static_to_iter, native_static_to_string_value, native_static_truthy,
        native_straightline_heap_const_value,
    },
};

pub(super) fn native_straightline_function_return(
    artifact: &ModuleArtifact,
    function_index: usize,
    args: &[NativeStraightlineValue],
    captures: &[NativeStraightlineValue],
    globals: &mut [Option<NativeStraightlineValue>],
    depth: usize,
    body: &mut String,
    ssa_index: &mut usize,
) -> Result<Option<NativeStraightlineValue>> {
    if depth > 8 {
        return Ok(None);
    }
    let Some(function) = artifact.module.functions.get(function_index) else {
        return Ok(None);
    };
    if function.capture_count as usize != captures.len() || function.param_count as usize != args.len() {
        return Ok(None);
    }
    let code = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()?;
    let mut regs: Vec<Option<NativeStraightlineValue>> = vec![None; function.register_count as usize];
    if args.len() > regs.len() {
        return Ok(None);
    }
    for (index, value) in args.iter().cloned().enumerate() {
        regs[index] = Some(value);
    }
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
                    symbol: format!("@lk_func{function_index}_str_{}", instr.bx()),
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
                regs[instr.a() as usize] = native_straightline_heap_const_value(function_index, instr.bx(), value);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::LoadFunction => {
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(NativeStraightlineValue::Function(instr.bx()));
            }
            Opcode::LoadCapture => {
                let Some(value) = captures.get(instr.bx() as usize).cloned() else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                regs[instr.a() as usize] = Some(value);
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
                native_replace_static_aliases(&mut regs, globals, instr.a() as usize, &value);
                regs[instr.a() as usize] = Some(value);
            }
            Opcode::MakeClosure => {
                let Some(function) = artifact.module.functions.get(instr.b() as usize) else {
                    return Ok(None);
                };
                let start = instr.c() as usize;
                let end = start
                    .checked_add(function.capture_count as usize)
                    .ok_or_else(|| anyhow::anyhow!("LLVM native closure capture range overflow"))?;
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
            Opcode::SetGlobal => {
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(global) = globals.get_mut(instr.bx() as usize) else {
                    return Ok(None);
                };
                *global = Some(value);
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
                            let sum = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                            *ssa_index += 1;
                            let name = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                            *ssa_index += 1;
                            body.push_str(&format!("  {sum} = add i64 {lhs}, {rhs}\n"));
                            body.push_str(&format!("  {name} = sdiv i64 {sum}, 2\n"));
                            name
                        };
                        if instr.a() as usize >= regs.len() {
                            return Ok(None);
                        }
                        regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(name));
                        pc = next_pc;
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
                        let partial = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                        *ssa_index += 1;
                        let name = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                        *ssa_index += 1;
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
                    pc = next_pc;
                    continue;
                }
                if instr.opcode() == Opcode::AddInt
                    && let (
                        Some(NativeStraightlineValue::String { value: lhs, .. }),
                        Some(NativeStraightlineValue::String { value: rhs, .. }),
                    ) = (
                        regs.get(instr.b() as usize).and_then(Clone::clone),
                        regs.get(instr.c() as usize).and_then(Clone::clone),
                    )
                {
                    let value = format!("{lhs}{rhs}");
                    let symbol = format!("@lk_func{function_index}_add_str_{}", *ssa_index);
                    *ssa_index += 1;
                    regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                        symbol,
                        len: value.chars().count(),
                        key_kind: native_runtime_string_key_kind(&value),
                        value,
                    });
                    pc = next_pc;
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
                    let name = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                    *ssa_index += 1;
                    if matches!(instr.opcode(), Opcode::MinInt | Opcode::MaxInt) {
                        let pred = if instr.opcode() == Opcode::MinInt { "slt" } else { "sgt" };
                        let cond = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                        *ssa_index += 1;
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
                    let name = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                    *ssa_index += 1;
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
                    let name = format!("%f{function_index}_r{}_{}", instr.a(), *ssa_index);
                    *ssa_index += 1;
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
                let cmp = format!("%f{function_index}_cmp{}_{}", instr.a(), *ssa_index);
                *ssa_index += 1;
                let out = format!("%f{function_index}_bool{}_{}", instr.a(), *ssa_index);
                *ssa_index += 1;
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
                            let cmp = format!("%f{function_index}_not{}_{}", instr.a(), *ssa_index);
                            *ssa_index += 1;
                            let out = format!("%f{function_index}_bool{}_{}", instr.a(), *ssa_index);
                            *ssa_index += 1;
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
                let symbol = format!("@lk_func{function_index}_to_iter_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_to_iter(value, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::GetIndex | Opcode::GetList => {
                let Some(target) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) =
                    native_known_string_key(function, pc, format!("@lk_func{function_index}_known_key_{pc}"))
                        .or_else(|| regs.get(instr.c() as usize).and_then(Clone::clone))
                else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_func{function_index}_index_str_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_index(target, key, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_field_k_str_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_index(target, key, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::SetIndex => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some(key) =
                    native_known_string_key(function, pc, format!("@lk_func{function_index}_known_set_key_{pc}"))
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
                native_replace_static_aliases(&mut regs, globals, instr.a() as usize, &value);
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
                native_replace_static_aliases(&mut regs, globals, instr.a() as usize, &value);
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
                native_replace_static_aliases(&mut regs, globals, instr.a() as usize, &value);
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
                regs[instr.a() as usize] = native_static_contains(needle, haystack);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_slice_str_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_slice_from(target, start, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_map_rest_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_map_rest(target, &keys, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_new_list_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_list_from_values(&values, symbol)
                    .or(Some(NativeStraightlineValue::ArgList { elements: values }));
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_new_map_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_map_from_pairs(&pairs, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_new_range_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_int_range(start, end, step, instr.c() != 0, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
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
                let symbol = format!("@lk_func{function_index}_new_object_{}", *ssa_index);
                *ssa_index += 1;
                regs[instr.a() as usize] = native_static_object_from_fields(&values, symbol);
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::ToString => {
                let Some(value) = regs.get(instr.b() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                if instr.a() as usize >= regs.len() {
                    return Ok(None);
                }
                let symbol = format!("@lk_func{function_index}_tostring_str_{}", *ssa_index);
                *ssa_index += 1;
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
                let symbol = format!("@lk_func{function_index}_concat_str_{}", *ssa_index);
                *ssa_index += 1;
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
                let symbol = format!("@lk_func{function_index}_concat_str_{}", *ssa_index);
                *ssa_index += 1;
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
                let symbol = format!("@lk_func{function_index}_split_list_{}", *ssa_index);
                *ssa_index += 1;
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
                let symbol = format!("@lk_func{function_index}_join_str_{}", *ssa_index);
                *ssa_index += 1;
                let Some(value) = native_static_list_join(target, separator, symbol) else {
                    return Ok(None);
                };
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
            Opcode::BrNil | Opcode::BrNotNil => {
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let is_nil = matches!(value, NativeStraightlineValue::Nil);
                let Some(taken) = native_relative_target(pc, instr.sbx() as i32, code.len()) else {
                    return Ok(None);
                };
                let branch =
                    (instr.opcode() == Opcode::BrNil && is_nil) || (instr.opcode() == Opcode::BrNotNil && !is_nil);
                next_pc = if branch { taken } else { pc + 1 };
            }
            Opcode::BrEqZeroInt
            | Opcode::BrNeZeroInt
            | Opcode::BrEqIntI4
            | Opcode::BrNeIntI4
            | Opcode::BrModEqZeroIntI4
            | Opcode::BrModNeZeroIntI4 => {
                let Some(lhs) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let (compare_opcode, rhs, offset, divisor) = match instr.opcode() {
                    Opcode::BrEqZeroInt => (Opcode::CmpInt, 0, instr.sbx() as i32, None),
                    Opcode::BrNeZeroInt => (Opcode::CmpNeInt, 0, instr.sbx() as i32, None),
                    Opcode::BrEqIntI4 => (
                        Opcode::CmpInt,
                        instr.branch_i4_immediate(),
                        instr.branch_i4_offset() as i32,
                        None,
                    ),
                    Opcode::BrNeIntI4 => (
                        Opcode::CmpNeInt,
                        instr.branch_i4_immediate(),
                        instr.branch_i4_offset() as i32,
                        None,
                    ),
                    Opcode::BrModEqZeroIntI4 => (
                        Opcode::CmpInt,
                        0,
                        instr.branch_i4_offset() as i32,
                        Some(instr.branch_i4_immediate()),
                    ),
                    Opcode::BrModNeZeroIntI4 => (
                        Opcode::CmpNeInt,
                        0,
                        instr.branch_i4_offset() as i32,
                        Some(instr.branch_i4_immediate()),
                    ),
                    _ => return Ok(None),
                };
                let lhs = if let Some(divisor) = divisor {
                    let NativeStraightlineValue::I64(raw) = lhs else {
                        return Ok(None);
                    };
                    let Ok(value) = raw.parse::<i64>() else {
                        return Ok(None);
                    };
                    if divisor == 0 {
                        return Ok(None);
                    }
                    NativeStraightlineValue::I64((value % i64::from(divisor)).to_string())
                } else {
                    lhs
                };
                let rhs = NativeStraightlineValue::I64(rhs.to_string());
                let Some(branch) = native_static_compare_bool(&lhs, &rhs, compare_opcode) else {
                    return Ok(None);
                };
                let Some(taken) = native_relative_target(pc, offset, code.len()) else {
                    return Ok(None);
                };
                next_pc = if branch { taken } else { pc + 1 };
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
                *ssa_index += 1;
                next_pc = catch_pc;
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
                    if builtin == super::straightline_value::NativeBuiltin::CoreCallMethod
                        && let Some(value) =
                            native_straightline_core_call_method(artifact, &args, globals, depth, body, ssa_index)?
                    {
                        regs[instr.a() as usize] = Some(value);
                        pc = next_pc;
                        continue;
                    }
                    regs[instr.a() as usize] = emit_native_builtin_call(body, builtin, &args, ssa_index);
                    if regs[instr.a() as usize].is_none() {
                        return Ok(None);
                    }
                    pc = next_pc;
                    continue;
                }
                let Some((target, captures)) = native_straightline_call_target(target) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = native_straightline_function_return(
                    artifact,
                    target as usize,
                    &args,
                    &captures,
                    globals,
                    depth + 1,
                    body,
                    ssa_index,
                )?;
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::CallDirect => {
                let Some(args) = native_straightline_call_args(&regs, instr.a(), instr.c()) else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = native_straightline_function_return(
                    artifact,
                    instr.b() as usize,
                    &args,
                    &[],
                    globals,
                    depth + 1,
                    body,
                    ssa_index,
                )?;
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::CallNamed => {
                let Some(target) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                let Some((target, captures)) = native_straightline_call_target(target) else {
                    return Ok(None);
                };
                let Some(function) = artifact.module.functions.get(target as usize) else {
                    return Ok(None);
                };
                let positional_count = instr.bx() & 0x7f;
                let named_count = instr.bx() >> 7;
                let Some(args) =
                    native_straightline_named_call_args(function, &regs, instr.a(), positional_count, named_count)
                else {
                    return Ok(None);
                };
                regs[instr.a() as usize] = native_straightline_function_return(
                    artifact,
                    target as usize,
                    &args,
                    &captures,
                    globals,
                    depth + 1,
                    body,
                    ssa_index,
                )?;
                if regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            opcode if opcode.is_return() => {
                if instr.return_count() == 0 {
                    return Ok(Some(NativeStraightlineValue::Nil));
                }
                if instr.return_count() != 1 {
                    return Ok(None);
                }
                let Some(value) = regs.get(instr.a() as usize).and_then(Clone::clone) else {
                    return Ok(None);
                };
                return Ok(match value {
                    NativeStraightlineValue::Function(_) => None,
                    value => Some(value),
                });
            }
            Opcode::Nop => {}
            _ => return Ok(None),
        }
        pc = next_pc;
    }
    Ok(None)
}

pub(super) fn native_direct_call_static_return_value(
    functions: &[FunctionData],
    instr: Instr,
    caller_static_values: &[Option<NativeStraightlineValue>],
    caller_code: &[Instr],
    caller_int_consts: &[i64],
    call_pc: usize,
    captures: &[NativeStraightlineValue],
    depth: usize,
) -> Option<NativeStraightlineValue> {
    if depth >= 8 {
        return None;
    }
    let function_index = instr.b() as usize;
    let function = functions.get(function_index)?;
    if function.capture_count as usize != captures.len() || function.param_count != instr.c() as u16 {
        return None;
    }
    let code = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if code.iter().any(|instr| {
        matches!(
            instr.opcode(),
            Opcode::Test
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
                | Opcode::CallDirect
                | Opcode::CallNamed
        )
    }) {
        return None;
    }
    let mut regs = vec![None; function.register_count as usize];
    for arg in 0..instr.c() as usize {
        let caller_reg = instr.a() as usize + 1 + arg;
        let value =
            caller_static_values.get(caller_reg).cloned().flatten().or_else(|| {
                native_local_static_i64_before(caller_code, caller_int_consts, call_pc, caller_reg as u8)
            })?;
        *regs.get_mut(arg)? = Some(value);
    }
    for pc in 0..code.len() {
        let instr = code[pc];
        match instr.opcode() {
            Opcode::LoadInt => {
                let value = function.consts.ints.get(instr.bx() as usize)?;
                *regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::I64(value.to_string()));
            }
            Opcode::LoadHeapConst => {
                let value = function.consts.heap_values.get(instr.bx() as usize)?;
                *regs.get_mut(instr.a() as usize)? =
                    native_straightline_heap_const_value(function_index, instr.bx(), value);
                regs.get(instr.a() as usize)?.as_ref()?;
            }
            Opcode::LoadFunction => {
                *regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::Function(instr.bx()));
            }
            Opcode::LoadCapture => {
                *regs.get_mut(instr.a() as usize)? = Some(captures.get(instr.bx() as usize)?.clone());
            }
            Opcode::LoadCellVal => {
                let cell = regs.get(instr.b() as usize)?.clone()?;
                *regs.get_mut(instr.a() as usize)? = Some(native_static_load_cell(cell)?);
            }
            Opcode::StoreCellVal => {
                let cell = regs.get(instr.a() as usize)?.clone()?;
                let value = regs.get(instr.b() as usize)?.clone()?;
                let value = native_static_store_cell(cell, value)?;
                let mut globals = Vec::new();
                native_replace_static_aliases(&mut regs, &mut globals, instr.a() as usize, &value);
                *regs.get_mut(instr.a() as usize)? = Some(value);
            }
            Opcode::MakeClosure => {
                let callee = functions.get(instr.b() as usize)?;
                let start = instr.c() as usize;
                let end = start.checked_add(callee.capture_count as usize)?;
                let captures = regs.get(start..end)?.iter().cloned().collect::<Option<Vec<_>>>()?;
                *regs.get_mut(instr.a() as usize)? = Some(NativeStraightlineValue::Closure {
                    function_index: instr.b() as u16,
                    captures,
                });
            }
            Opcode::Move => {
                *regs.get_mut(instr.a() as usize)? = regs.get(instr.b() as usize)?.clone();
            }
            opcode if opcode.is_return() && instr.return_count() == 0 => return Some(NativeStraightlineValue::Nil),
            opcode if opcode.is_return() && instr.return_count() == 1 => return regs.get(instr.a() as usize)?.clone(),
            opcode if opcode.is_return() => return None,
            Opcode::Nop => {}
            _ => return None,
        }
    }
    None
}

fn native_local_static_i64_before(
    code: &[Instr],
    int_consts: &[i64],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev in code.get(..pc)?.iter().copied().rev() {
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadInt => int_consts
                .get(prev.bx() as usize)
                .map(|value| NativeStraightlineValue::I64(value.to_string())),
            Opcode::Move => native_local_static_i64_before(code, int_consts, pc, prev.b()),
            _ => None,
        };
    }
    None
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

fn native_straightline_core_call_method(
    artifact: &ModuleArtifact,
    args: &[NativeStraightlineValue],
    globals: &mut [Option<NativeStraightlineValue>],
    depth: usize,
    body: &mut String,
    ssa_index: &mut usize,
) -> Result<Option<NativeStraightlineValue>> {
    let [
        NativeStraightlineValue::List { elements, .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::ArgList { elements: method_args },
    ] = args
    else {
        return Ok(None);
    };
    match method.as_str() {
        "filter" => {
            let [callable] = method_args.as_slice() else {
                return Ok(None);
            };
            let Some((function_index, captures)) = native_straightline_call_target(callable.clone()) else {
                return Ok(None);
            };
            let mut filtered = Vec::new();
            for value in elements {
                let Some(item) = native_straightline_const_value(value) else {
                    return Ok(None);
                };
                let Some(result) = native_straightline_function_return(
                    artifact,
                    function_index as usize,
                    std::slice::from_ref(&item),
                    &captures,
                    globals,
                    depth + 1,
                    body,
                    ssa_index,
                )?
                else {
                    return Ok(None);
                };
                let Some(truthy) = native_static_truthy(&result) else {
                    return Ok(None);
                };
                if truthy {
                    filtered.push(item);
                }
            }
            Ok(native_static_list_from_values(
                &filtered,
                format!("@lk_call_method_filter_{}", *ssa_index),
            ))
        }
        "map" => {
            let [callable] = method_args.as_slice() else {
                return Ok(None);
            };
            let Some((function_index, captures)) = native_straightline_call_target(callable.clone()) else {
                return Ok(None);
            };
            let mut mapped = Vec::new();
            for value in elements {
                let Some(item) = native_straightline_const_value(value) else {
                    return Ok(None);
                };
                let Some(result) = native_straightline_function_return(
                    artifact,
                    function_index as usize,
                    &[item],
                    &captures,
                    globals,
                    depth + 1,
                    body,
                    ssa_index,
                )?
                else {
                    return Ok(None);
                };
                mapped.push(result);
            }
            Ok(native_static_list_from_values(
                &mapped,
                format!("@lk_call_method_map_{}", *ssa_index),
            ))
        }
        "reduce" => {
            let [initial, callable] = method_args.as_slice() else {
                return Ok(None);
            };
            let Some((function_index, captures)) = native_straightline_call_target(callable.clone()) else {
                return Ok(None);
            };
            let mut acc = initial.clone();
            for value in elements {
                let Some(item) = native_straightline_const_value(value) else {
                    return Ok(None);
                };
                let Some(result) = native_straightline_function_return(
                    artifact,
                    function_index as usize,
                    &[acc, item],
                    &captures,
                    globals,
                    depth + 1,
                    body,
                    ssa_index,
                )?
                else {
                    return Ok(None);
                };
                acc = result;
            }
            Ok(Some(acc))
        }
        _ => Ok(None),
    }
}

fn native_straightline_const_value(value: &ConstRuntimeValueData) -> Option<NativeStraightlineValue> {
    match value {
        ConstRuntimeValueData::Nil => Some(NativeStraightlineValue::Nil),
        ConstRuntimeValueData::Bool(value) => Some(NativeStraightlineValue::Bool(i64::from(*value).to_string())),
        ConstRuntimeValueData::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
        ConstRuntimeValueData::Float(value) => Some(NativeStraightlineValue::F64(llvm_float_literal(*value))),
        ConstRuntimeValueData::ShortStr(value) => Some(NativeStraightlineValue::String {
            symbol: String::new(),
            len: value.chars().count(),
            key_kind: NativeStringKeyKind::Short,
            value: native_string_const_value(value)?,
        }),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => Some(NativeStraightlineValue::String {
                symbol: String::new(),
                len: value.chars().count(),
                key_kind: NativeStringKeyKind::Heap,
                value: native_string_const_value(value)?,
            }),
            ConstHeapValueData::List(elements) => Some(NativeStraightlineValue::List {
                symbol: String::new(),
                value: String::new(),
                elements: elements.clone(),
            }),
            ConstHeapValueData::Map(entries) => Some(NativeStraightlineValue::Map {
                symbol: String::new(),
                value: String::new(),
                entries: entries.clone(),
            }),
            ConstHeapValueData::UpvalCell(_) => None,
        },
    }
}

pub(super) fn native_straightline_named_call_args(
    function: &FunctionData,
    regs: &[Option<NativeStraightlineValue>],
    callee: u8,
    positional_count: u16,
    named_count: u16,
) -> Option<Vec<NativeStraightlineValue>> {
    let total_count = function.param_count as usize;
    let positional_count = positional_count as usize;
    let named_count = named_count as usize;
    if function.param_names.len() != total_count
        || function.positional_param_count as usize != positional_count
        || positional_count.checked_add(named_count)? != total_count
    {
        return None;
    }

    let positional_start = callee as usize + 1;
    let positional_end = positional_start.checked_add(positional_count)?;
    let named_start = positional_end;
    let named_width = named_count.checked_mul(2)?;
    let named_end = named_start.checked_add(named_width)?;
    if named_end > regs.len() {
        return None;
    }

    let mut args = vec![None; total_count];
    for (slot, value) in args[..positional_count]
        .iter_mut()
        .zip(regs[positional_start..positional_end].iter())
    {
        *slot = value.clone();
    }

    let mut seen = vec![false; named_count];
    for pair_start in (named_start..named_end).step_by(2) {
        let Some(NativeStraightlineValue::String { value: name, .. }) = regs[pair_start].clone() else {
            return None;
        };
        let offset = function.param_names[positional_count..]
            .iter()
            .position(|param| param == &name)?;
        if std::mem::replace(&mut seen[offset], true) {
            return None;
        }
        args[positional_count + offset] = regs[pair_start + 1].clone();
    }

    if seen.iter().any(|seen| !seen) {
        return None;
    }
    args.into_iter().collect()
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
