mod formatting;
mod object_display;
mod scalars;
mod static_direct;
mod symbolic;

use super::{facts::NativeScalarFacts, facts::NativeScalarKind};
use crate::llvm::{
    callee_eval::native_straightline_function_return,
    const_display::llvm_string_constant,
    ir_text::{native_relative_target, next_tmp, reg_in_bounds},
    output::emit_native_static_core_call_method,
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeModule,
        NativeStraightlineValue, NativeTextPart, native_runtime_string_key_kind, native_straightline_heap_const_value,
    },
};
use crate::vm::{ConstHeapValueData, ConstRuntimeValueData, Instr, ModuleArtifact, Opcode, RuntimeMapKeyData};
pub(in crate::llvm) use formatting::emit_static_formatted_print;
use object_display::native_object_display_text;
pub(in crate::llvm) use scalars::{emit_static_scalar_value_store_if_needed, scalar_arg_value};
use static_direct::static_direct_call_args;
pub(in crate::llvm) use symbolic::kind_symbolic_value;
pub(in crate::llvm) fn control_flow_static_boundaries(code: &[Instr]) -> Vec<bool> {
    let mut boundaries = vec![false; code.len()];
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::Jmp | Opcode::Test | Opcode::BrFalse | Opcode::BrTrue | Opcode::BrNil | Opcode::BrNotNil
                if !instr.opcode().is_compare_test() =>
            {
                if instr.opcode() != Opcode::Jmp && pc + 1 < code.len() {
                    boundaries[pc + 1] = true;
                }
                mark_forward_boundary(code, pc, instr, &mut boundaries);
            }
            opcode if opcode.is_compare_test() => {
                if pc + 2 < code.len() {
                    boundaries[pc + 2] = true;
                }
                mark_forward_boundary(code, pc, instr, &mut boundaries);
            }
            _ => {}
        }
    }
    for (pc, count) in predecessor_counts(code).into_iter().enumerate().take(code.len()) {
        if count > 1 {
            boundaries[pc] = true;
        }
    }
    if !boundaries.is_empty() {
        boundaries[0] = false;
    }
    boundaries
}

fn mark_forward_boundary(code: &[Instr], pc: usize, instr: Instr, boundaries: &mut [bool]) {
    if let Some(target) = branch_target(code, pc, instr)
        && target < code.len()
        && target > pc
    {
        boundaries[target] = true;
    }
}

fn branch_target(code: &[Instr], pc: usize, instr: Instr) -> Option<usize> {
    match instr.opcode() {
        Opcode::Jmp => native_relative_target(pc, instr.sj_arg(), code.len()),
        Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code.len()),
        Opcode::BrFalse | Opcode::BrTrue | Opcode::BrNil | Opcode::BrNotNil => {
            native_relative_target(pc, instr.sbx() as i32, code.len())
        }
        opcode if opcode.is_compare_test() => {
            let jmp = code.get(pc + 1).copied()?;
            (jmp.opcode() == Opcode::Jmp).then(|| native_relative_target(pc + 1, jmp.sj_arg(), code.len()))?
        }
        _ => None,
    }
}

pub(in crate::llvm) fn clear_control_flow_static_values(values: &mut [Option<NativeStraightlineValue>]) {
    for value in values {
        if !matches!(
            value,
            Some(
                NativeStraightlineValue::List { .. }
                    | NativeStraightlineValue::Map { .. }
                    | NativeStraightlineValue::DisplayMap { .. }
                    | NativeStraightlineValue::Object { .. }
                    | NativeStraightlineValue::DynamicMap { .. }
                    | NativeStraightlineValue::DynamicMapIter { .. }
                    | NativeStraightlineValue::DynamicMapEntry { .. }
                    | NativeStraightlineValue::DynamicList { .. }
                    | NativeStraightlineValue::DynamicPairList { .. }
                    | NativeStraightlineValue::DynamicConstListElement { .. }
                    | NativeStraightlineValue::DynamicArgListElement { .. }
                    | NativeStraightlineValue::DynamicJoinedText { .. }
                    | NativeStraightlineValue::ArgList { .. }
                    | NativeStraightlineValue::Builtin(_)
                    | NativeStraightlineValue::Module(_)
                    | NativeStraightlineValue::Function(_)
                    | NativeStraightlineValue::Closure { .. }
            )
        ) {
            *value = None;
        }
    }
}
pub(in crate::llvm) fn local_register_kind_before(code: &[Instr], pc: usize, reg: u8) -> Option<NativeScalarKind> {
    let start = pc.saturating_sub(64);
    let mut nearest = None;
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        let kind = match prev.opcode() {
            Opcode::LoadInt
            | Opcode::AddInt
            | Opcode::AddIntI
            | Opcode::MulIntI
            | Opcode::ModIntI
            | Opcode::SubInt
            | Opcode::MulInt
            | Opcode::DivInt
            | Opcode::ModInt
            | Opcode::Len
            | Opcode::ForLoopI => Some(NativeScalarKind::I64),
            Opcode::LoadFloat => Some(NativeScalarKind::F64),
            Opcode::LoadString | Opcode::ToString | Opcode::ConcatString | Opcode::StringSplit | Opcode::ListJoin => {
                Some(NativeScalarKind::StrPtr)
            }
            Opcode::LoadBool
            | Opcode::Not
            | Opcode::IsNil
            | Opcode::StringStartsWith
            | Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt => Some(NativeScalarKind::Bool),
            Opcode::LoadNil => Some(NativeScalarKind::Nil),
            Opcode::Move => match local_register_kind_before(code, prev_pc, prev.b()) {
                Some(kind) => Some(kind),
                None => return nearest,
            },
            _ => return nearest,
        };
        if kind == Some(NativeScalarKind::StrPtr) && matches!(nearest, Some(NativeScalarKind::Nil)) {
            return Some(NativeScalarKind::MaybeStrPtr);
        }
        if kind == Some(NativeScalarKind::Nil) && matches!(nearest, Some(NativeScalarKind::StrPtr)) {
            return Some(NativeScalarKind::MaybeStrPtr);
        }
        if kind == Some(NativeScalarKind::StrPtr) && nearest.is_none() {
            return kind;
        }
        if nearest.is_none() {
            nearest = kind;
        } else if !matches!(nearest, Some(NativeScalarKind::Nil)) {
            return nearest;
        }
    }
    nearest
}

