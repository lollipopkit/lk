use anyhow::Result;

use crate::vm::{ConstHeapValueData, FunctionData, Instr, ModuleArtifact, Opcode};

use crate::llvm::{
    dynamic_containers::{
        emit_dynamic_int_list_allocas, emit_dynamic_int_list_concat, emit_dynamic_int_list_copy,
        emit_dynamic_int_list_get, emit_dynamic_int_list_push, emit_dynamic_int_list_slice, emit_dynamic_int_list_take,
    },
    ir_text::{native_label, native_relative_target, next_tmp},
    scalar::block_helpers::local_register_kind_before,
    scalar::emit::{emit_i64_binary_block, emit_numeric_compare_block},
    scalar::facts::NativeScalarKind,
    straightline_value::{NativeBuiltin, NativeListElementKind, NativeStraightlineValue, native_static_global},
};

const I64_LIST_PARAM_BASE: usize = 700_000;
const I64_LIST_REG_BASE: usize = 600_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum I64ListParamKind {
    List,
    I64,
}

pub(in crate::llvm) fn compile_native_i64_list_subfunction(
    artifact: &ModuleArtifact,
    function_index: usize,
) -> Result<Option<String>> {
    let Some(function) = artifact.module.functions.get(function_index) else {
        return Ok(None);
    };
    if function.param_count == 0 || function.capture_count != 0 {
        return Ok(None);
    }
    let Ok(code) = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    else {
        return Ok(None);
    };
    if !i64_list_return_like(function, &code) {
        return Ok(None);
    }

    let profiles = i64_list_param_profiles(artifact, function_index, function.param_count as usize);
    let mut out = String::new();
    for profile in profiles {
        if let Some(ir) =
            compile_native_i64_list_subfunction_profile(artifact, function_index, function, &code, &profile)?
        {
            out.push_str(&ir);
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

fn compile_native_i64_list_subfunction_profile(
    artifact: &ModuleArtifact,
    function_index: usize,
    function: &FunctionData,
    code: &[Instr],
    param_profile: &[I64ListParamKind],
) -> Result<Option<String>> {
    let register_count = function.register_count as usize;
    let param_count = function.param_count as usize;
    if param_profile.len() != param_count || !param_profile.contains(&I64ListParamKind::List) {
        return Ok(None);
    }
    let code_len = code.len();
    let mut ir = String::new();
    let mut static_regs: Vec<Option<NativeStraightlineValue>> = vec![None; register_count];
    let fn_name = i64_list_function_name(function_index, param_profile);
    ir.push_str(&format!("define private void {fn_name}("));
    let mut has_param = false;
    for (param, kind) in param_profile.iter().copied().enumerate() {
        if param > 0 {
            ir.push_str(", ");
        }
        match kind {
            I64ListParamKind::List => ir.push_str(&format!("ptr %arg{param}.values, ptr %arg{param}.len.slot")),
            I64ListParamKind::I64 => ir.push_str(&format!("i64 %arg{param}")),
        }
        has_param = true;
    }
    if has_param {
        ir.push_str(", ");
    }
    ir.push_str("ptr %out.values, ptr %out.len.slot) {\n");
    ir.push_str("entry:\n");
    for reg in 0..register_count {
        ir.push_str(&format!("  %r{reg}.slot = alloca i64\n"));
        ir.push_str(&format!("  %r{reg}.present.slot = alloca i64\n"));
        ir.push_str(&format!("  store i64 1, ptr %r{reg}.present.slot\n"));
        emit_dynamic_int_list_allocas(&mut ir, &format!("list{}", i64_list_reg_id(function_index, reg)));
    }
    for (param, kind) in param_profile.iter().copied().enumerate() {
        if kind == I64ListParamKind::List {
            emit_dynamic_int_list_allocas(&mut ir, &format!("list{}", i64_list_param_id(function_index, param)));
        }
    }
    for (pc, instr) in code.iter().copied().enumerate() {
        if i64_list_alloca_needed(function, instr) || matches!(instr.opcode(), Opcode::Call) {
            emit_dynamic_int_list_allocas(&mut ir, &format!("list{pc}"));
        }
    }
    let mut tmp_index = 0usize;
    for (param, kind) in param_profile.iter().copied().enumerate() {
        match kind {
            I64ListParamKind::List => {
                let arg_len = next_tmp(&mut tmp_index);
                let param_id = i64_list_param_id(function_index, param);
                ir.push_str(&format!("  {arg_len} = load i64, ptr %arg{param}.len.slot\n"));
                ir.push_str(&format!(
                    "  call void @lk_slice_i64_list(ptr %arg{param}.values, i64 {arg_len}, i64 0, ptr %list{param_id}.value.slots, ptr %list{param_id}.len.slot)\n"
                ));
                static_regs[param] = Some(NativeStraightlineValue::DynamicList {
                    id: param_id,
                    element: NativeListElementKind::I64,
                });
            }
            I64ListParamKind::I64 => {
                ir.push_str(&format!("  store i64 %arg{param}, ptr %r{param}.slot\n"));
                static_regs[param] = None;
            }
        }
    }
    ir.push_str("  br label %bb0\n\n");

    let block_targets = find_block_targets(&code, code_len);
    let mut emitted_terminator = true;
    let mut after_return = false;
    for (pc, instr) in code.iter().copied().enumerate() {
        if pc == 0 {
            ir.push_str("bb0:\n");
            emitted_terminator = false;
        } else if block_targets.contains(&pc) {
            if !emitted_terminator {
                ir.push_str(&format!("  br label {}\n", native_label(pc, code_len)));
            }
            ir.push_str(&format!("{}:\n", native_label(pc, code_len).trim_start_matches('%')));
            after_return = false;
            emitted_terminator = false;
        }
        if after_return {
            continue;
        }
        emitted_terminator = false;
        match instr.opcode() {
            Opcode::Nop => {}
            Opcode::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code_len) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  br label {}\n", native_label(target, code_len)));
                emitted_terminator = true;
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                let value = next_tmp(&mut tmp_index);
                let cond = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
                ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                let Some((truthy_target, falsy_target)) = branch_truthy_falsy_targets(pc, instr, code_len) else {
                    return Ok(None);
                };
                ir.push_str(&format!(
                    "  br i1 {cond}, label {}, label {}\n",
                    native_label(truthy_target, code_len),
                    native_label(falsy_target, code_len)
                ));
                emitted_terminator = true;
            }
            opcode if opcode.is_compare_test() => {
                let lhs = next_tmp(&mut tmp_index);
                let rhs = next_tmp(&mut tmp_index);
                let cond = next_tmp(&mut tmp_index);
                let branch_cond = next_tmp(&mut tmp_index);
                let Some((taken, fallthrough)) = compare_test_targets(code, pc, code_len) else {
                    return Ok(None);
                };
                let Some(pred) = compare_test_i64_pred(instr.opcode()) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.a()));
                ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.b()));
                ir.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
                if instr.c() != 0 {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, false\n"));
                } else {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, true\n"));
                }
                ir.push_str(&format!(
                    "  br i1 {branch_cond}, label {}, label {}\n",
                    native_label(taken, code_len),
                    native_label(fallthrough, code_len)
                ));
                emitted_terminator = true;
            }
            Opcode::LoadInt => {
                let Some(value) = function.consts.ints.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(value.to_string()));
            }
            Opcode::LoadString => {
                let Some(value) = function.consts.strings.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                    symbol: String::new(),
                    value: value.clone(),
                    len: value.chars().count(),
                    key_kind: crate::llvm::straightline_value::NativeStringKeyKind::Short,
                });
            }
            Opcode::GetGlobal => {
                let Some(name) = artifact.module.globals.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = native_static_global(name);
                if static_regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::LoadHeapConst => {
                let Some(ConstHeapValueData::List(values)) = function.consts.heap_values.get(instr.bx() as usize)
                else {
                    return Ok(None);
                };
                if !values.is_empty() {
                    return Ok(None);
                }
                let list_id = i64_list_reg_id(function_index, instr.a() as usize);
                ir.push_str(&format!("  store i64 0, ptr %list{list_id}.len.slot\n"));
                ir.push_str(&format!("  store i64 0, ptr %list{list_id}.text.len.slot\n"));
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                    id: list_id,
                    element: NativeListElementKind::I64,
                });
            }
            Opcode::Move => {
                if emit_move(&mut ir, &mut static_regs, function_index, instr, &mut tmp_index)?.is_none() {
                    return Ok(None);
                }
            }
            Opcode::Len => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let len = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
                ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = None;
            }
            Opcode::AddInt | Opcode::SubInt | Opcode::MulInt | Opcode::DivInt | Opcode::ModInt => {
                emit_i64_binary_block(&mut ir, instr, &mut tmp_index);
                static_regs[instr.a() as usize] = None;
            }
            Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt => {
                emit_numeric_compare_block(
                    &mut ir,
                    instr,
                    NativeScalarKind::I64,
                    NativeScalarKind::I64,
                    &mut tmp_index,
                );
                static_regs[instr.a() as usize] = None;
            }
            Opcode::GetIndex | Opcode::GetList => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                emit_dynamic_int_list_get(&mut ir, id, instr.a(), instr.c(), &mut tmp_index);
                static_regs[instr.a() as usize] = None;
            }
            Opcode::ListPush => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.a() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                emit_dynamic_int_list_push(&mut ir, id, instr.b(), &mut tmp_index);
            }
            Opcode::NewList => {
                if !emit_new_list(&mut ir, &mut static_regs, function_index, instr, pc, &mut tmp_index)? {
                    return Ok(None);
                }
            }
            Opcode::Call => {
                let Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod)) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                if emit_i64_list_core_method(&mut ir, &mut static_regs, code, instr, pc, &mut tmp_index).is_none() {
                    return Ok(None);
                }
            }
            Opcode::Return => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.a() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let len = next_tmp(&mut tmp_index);
                let base = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
                ir.push_str(&format!(
                    "  {base} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 0\n"
                ));
                ir.push_str(&format!(
                    "  call void @lk_slice_i64_list(ptr {base}, i64 {len}, i64 0, ptr %out.values, ptr %out.len.slot)\n"
                ));
                ir.push_str("  ret void\n");
                after_return = true;
                emitted_terminator = true;
            }
            _ => return Ok(None),
        }
    }
    ir.push_str("exit:\n  ret void\n");
    ir.push_str("}\n");
    Ok(Some(ir))
}