pub(in crate::llvm) fn local_heap_kind_before(
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    reg: u8,
) -> Option<NativeScalarKind> {
    let start = pc.saturating_sub(64);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadHeapConst
                if matches!(
                    heap_values.get(prev.bx() as usize),
                    Some(ConstHeapValueData::LongString(_))
                ) =>
            {
                Some(NativeScalarKind::StrPtr)
            }
            _ => None,
        };
    }
    None
}
pub(in crate::llvm) fn local_compare_kind(
    kind: Option<NativeScalarKind>,
    heap_kind: Option<NativeScalarKind>,
    local_kind: Option<NativeScalarKind>,
) -> Option<NativeScalarKind> {
    if heap_kind == Some(NativeScalarKind::StrPtr) && !kind.is_some_and(NativeScalarKind::is_numeric) {
        heap_kind
    } else if local_kind == Some(NativeScalarKind::StrPtr) && !kind.is_some_and(NativeScalarKind::is_numeric) {
        local_kind
    } else if kind.is_some_and(|kind| kind != NativeScalarKind::StrPtr) {
        kind
    } else {
        kind.or(local_kind)
    }
}

pub(in crate::llvm) fn local_static_container_before(
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let start = pc.saturating_sub(128);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if !instr_writes_register(prev, reg) {
            continue;
        }
        let value = match prev.opcode() {
            Opcode::LoadHeapConst => match heap_values.get(prev.bx() as usize) {
                Some(ConstHeapValueData::List(values))
                    if values
                        .iter()
                        .all(|value| matches!(value, ConstRuntimeValueData::Int(_))) =>
                {
                    Some(NativeStraightlineValue::DynamicList {
                        id: prev_pc,
                        element: NativeListElementKind::I64,
                    })
                }
                Some(ConstHeapValueData::Map(values)) if values.is_empty() => {
                    Some(NativeStraightlineValue::DynamicMap {
                        id: prev_pc,
                        key: NativeMapKeyKind::Str,
                        value: NativeMapValueKind::I64,
                    })
                }
                Some(value) => native_straightline_heap_const_value(0, prev.bx(), value),
                None => None,
            },
            Opcode::SliceFrom => Some(NativeStraightlineValue::DynamicList {
                id: prev_pc,
                element: NativeListElementKind::I64,
            }),
            Opcode::ListPush => local_static_container_before(code, heap_values, prev_pc, prev.a()),
            Opcode::Move if prev.b() != reg => local_static_container_before(code, heap_values, prev_pc, prev.b()),
            _ => None,
        };
        return value;
    }
    None
}

pub(in crate::llvm) fn local_static_i64_before(
    code: &[Instr],
    int_consts: &[i64],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let start = pc.saturating_sub(32);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadInt => int_consts
                .get(prev.bx() as usize)
                .map(|value| NativeStraightlineValue::I64(value.to_string())),
            Opcode::Move if prev.b() != reg => local_static_i64_before(code, int_consts, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

pub(in crate::llvm) fn emit_text_string_equality_block(
    ir: &mut String,
    extra_globals: &mut String,
    parts: &[NativeTextPart],
    expected: &str,
    out_reg: u8,
    not_equal: bool,
    tmp_index: &mut usize,
) -> Option<()> {
    let mut remaining = expected;
    let mut checks = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if let NativeTextPart::StrPtr(ptr) = part {
            let (prefix, after_prefix) = split_expected_for_dynamic_part(remaining, &parts[index + 1..])?;
            let symbol = format!("@lk_text_cmp_{}", *tmp_index);
            *tmp_index += 1;
            extra_globals.push_str(&llvm_string_constant(&symbol, prefix));
            let cmp = next_tmp(tmp_index);
            let ok = next_tmp(tmp_index);
            ir.push_str(&format!("  {cmp} = call i32 @strcmp(ptr {ptr}, ptr {symbol})\n"));
            ir.push_str(&format!("  {ok} = icmp eq i32 {cmp}, 0\n"));
            checks.push(ok);
            remaining = after_prefix;
            continue;
        }
        if let NativeTextPart::I64(value) = part
            && value.starts_with('%')
        {
            let (expected, after_expected) = split_expected_for_dynamic_part(remaining, &parts[index + 1..])?;
            let expected = expected.parse::<i64>().ok()?;
            let ok = next_tmp(tmp_index);
            ir.push_str(&format!("  {ok} = icmp eq i64 {value}, {expected}\n"));
            checks.push(ok);
            remaining = after_expected;
            continue;
        }
        let literal = static_text_part_literal(part)?;
        remaining = remaining.strip_prefix(&literal)?;
    }
    if !remaining.is_empty() {
        return None;
    }
    let mut combined = if let Some(first) = checks.first() {
        first.clone()
    } else {
        ir.push_str(&format!(
            "  store i64 {}, ptr %r{out_reg}.slot\n",
            i64::from(!not_equal)
        ));
        return Some(());
    };
    for check in checks.iter().skip(1) {
        let next = next_tmp(tmp_index);
        ir.push_str(&format!("  {next} = and i1 {combined}, {check}\n"));
        combined = next;
    }
    if not_equal {
        let inverted = next_tmp(tmp_index);
        ir.push_str(&format!("  {inverted} = xor i1 {combined}, true\n"));
        combined = inverted;
    }
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {out} = zext i1 {combined} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{out_reg}.slot\n"));
    Some(())
}

fn split_expected_for_dynamic_part<'a>(remaining: &'a str, suffix: &[NativeTextPart]) -> Option<(&'a str, &'a str)> {
    let delimiter = suffix
        .iter()
        .filter_map(static_text_part_literal)
        .find(|s| !s.is_empty());
    let Some(delimiter) = delimiter else {
        return Some((remaining, ""));
    };
    let offset = remaining.find(&delimiter)?;
    Some(remaining.split_at(offset))
}