fn emit_move(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    function_index: usize,
    instr: Instr,
    tmp_index: &mut usize,
) -> Result<Option<()>> {
    if let Some(NativeStraightlineValue::DynamicList { id: src_id, element }) =
        static_regs.get(instr.b() as usize).and_then(Clone::clone)
    {
        let dst_id = i64_list_reg_id(function_index, instr.a() as usize);
        if emit_dynamic_int_list_copy(ir, src_id, dst_id, tmp_index).is_none() {
            return Ok(None);
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList { id: dst_id, element });
    } else {
        let value = next_tmp(tmp_index);
        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
        static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
    }
    Ok(Some(()))
}

fn emit_new_list(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    function_index: usize,
    instr: Instr,
    _pc: usize,
    tmp_index: &mut usize,
) -> Result<bool> {
    let start = instr.b() as usize;
    let Some(end) = start.checked_add(instr.c() as usize) else {
        return Ok(false);
    };
    let Some(values) = static_regs.get(start..end) else {
        return Ok(false);
    };
    if values
        .iter()
        .any(|value| matches!(value, Some(NativeStraightlineValue::DynamicList { .. })))
    {
        static_regs[instr.a() as usize] = values
            .iter()
            .cloned()
            .collect::<Option<Vec<_>>>()
            .map(|elements| NativeStraightlineValue::ArgList { elements });
        return Ok(static_regs[instr.a() as usize].is_some());
    }
    let list_id = i64_list_reg_id(function_index, instr.a() as usize);
    ir.push_str(&format!("  store i64 {}, ptr %list{list_id}.len.slot\n", end - start));
    ir.push_str(&format!("  store i64 0, ptr %list{list_id}.text.len.slot\n"));
    for (index, reg) in (start..end).enumerate() {
        let slot = next_tmp(tmp_index);
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x i64], ptr %list{list_id}.value.slots, i64 0, i64 {index}\n"
        ));
        if let Some(NativeStraightlineValue::I64(value)) = static_regs.get(reg).and_then(Clone::clone) {
            ir.push_str(&format!("  store i64 {value}, ptr {slot}\n"));
        } else {
            let tmp = next_tmp(tmp_index);
            ir.push_str(&format!("  {tmp} = load i64, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 {tmp}, ptr {slot}\n"));
        }
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: list_id,
        element: NativeListElementKind::I64,
    });
    Ok(true)
}