fn static_text_part_literal(part: &NativeTextPart) -> Option<String> {
    match part {
        NativeTextPart::I64(value) if !value.starts_with('%') => Some(value.clone()),
        NativeTextPart::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => Some(value.clone()),
        NativeTextPart::Bool(value) if value == "0" => Some("false".to_string()),
        NativeTextPart::Bool(value) if value == "1" => Some("true".to_string()),
        NativeTextPart::Nil => Some("nil".to_string()),
        NativeTextPart::String { value, .. } => Some(value.clone()),
        NativeTextPart::StrPtr(_) | NativeTextPart::Bool(_) | NativeTextPart::I64(_) | NativeTextPart::F64(_) => None,
    }
}

pub(in crate::llvm) fn mark_static_untaken_return_path(
    skip_pcs: &mut [bool],
    boundaries: &[bool],
    code: &[Instr],
    start: usize,
) {
    if predecessor_counts(code).get(start).copied().unwrap_or(0) > 1 {
        return;
    }
    if mark_static_untaken_merge_path(skip_pcs, code, start) {
        return;
    }
    let Some(path) = static_untaken_return_path(boundaries, code, start) else {
        return;
    };
    for pc in path {
        if let Some(skip) = skip_pcs.get_mut(pc) {
            *skip = true;
        }
    }
}

fn mark_static_untaken_merge_path(skip_pcs: &mut [bool], code: &[Instr], start: usize) -> bool {
    let predecessors = predecessor_counts(code);
    let mut pc = start;
    let mut marked_any = false;
    let mut seen = vec![false; code.len()];
    while pc < code.len() {
        if marked_any && predecessors.get(pc).copied().unwrap_or(0) > 1 {
            return true;
        }
        if seen[pc] {
            return marked_any;
        }
        seen[pc] = true;
        if let Some(skip) = skip_pcs.get_mut(pc) {
            *skip = true;
            marked_any = true;
        }
        let instr = code[pc];
        match instr.opcode() {
            Opcode::Return => return true,
            Opcode::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len()) else {
                    return marked_any;
                };
                pc = target;
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => return marked_any,
            opcode if opcode.is_compare_test() => return marked_any,
            _ => {
                let Some(next) = pc.checked_add(1) else {
                    return marked_any;
                };
                pc = next;
            }
        }
    }
    marked_any
}

fn predecessor_counts(code: &[Instr]) -> Vec<usize> {
    let mut predecessors = vec![0usize; code.len() + 1];
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::Jmp | Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                if instr.opcode() != Opcode::Jmp
                    && let Some(count) = predecessors.get_mut(pc + 1)
                {
                    *count += 1;
                }
                if let Some(target) = branch_target(code, pc, instr)
                    && let Some(count) = predecessors.get_mut(target)
                {
                    *count += 1;
                }
            }
            opcode if opcode.is_compare_test() => {
                if let Some(count) = predecessors.get_mut(pc + 2) {
                    *count += 1;
                }
                if let Some(target) = branch_target(code, pc, instr)
                    && let Some(count) = predecessors.get_mut(target)
                {
                    *count += 1;
                }
            }
            Opcode::Return => {}
            _ => {
                if let Some(count) = predecessors.get_mut(pc + 1) {
                    *count += 1;
                }
            }
        }
    }
    predecessors
}

fn static_untaken_return_path(boundaries: &[bool], code: &[Instr], start: usize) -> Option<Vec<usize>> {
    let instr = *code.get(start)?;
    if instr.opcode() == Opcode::Jmp {
        let mut path = vec![start];
        let target = native_relative_target(start, instr.sj_arg(), code.len())?;
        path.extend(static_untaken_linear_return_path(boundaries, code, target)?);
        return Some(path);
    }
    static_untaken_linear_return_path(boundaries, code, start)
}

fn static_untaken_linear_return_path(boundaries: &[bool], code: &[Instr], start: usize) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    let mut pc = start;
    let mut first = true;
    loop {
        let instr = *code.get(pc)?;
        if !first && boundaries.get(pc).copied().unwrap_or(false) {
            return None;
        }
        path.push(pc);
        match instr.opcode() {
            Opcode::Return => return Some(path),
            Opcode::Jmp | Opcode::Test | Opcode::BrFalse | Opcode::BrTrue | Opcode::ForLoopI => return None,
            opcode if opcode.is_compare_test() => return None,
            _ => {
                pc = pc.checked_add(1)?;
                first = false;
            }
        }
    }
}

pub(in crate::llvm) fn static_register_value_trusted_before(code: &[Instr], pc_limit: usize, reg: u8) -> bool {
    static_register_value_trusted_before_inner(code, pc_limit, reg, 0)
}
pub(in crate::llvm) fn static_string_value_trusted_at_call(code: &[Instr], call_pc: usize, reg: u8) -> bool {
    static_register_value_trusted_before(code, call_pc, reg)
}

pub(in crate::llvm) fn three_regs_in_bounds(register_count: usize, instr: Instr) -> bool {
    reg_in_bounds(register_count, instr.a())
        && reg_in_bounds(register_count, instr.b())
        && reg_in_bounds(register_count, instr.c())
}

pub(in crate::llvm) fn i64_slot_kind(kind: NativeScalarKind) -> bool {
    matches!(kind, NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
}

pub(in crate::llvm) fn static_call_args(
    static_regs: &[Option<NativeStraightlineValue>],
    callee: u8,
    count: u8,
) -> Option<Vec<NativeStraightlineValue>> {
    let start = callee as usize + 1;
    let end = start.checked_add(count as usize)?;
    static_regs
        .get(start..end)?
        .iter()
        .cloned()
        .map(|value| match value {
            Some(NativeStraightlineValue::Builtin(_)) => None,
            value => value,
        })
        .collect()
}

pub(in crate::llvm) fn static_call_target(
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

pub(in crate::llvm) fn native_static_closure(
    functions: &[crate::vm::FunctionData],
    function_index: u8,
    capture_start: u8,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    let function = functions.get(function_index as usize)?;
    let start = capture_start as usize;
    let end = start.checked_add(function.capture_count as usize)?;
    let captures = static_regs
        .get(start..end)?
        .iter()
        .cloned()
        .collect::<Option<Vec<_>>>()?;
    Some(NativeStraightlineValue::Closure {
        function_index: function_index as u16,
        captures,
    })
}

pub(in crate::llvm) fn static_callable_value(
    functions: &[crate::vm::FunctionData],
    instr: Instr,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    match instr.opcode() {
        Opcode::LoadFunction => Some(NativeStraightlineValue::Function(instr.bx())),
        Opcode::MakeClosure => native_static_closure(functions, instr.b(), instr.c(), static_regs),
        _ => None,
    }
}

pub(in crate::llvm) fn emit_inline_scalar_arg_stores(
    ir: &mut String,
    caller_facts: &NativeScalarFacts,
    call_pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) -> Option<()> {
    for arg in 0..instr.c() as usize {
        let caller_reg = instr.a().checked_add(1)?.checked_add(arg as u8)?;
        let Some(kind) = caller_facts.register_kind_before(call_pc, caller_reg) else {
            continue;
        };
        let value = next_tmp(tmp_index);
        let ty = kind.llvm_type();
        ir.push_str(&format!("  {value} = load {ty}, ptr %r{caller_reg}.slot\n"));
        ir.push_str(&format!("  store {ty} {value}, ptr %call{call_pc}.r{arg}.slot\n"));
    }
    Some(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn emit_static_named_call(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    facts: &NativeScalarFacts,
    pc: usize,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    tmp_index: &mut usize,
) -> Option<()> {
    let target = static_regs.get(instr.a() as usize).and_then(Clone::clone)?;
    let (function_index, captures) = static_call_target(target)?;
    let function = artifact.module.functions.get(function_index as usize)?;
    let args = scalar_named_call_args(
        function,
        ir,
        "",
        facts,
        pc,
        static_regs,
        instr.a(),
        instr.bx() & 0x7f,
        instr.bx() >> 7,
        tmp_index,
    )?;
    let value = native_straightline_function_return(
        artifact,
        function_index as usize,
        &args,
        &captures,
        static_globals,
        0,
        ir,
        tmp_index,
    )
    .ok()??;
    store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index)?;
    Some(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn emit_static_direct_call_result(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &ModuleArtifact,
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    tmp_index: &mut usize,
) -> Option<()> {
    if artifact
        .module
        .functions
        .get(instr.b() as usize)
        .is_some_and(callee_has_list_return_shape)
    {
        return None;
    }
    let (args, _) = static_direct_call_args(
        code,
        int_consts,
        strings,
        heap_values,
        pc,
        static_regs,
        instr.a(),
        instr.c(),
    )?;
    let value = native_straightline_function_return(
        artifact,
        instr.b() as usize,
        &args,
        &[],
        static_globals,
        0,
        ir,
        tmp_index,
    )
    .ok()
    .flatten()?;
    store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index)
}

fn callee_has_list_return_shape(function: &crate::vm::FunctionData) -> bool {
    function
        .code
        .iter()
        .copied()
        .filter_map(|raw| Instr::try_from_raw(raw).ok())
        .any(|instr| instr.opcode() == Opcode::ListPush)
}
#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn scalar_named_call_args(
    function: &crate::vm::FunctionData,
    ir: &mut String,
    slot_prefix: &str,
    facts: &NativeScalarFacts,
    pc: usize,
    static_regs: &[Option<NativeStraightlineValue>],
    callee: u8,
    positional_count: u16,
    named_count: u16,
    tmp_index: &mut usize,
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
    if named_end > static_regs.len() {
        return None;
    }

    let mut args = vec![None; total_count];
    for (offset, slot) in args[..positional_count].iter_mut().enumerate() {
        let reg = positional_start.checked_add(offset)?;
        *slot = Some(scalar_arg_value(
            ir,
            slot_prefix,
            facts,
            pc,
            static_regs,
            reg,
            tmp_index,
        )?);
    }

    let mut seen = vec![false; named_count];
    for pair_start in (named_start..named_end).step_by(2) {
        let Some(NativeStraightlineValue::String { value: name, .. }) = static_regs[pair_start].clone() else {
            return None;
        };
        let offset = function.param_names[positional_count..]
            .iter()
            .position(|param| param == &name)?;
        if std::mem::replace(&mut seen[offset], true) {
            return None;
        }
        args[positional_count + offset] = Some(scalar_arg_value(
            ir,
            slot_prefix,
            facts,
            pc,
            static_regs,
            pair_start + 1,
            tmp_index,
        )?);
    }

    if seen.iter().any(|seen| !seen) {
        return None;
    }
    args.into_iter().collect()
}

fn static_register_value_trusted_before_inner(code: &[Instr], pc_limit: usize, reg: u8, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    if register_written_by_enclosing_backedge_loop(code, pc_limit, reg) {
        return false;
    }
    let Some(last_write) = code
        .iter()
        .copied()
        .take(pc_limit)
        .enumerate()
        .rev()
        .find_map(|(pc, instr)| instr_writes_register(instr, reg).then_some(pc))
    else {
        return true;
    };
    let boundaries = control_flow_static_boundaries(code);
    let crosses_boundary = boundaries
        .iter()
        .copied()
        .skip(last_write)
        .take(pc_limit.saturating_sub(last_write) + 1)
        .any(|boundary| boundary);
    if crosses_boundary || branch_enters_after_write(code, last_write, pc_limit) {
        return false;
    }
    let instr = code[last_write];
    if instr.opcode() == Opcode::Move {
        return static_register_value_trusted_before_inner(code, last_write, instr.b(), depth + 1);
    }
    if code
        .iter()
        .copied()
        .skip(last_write + 1)
        .take(pc_limit.saturating_sub(last_write + 1))
        .any(|instr| {
            matches!(
                instr.opcode(),
                Opcode::Jmp | Opcode::Test | Opcode::BrFalse | Opcode::BrTrue | Opcode::ForLoopI | Opcode::Return
            ) || instr.opcode().is_compare_test()
        })
    {
        return false;
    }
    true
}

fn branch_enters_after_write(code: &[Instr], last_write: usize, pc_limit: usize) -> bool {
    code.iter().copied().take(last_write).enumerate().any(|(pc, instr)| {
        matches!(branch_target(code, pc, instr), Some(target) if target > last_write && target <= pc_limit)
    })
}

fn register_written_by_enclosing_backedge_loop(code: &[Instr], pc_limit: usize, reg: u8) -> bool {
    code.iter()
        .copied()
        .enumerate()
        .skip(pc_limit.saturating_add(1))
        .filter(|(_, instr)| instr.opcode() == Opcode::Jmp)
        .any(|(jump_pc, instr)| {
            let Some(target) = native_relative_target(jump_pc, instr.sj_arg(), code.len()) else {
                return false;
            };
            target <= pc_limit && (target..jump_pc).any(|pc| instr_writes_register(code[pc], reg))
        })
}

fn instr_writes_register(instr: Instr, reg: u8) -> bool {
    instr.a() == reg
        && !matches!(
            instr.opcode(),
            Opcode::Nop
                | Opcode::Jmp
                | Opcode::Test
                | Opcode::BrFalse
                | Opcode::BrTrue
                | Opcode::Return
                | Opcode::SetGlobal
                | Opcode::Raise
                | Opcode::TryBegin
                | Opcode::TryEnd
                | Opcode::Wide
        )
}

pub(in crate::llvm) fn store_native_scalar_call_result(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    dst: u8,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            static_regs[dst as usize] = (!value.starts_with('%')).then(|| NativeStraightlineValue::I64(value.clone()));
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeI64 { value, present } => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::MaybeI64 {
                value: value.clone(),
                present: present.clone(),
            });
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::F64(value) => {
            static_regs[dst as usize] = (!value.starts_with('%')).then(|| NativeStraightlineValue::F64(value.clone()));
            ir.push_str(&format!("  store double {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeF64 { value, present } => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::MaybeF64 {
                value: value.clone(),
                present: present.clone(),
            });
            ir.push_str(&format!("  store double {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::Bool(value) => {
            static_regs[dst as usize] = (!value.starts_with('%')).then(|| NativeStraightlineValue::Bool(value.clone()));
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeBool { value, present } => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::MaybeBool {
                value: value.clone(),
                present: present.clone(),
            });
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::Nil => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::Nil);
            ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            if symbol.is_empty() {
                let symbol = format!("@lk_call_str_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
                ir.push_str(&format!("  store ptr {symbol}, ptr %r{dst}.slot\n"));
            } else {
                if symbol.starts_with("@lk_func") || symbol.starts_with("@lk_static_") {
                    extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                }
                static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
                ir.push_str(&format!("  store ptr {symbol}, ptr %r{dst}.slot\n"));
            }
        }
        NativeStraightlineValue::StringPtr(value) => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::StringPtr(value.clone()));
            ir.push_str(&format!("  store ptr {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeStrPtr { value, present } => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::MaybeStrPtr {
                value: value.clone(),
                present: present.clone(),
            });
            ir.push_str(&format!("  store ptr {value}, ptr %r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        }
        | NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::Text,
            ..
        }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DisplayMap { .. }
        | NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. }
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. } => {
            static_regs[dst as usize] = Some(value);
        }
        _ => return None,
    }
    Some(())
}