fn emit_i64_list_core_method(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let start = instr.a() as usize + 1;
    let end = start.checked_add(instr.c() as usize)?;
    if end > static_regs.len() || instr.c() != 3 {
        return None;
    }
    let NativeStraightlineValue::DynamicList { id, .. } = static_regs.get(start)?.clone()? else {
        return None;
    };
    let NativeStraightlineValue::String { value: method, .. } = static_regs.get(start + 1)?.clone()? else {
        return None;
    };
    match method.as_str() {
        "take" => {
            let arg_list_reg = instr.a().checked_add(3)?;
            let count_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg)?;
            emit_dynamic_int_list_take(ir, id, pc, count_reg, tmp_index)?;
        }
        "skip" => {
            let arg_list_reg = instr.a().checked_add(3)?;
            let start_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg)?;
            emit_dynamic_int_list_slice(ir, id, pc, start_reg, tmp_index)?;
        }
        "concat" | "chain" => {
            let arg_list_reg = instr.a().checked_add(3)?;
            let rhs_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg)?;
            let NativeStraightlineValue::DynamicList { id: rhs_id, .. } =
                static_regs.get(rhs_reg as usize).cloned().flatten()?
            else {
                return None;
            };
            emit_dynamic_int_list_concat(ir, id, rhs_id, pc, tmp_index)?;
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::I64,
    });
    Some(())
}