pub(in crate::llvm) fn store_native_inline_scalar_value(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    call_pc: usize,
    dst: u8,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %call{call_pc}.r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeI64 { value, present } => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::F64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store double {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeF64 { value, present } => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store double {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::Bool(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeBool { value, present } => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::Nil => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 0, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            if symbol.is_empty() {
                let symbol = format!("@lk_call_inline_str_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
                ir.push_str(&format!("  store ptr {symbol}, ptr %call{call_pc}.r{dst}.slot\n"));
            } else {
                if symbol.starts_with("@lk_func") || symbol.starts_with("@lk_static_") {
                    extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                }
                static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
                ir.push_str(&format!("  store ptr {symbol}, ptr %call{call_pc}.r{dst}.slot\n"));
            }
        }
        NativeStraightlineValue::StringPtr(value) => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::StringPtr(value.clone()));
            ir.push_str(&format!("  store ptr {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeStrPtr { value, present } => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::MaybeStrPtr {
                value: value.clone(),
                present: present.clone(),
            });
            ir.push_str(&format!("  store ptr {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::Object { .. } => {
            static_regs[dst as usize] = Some(value);
        }
        _ => return None,
    }
    Some(())
}

pub(in crate::llvm) fn emit_inline_i64_binary_block(
    ir: &mut String,
    call_pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.c()));
    match instr.opcode() {
        Opcode::AddInt => ir.push_str(&format!("  {out} = add i64 {lhs}, {rhs}\n")),
        Opcode::SubInt => ir.push_str(&format!("  {out} = sub i64 {lhs}, {rhs}\n")),
        Opcode::MulInt => ir.push_str(&format!("  {out} = mul i64 {lhs}, {rhs}\n")),
        Opcode::DivInt => {
            let zero = next_tmp(tmp_index);
            let label = format!("call{call_pc}.div_ok_{}", out.trim_start_matches('%'));
            ir.push_str(&format!("  {zero} = icmp eq i64 {rhs}, 0\n"));
            ir.push_str(&format!("  br i1 {zero}, label %lk_divisor_zero, label %{label}\n"));
            ir.push_str(&format!("{label}:\n"));
            ir.push_str(&format!("  {out} = sdiv i64 {lhs}, {rhs}\n"));
        }
        Opcode::ModInt => {
            let zero = next_tmp(tmp_index);
            let label = format!("call{call_pc}.mod_ok_{}", out.trim_start_matches('%'));
            ir.push_str(&format!("  {zero} = icmp eq i64 {rhs}, 0\n"));
            ir.push_str(&format!("  br i1 {zero}, label %lk_divisor_zero, label %{label}\n"));
            ir.push_str(&format!("{label}:\n"));
            ir.push_str(&format!("  {out} = srem i64 {lhs}, {rhs}\n"));
        }
        _ => unreachable!("checked by caller"),
    }
    ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
}

pub(in crate::llvm) fn emit_mixed_numeric_int_opcode_block(
    ir: &mut String,
    slot_prefix: &str,
    instr: Instr,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) {
    let lhs = emit_numeric_load_as_f64(ir, slot_prefix, instr.b(), lhs_kind, tmp_index);
    let rhs = emit_numeric_load_as_f64(ir, slot_prefix, instr.c(), rhs_kind, tmp_index);
    let out = next_tmp(tmp_index);
    match instr.opcode() {
        Opcode::AddInt => ir.push_str(&format!("  {out} = fadd double {lhs}, {rhs}\n")),
        Opcode::SubInt => ir.push_str(&format!("  {out} = fsub double {lhs}, {rhs}\n")),
        Opcode::MulInt => ir.push_str(&format!("  {out} = fmul double {lhs}, {rhs}\n")),
        Opcode::DivInt => ir.push_str(&format!("  {out} = fdiv double {lhs}, {rhs}\n")),
        Opcode::ModInt => ir.push_str(&format!("  {out} = frem double {lhs}, {rhs}\n")),
        _ => unreachable!("checked by caller"),
    }
    ir.push_str(&format!(
        "  store double {out}, ptr %{}r{}.slot\n",
        slot_prefix,
        instr.a()
    ));
}

pub(in crate::llvm) fn emit_dynamic_string_starts_with(
    ir: &mut String,
    extra_globals: &mut String,
    slot_prefix: &str,
    dst: u8,
    target: u8,
    prefix: &str,
    tmp_index: &mut usize,
) {
    let symbol = format!("@lk_starts_with_prefix_{}", *tmp_index);
    *tmp_index += 1;
    let target_ptr = next_tmp(tmp_index);
    let cmp_value = next_tmp(tmp_index);
    let is_match = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    extra_globals.push_str(&llvm_string_constant(&symbol, prefix));
    ir.push_str(&format!(
        "  {target_ptr} = load ptr, ptr %{slot_prefix}r{target}.slot\n"
    ));
    ir.push_str(&format!(
        "  {cmp_value} = call i32 @strncmp(ptr {target_ptr}, ptr {symbol}, i64 {})\n",
        prefix.len()
    ));
    ir.push_str(&format!("  {is_match} = icmp eq i32 {cmp_value}, 0\n"));
    ir.push_str(&format!("  {out} = zext i1 {is_match} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %{slot_prefix}r{dst}.slot\n"));
}

pub(in crate::llvm) fn emit_static_string_i64_map_get(
    ir: &mut String,
    extra_globals: &mut String,
    entries: &[(RuntimeMapKeyData, ConstRuntimeValueData)],
    slot_prefix: &str,
    dst: u8,
    key: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key_ptr = next_tmp(tmp_index);
    ir.push_str(&format!("  {key_ptr} = load ptr, ptr %{slot_prefix}r{key}.slot\n"));
    let mut selected_value = "0".to_string();
    let mut selected_present = "0".to_string();
    for (map_key, value) in entries {
        let key_text = runtime_map_key_text(map_key)?;
        let value = const_runtime_i64_value(value)?;
        let symbol = format!("@lk_static_map_key_{}", *tmp_index);
        *tmp_index += 1;
        let cmp_value = next_tmp(tmp_index);
        let is_match = next_tmp(tmp_index);
        let next_value = next_tmp(tmp_index);
        let next_present = next_tmp(tmp_index);
        extra_globals.push_str(&llvm_string_constant(&symbol, key_text));
        ir.push_str(&format!(
            "  {cmp_value} = call i32 @strcmp(ptr {key_ptr}, ptr {symbol})\n"
        ));
        ir.push_str(&format!("  {is_match} = icmp eq i32 {cmp_value}, 0\n"));
        ir.push_str(&format!(
            "  {next_value} = select i1 {is_match}, i64 {value}, i64 {selected_value}\n"
        ));
        ir.push_str(&format!(
            "  {next_present} = select i1 {is_match}, i64 1, i64 {selected_present}\n"
        ));
        selected_value = next_value;
        selected_present = next_present;
    }
    ir.push_str(&format!(
        "  store i64 {selected_value}, ptr %{slot_prefix}r{dst}.slot\n"
    ));
    ir.push_str(&format!(
        "  store i64 {selected_present}, ptr %{slot_prefix}r{dst}.present.slot\n"
    ));
    Some(())
}

pub(in crate::llvm) fn static_string_i64_map_supported(entries: &[(RuntimeMapKeyData, ConstRuntimeValueData)]) -> bool {
    entries
        .iter()
        .all(|(key, value)| runtime_map_key_text(key).is_some() && const_runtime_i64_value(value).is_some())
}