fn i64_list_param_id(function_index: usize, param: usize) -> usize {
    I64_LIST_PARAM_BASE + function_index.saturating_mul(16) + param
}

fn i64_list_reg_id(function_index: usize, reg: usize) -> usize {
    I64_LIST_REG_BASE + function_index.saturating_mul(256) + reg
}

fn i64_list_alloca_needed(function: &FunctionData, instr: Instr) -> bool {
    matches!(instr.opcode(), Opcode::NewList)
        || matches!(instr.opcode(), Opcode::LoadHeapConst)
            && matches!(
                function.consts.heap_values.get(instr.bx() as usize),
                Some(ConstHeapValueData::List(values)) if values.is_empty()
            )
}

fn i64_list_return_like(function: &FunctionData, code: &[Instr]) -> bool {
    code.iter().any(|instr| instr.opcode() == Opcode::ListPush)
        || function
            .consts
            .strings
            .iter()
            .any(|value| matches!(value.as_str(), "take" | "skip" | "concat" | "chain" | "unique"))
}

fn i64_list_function_name(function_index: usize, profile: &[I64ListParamKind]) -> String {
    if profile.iter().all(|kind| *kind == I64ListParamKind::List) {
        format!("@lk_fn_{function_index}_i64_list")
    } else {
        let suffix = profile
            .iter()
            .map(|kind| match kind {
                I64ListParamKind::List => 'l',
                I64ListParamKind::I64 => 'i',
            })
            .collect::<String>();
        format!("@lk_fn_{function_index}_i64_list_{suffix}")
    }
}