fn runtime_map_key_text(key: &RuntimeMapKeyData) -> Option<&str> {
    match key {
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn const_runtime_i64_value(value: &ConstRuntimeValueData) -> Option<i64> {
    match value {
        ConstRuntimeValueData::Int(value) => Some(*value),
        ConstRuntimeValueData::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
}

fn emit_numeric_load_as_f64(
    ir: &mut String,
    slot_prefix: &str,
    reg: u8,
    kind: NativeScalarKind,
    tmp_index: &mut usize,
) -> String {
    let value = next_tmp(tmp_index);
    match kind {
        NativeScalarKind::F64 => {
            ir.push_str(&format!("  {value} = load double, ptr %{slot_prefix}r{reg}.slot\n"));
            value
        }
        NativeScalarKind::I64 => {
            let cast = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %{slot_prefix}r{reg}.slot\n"));
            ir.push_str(&format!("  {cast} = sitofp i64 {value} to double\n"));
            cast
        }
        NativeScalarKind::Bool
        | NativeScalarKind::Nil
        | NativeScalarKind::StrPtr
        | NativeScalarKind::MaybeI64
        | NativeScalarKind::MaybeStrPtr => {
            unreachable!("checked by caller")
        }
    }
}

pub(in crate::llvm) fn inline_text_value_from_reg(
    ir: &mut String,
    call_pc: usize,
    reg: u8,
    kind: Option<NativeScalarKind>,
    static_regs: &[Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = static_regs.get(reg as usize).and_then(Clone::clone) {
        return text_value_from_native(value);
    }
    let kind = kind?;
    let value = next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %call{call_pc}.r{reg}.slot\n"));
    let part = match kind {
        NativeScalarKind::I64 => NativeTextPart::I64(value),
        NativeScalarKind::F64 => NativeTextPart::F64(value),
        NativeScalarKind::Bool => NativeTextPart::Bool(value),
        NativeScalarKind::Nil => NativeTextPart::Nil,
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => NativeTextPart::StrPtr(value),
        NativeScalarKind::MaybeI64 => return None,
    };
    Some(NativeStraightlineValue::Text(vec![part]))
}

pub(in crate::llvm) fn text_value_from_reg(
    ir: &mut String,
    reg: u8,
    kind: Option<NativeScalarKind>,
    static_regs: &[Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = static_regs.get(reg as usize).and_then(Clone::clone) {
        return text_value_from_native(value);
    }
    let kind = kind?;
    let value = next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %r{reg}.slot\n"));
    let part = match kind {
        NativeScalarKind::I64 => NativeTextPart::I64(value),
        NativeScalarKind::F64 => NativeTextPart::F64(value),
        NativeScalarKind::Bool => NativeTextPart::Bool(value),
        NativeScalarKind::Nil => NativeTextPart::Nil,
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => NativeTextPart::StrPtr(value),
        NativeScalarKind::MaybeI64 => return None,
    };
    Some(NativeStraightlineValue::Text(vec![part]))
}

pub(in crate::llvm) fn text_value_from_native(value: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let parts = match value {
        NativeStraightlineValue::Text(parts) => parts,
        NativeStraightlineValue::I64(value) => vec![NativeTextPart::I64(value)],
        NativeStraightlineValue::F64(value) => vec![NativeTextPart::F64(value)],
        NativeStraightlineValue::Bool(value) => vec![NativeTextPart::Bool(value)],
        NativeStraightlineValue::Nil => vec![NativeTextPart::Nil],
        NativeStraightlineValue::StringPtr(value) => vec![NativeTextPart::StrPtr(value)],
        NativeStraightlineValue::String { symbol, value, .. } => vec![NativeTextPart::String { symbol, value }],
        NativeStraightlineValue::Object {
            symbol,
            value,
            type_name,
            fields,
        } => match native_object_display_text(&type_name, &fields) {
            Some(value) => vec![NativeTextPart::String {
                symbol: String::new(),
                value,
            }],
            None => vec![NativeTextPart::String { symbol, value }],
        },
        NativeStraightlineValue::DynamicTextChar => vec![NativeTextPart::String {
            symbol: String::new(),
            value: "x".to_string(),
        }],
        _ => return None,
    };
    Some(NativeStraightlineValue::Text(parts))
}

pub(in crate::llvm) fn concat_text_values(
    lhs: NativeStraightlineValue,
    rhs: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Text(mut lhs) = text_value_from_native(lhs)? else {
        return None;
    };
    let NativeStraightlineValue::Text(rhs) = text_value_from_native(rhs)? else {
        return None;
    };
    lhs.extend(rhs);
    Some(NativeStraightlineValue::Text(lhs))
}

pub(in crate::llvm) fn emit_native_block_core_call_method(
    ir: &mut String,
    extra_globals: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if builtin != NativeBuiltin::CoreCallMethod {
        return None;
    }
    if let Some(value) = emit_native_static_core_call_method(args, tmp_index) {
        return Some(value);
    }
    let [
        NativeStraightlineValue::Module(NativeModule::OsEnv),
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    else {
        return None;
    };
    if method != "get" || (elements.len() != 1 && elements.len() != 2) {
        return None;
    }
    let name = native_const_string_arg(&elements[0])?;
    let default = match elements.get(1) {
        Some(value) => native_const_string_arg(value)?,
        None => String::new(),
    };
    let name_symbol = format!("@lk_env_name_{}", *tmp_index);
    *tmp_index += 1;
    let default_symbol = format!("@lk_env_default_{}", *tmp_index);
    *tmp_index += 1;
    let env_ptr = next_tmp(tmp_index);
    let missing = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    extra_globals.push_str(&llvm_string_constant(&name_symbol, &name));
    extra_globals.push_str(&llvm_string_constant(&default_symbol, &default));
    ir.push_str(&format!("  {env_ptr} = call ptr @getenv(ptr {name_symbol})\n"));
    ir.push_str(&format!("  {missing} = icmp eq ptr {env_ptr}, null\n"));
    ir.push_str(&format!(
        "  {out} = select i1 {missing}, ptr {default_symbol}, ptr {env_ptr}\n"
    ));
    Some(NativeStraightlineValue::StringPtr(out))
}

fn native_const_string_arg(value: &ConstRuntimeValueData) -> Option<String> {
    match value {
        ConstRuntimeValueData::ShortStr(value) => Some(value.clone()),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub(in crate::llvm) fn native_static_string(value: &str, symbol: String) -> NativeStraightlineValue {
    NativeStraightlineValue::String {
        symbol,
        value: value.to_string(),
        len: value.chars().count(),
        key_kind: native_runtime_string_key_kind(value),
    }
}

pub(in crate::llvm) fn emit_inline_branch_to_next(ir: &mut String, call_pc: usize, pc: usize, code_len: usize) {
    ir.push_str(&format!(
        "  br label {}\n",
        inline_native_label(call_pc, pc + 1, code_len)
    ));
}

pub(in crate::llvm) fn inline_native_label(call_pc: usize, target: usize, code_len: usize) -> String {
    if target >= code_len {
        format!("%call{call_pc}.exit")
    } else {
        format!("%call{call_pc}.bb{target}")
    }
}

pub(in crate::llvm) fn emit_inline_scalar_equality_block(
    ir: &mut String,
    call_pc: usize,
    instr: Instr,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) -> Option<()> {
    if lhs_kind != rhs_kind
        && !matches!(
            (lhs_kind, rhs_kind),
            (NativeScalarKind::MaybeI64, NativeScalarKind::Nil) | (NativeScalarKind::Nil, NativeScalarKind::MaybeI64)
        )
    {
        return None;
    }
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let cmp = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    let pred = if instr.opcode() == Opcode::CmpInt { "eq" } else { "ne" };
    match (lhs_kind, rhs_kind) {
        (NativeScalarKind::MaybeI64, NativeScalarKind::Nil) | (NativeScalarKind::Nil, NativeScalarKind::MaybeI64) => {
            let maybe_reg = if lhs_kind == NativeScalarKind::MaybeI64 {
                instr.b()
            } else {
                instr.c()
            };
            let present = next_tmp(tmp_index);
            let nil_equal = if instr.opcode() == Opcode::CmpInt { "eq" } else { "ne" };
            ir.push_str(&format!(
                "  {present} = load i64, ptr %call{call_pc}.r{}.present.slot\n",
                maybe_reg
            ));
            ir.push_str(&format!("  {cmp} = icmp {nil_equal} i64 {present}, 0\n"));
        }
        (NativeScalarKind::I64, _) | (NativeScalarKind::Bool, _) | (NativeScalarKind::Nil, _) => {
            ir.push_str(&format!("  {lhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.b()));
            ir.push_str(&format!("  {rhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.c()));
            ir.push_str(&format!("  {cmp} = icmp {pred} i64 {lhs}, {rhs}\n"));
        }
        (NativeScalarKind::F64, _) => {
            ir.push_str(&format!(
                "  {lhs} = load double, ptr %call{call_pc}.r{}.slot\n",
                instr.b()
            ));
            ir.push_str(&format!(
                "  {rhs} = load double, ptr %call{call_pc}.r{}.slot\n",
                instr.c()
            ));
            ir.push_str(&format!("  {cmp} = fcmp o{pred} double {lhs}, {rhs}\n"));
        }
        (NativeScalarKind::StrPtr, _) | (NativeScalarKind::MaybeI64, _) | (NativeScalarKind::MaybeStrPtr, _) => {
            return None;
        }
    }
    ir.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
    Some(())
}

pub(in crate::llvm) fn emit_string_ptr_equality_block(ir: &mut String, instr: Instr, tmp_index: &mut usize) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let cmp_value = next_tmp(tmp_index);
    let is_equal = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs} = load ptr, ptr %r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load ptr, ptr %r{}.slot\n", instr.c()));
    ir.push_str(&format!("  {cmp_value} = call i32 @strcmp(ptr {lhs}, ptr {rhs})\n"));
    let pred = if instr.opcode() == Opcode::CmpInt { "eq" } else { "ne" };
    ir.push_str(&format!("  {is_equal} = icmp {pred} i32 {cmp_value}, 0\n"));
    ir.push_str(&format!("  {out} = zext i1 {is_equal} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
}

pub(in crate::llvm) fn emit_inline_string_ptr_equality_block(
    ir: &mut String,
    call_pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let cmp_value = next_tmp(tmp_index);
    let is_equal = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs} = load ptr, ptr %call{call_pc}.r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load ptr, ptr %call{call_pc}.r{}.slot\n", instr.c()));
    ir.push_str(&format!("  {cmp_value} = call i32 @strcmp(ptr {lhs}, ptr {rhs})\n"));
    let pred = if instr.opcode() == Opcode::CmpInt { "eq" } else { "ne" };
    ir.push_str(&format!("  {is_equal} = icmp {pred} i32 {cmp_value}, 0\n"));
    ir.push_str(&format!("  {out} = zext i1 {is_equal} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
}

pub(in crate::llvm) fn emit_inline_scalar_ordered_comparison_block(
    ir: &mut String,
    call_pc: usize,
    instr: Instr,
    tmp_index: &mut usize,
) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let cmp = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    let pred = match instr.opcode() {
        Opcode::CmpLtInt => "slt",
        Opcode::CmpLeInt => "sle",
        Opcode::CmpGtInt => "sgt",
        Opcode::CmpGeInt => "sge",
        _ => "slt",
    };
    ir.push_str(&format!("  {lhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.c()));
    ir.push_str(&format!("  {cmp} = icmp {pred} i64 {lhs}, {rhs}\n"));
    ir.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
}