fn i64_list_param_profiles(
    artifact: &ModuleArtifact,
    function_index: usize,
    param_count: usize,
) -> Vec<Vec<I64ListParamKind>> {
    let mut out = Vec::new();
    for function in &artifact.module.functions {
        let Ok(code) = function
            .code
            .iter()
            .copied()
            .map(Instr::try_from_raw)
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        for (pc, instr) in code.iter().copied().enumerate() {
            if instr.opcode() != Opcode::CallDirect
                || instr.b() as usize != function_index
                || instr.c() as usize != param_count
            {
                continue;
            }
            let start = instr.a() as usize + 1;
            let Some(profile) = (start..start + param_count)
                .map(|reg| callsite_i64_list_param_kind(function, &code, pc, u8::try_from(reg).ok()?))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if profile.contains(&I64ListParamKind::List) && !out.contains(&profile) {
                out.push(profile);
            }
        }
    }
    out
}

fn callsite_i64_list_param_kind(
    function: &FunctionData,
    code: &[Instr],
    pc: usize,
    reg: u8,
) -> Option<I64ListParamKind> {
    if local_register_kind_before(code, pc, reg) == Some(NativeScalarKind::I64) {
        return Some(I64ListParamKind::I64);
    }
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::Move if prev.b() != reg => callsite_i64_list_param_kind(function, code, prev_pc, prev.b()),
            Opcode::LoadHeapConst
                if matches!(
                    function.consts.heap_values.get(prev.bx() as usize),
                    Some(ConstHeapValueData::List(values))
                        if values.iter().all(|value| matches!(value, crate::vm::ConstRuntimeValueData::Int(_)))
                ) =>
            {
                Some(I64ListParamKind::List)
            }
            _ => None,
        };
    }
    None
}

fn single_arg_list_source_reg_before(code: &[Instr], pc: usize, reg: u8) -> Option<u8> {
    let start = pc.saturating_sub(16);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::Move if prev.b() != reg => single_arg_list_source_reg_before(code, prev_pc, prev.b()),
            Opcode::NewList if prev.c() == 1 => Some(prev.b()),
            _ => None,
        };
    }
    None
}

fn find_block_targets(code: &[Instr], code_len: usize) -> Vec<usize> {
    let mut targets = vec![0];
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::Jmp => {
                if let Some(target) = native_relative_target(pc, instr.sj_arg(), code_len) {
                    targets.push(target);
                }
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                if let Some((truthy, falsy)) = branch_truthy_falsy_targets(pc, instr, code_len) {
                    targets.push(truthy);
                    targets.push(falsy);
                }
            }
            opcode if opcode.is_compare_test() => {
                if let Some((taken, fallthrough)) = compare_test_targets(code, pc, code_len) {
                    targets.push(taken);
                    targets.push(fallthrough);
                }
            }
            _ => {}
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

fn branch_truthy_falsy_targets(pc: usize, instr: Instr, code_len: usize) -> Option<(usize, usize)> {
    let fallthrough = pc + 1;
    let relative = match instr.opcode() {
        Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code_len)?,
        Opcode::BrFalse | Opcode::BrTrue => native_relative_target(pc, instr.sbx() as i32, code_len)?,
        _ => return None,
    };
    let truthy = if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue {
        relative
    } else {
        fallthrough
    };
    let falsy = if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse {
        relative
    } else {
        fallthrough
    };
    Some((truthy, falsy))
}

fn compare_test_targets(code: &[Instr], pc: usize, code_len: usize) -> Option<(usize, usize)> {
    let jmp = code.get(pc + 1).copied()?;
    if jmp.opcode() != Opcode::Jmp {
        return None;
    }
    Some((native_relative_target(pc + 1, jmp.sj_arg(), code_len)?, pc + 2))
}

fn compare_test_i64_pred(opcode: Opcode) -> Option<&'static str> {
    Some(match opcode {
        Opcode::TestEqInt => "eq",
        Opcode::TestNeInt => "ne",
        Opcode::TestLtInt => "slt",
        Opcode::TestLeInt => "sle",
        Opcode::TestGtInt => "sgt",
        Opcode::TestGeInt => "sge",
        _ => return None,
    })
}
