use crate::llvm::scalar::kind::NativeScalarKind;
use crate::llvm::straightline_value::{NativeListElementKind, NativeStraightlineValue};
use crate::vm::{Function32Data, Instr32, Opcode32};

use super::{
    analysis::{native_return_kind_from_facts, static_value_kind},
    native_scalar_block_facts_with_initial,
};

pub(in crate::llvm) fn static_call_target(
    value: &NativeStraightlineValue,
) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match value {
        NativeStraightlineValue::Function(function_index) => Some((*function_index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((*function_index, captures.clone())),
        _ => None,
    }
}

pub(in crate::llvm) fn static_global(
    values: &[Option<NativeStraightlineValue>],
    slot: u16,
) -> Option<NativeStraightlineValue> {
    values.get(slot as usize).cloned().flatten()
}

fn native_arg_value(
    kinds: &[Option<NativeScalarKind>],
    values: &[Option<NativeStraightlineValue>],
    reg: usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = values.get(reg).cloned().flatten() {
        return Some(value);
    }
    let kind = kinds.get(reg).copied().flatten()?;
    let value = format!("%call_arg_r{reg}");
    match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => Some(NativeStraightlineValue::I64(value)),
        NativeScalarKind::F64 => Some(NativeStraightlineValue::F64(value)),
        NativeScalarKind::Bool => Some(NativeStraightlineValue::Bool(value)),
        NativeScalarKind::Nil => Some(NativeStraightlineValue::Nil),
        NativeScalarKind::StrPtr => Some(NativeStraightlineValue::StringPtr(value)),
        NativeScalarKind::MaybeStrPtr => Some(NativeStraightlineValue::MaybeStrPtr {
            value,
            present: format!("%call_arg_r{reg}_present"),
        }),
    }
}

pub(in crate::llvm) fn native_named_call_args(
    function: &Function32Data,
    kinds: &[Option<NativeScalarKind>],
    values: &[Option<NativeStraightlineValue>],
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
    if named_end > values.len() || named_end > kinds.len() {
        return None;
    }

    let mut args = vec![None; total_count];
    for (offset, slot) in args[..positional_count].iter_mut().enumerate() {
        *slot = Some(native_arg_value(kinds, values, positional_start + offset)?);
    }

    let mut seen = vec![false; named_count];
    for pair_start in (named_start..named_end).step_by(2) {
        let Some(NativeStraightlineValue::String { value: name, .. }) = values[pair_start].clone() else {
            return None;
        };
        let offset = function.param_names[positional_count..]
            .iter()
            .position(|param| param == &name)?;
        if std::mem::replace(&mut seen[offset], true) {
            return None;
        }
        args[positional_count + offset] = Some(native_arg_value(kinds, values, pair_start + 1)?);
    }

    if seen.iter().any(|seen| !seen) {
        return None;
    }
    args.into_iter().collect()
}

pub(in crate::llvm) fn native_static_function_return_kind(
    functions: &[Function32Data],
    function_index: usize,
    args: &[NativeStraightlineValue],
    captures: &[NativeStraightlineValue],
    global_kinds: &[Option<NativeScalarKind>],
    static_globals: &[Option<NativeStraightlineValue>],
    global_count: usize,
    global_names: &[String],
    depth: usize,
    recursive_hints: &[(u16, Option<NativeScalarKind>)],
) -> Option<NativeScalarKind> {
    if depth >= 8 {
        if let Some((_, hint)) = recursive_hints.iter().find(|(idx, _)| *idx as usize == function_index) {
            return *hint;
        }
        return None;
    }
    let callee = functions.get(function_index)?;
    if callee.capture_count as usize != captures.len() || callee.param_count as usize != args.len() {
        return None;
    }
    let code = callee
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut callee_kinds = vec![None; callee.register_count as usize];
    let mut callee_static_values = vec![None; callee.register_count as usize];
    for (arg, value) in args.iter().cloned().enumerate() {
        *callee_kinds.get_mut(arg)? = static_value_kind(&value);
        *callee_static_values.get_mut(arg)? = Some(value);
    }
    let facts = native_scalar_block_facts_with_initial(
        callee.register_count as usize,
        global_count,
        global_names,
        &callee.consts.ints,
        &callee.consts.strings,
        &callee.consts.heap_values,
        &code,
        callee_kinds,
        callee_static_values,
        global_kinds.to_vec(),
        static_globals.to_vec(),
        Some(functions),
        captures,
        depth + 1,
        recursive_hints,
    )?;
    native_return_kind_from_facts(&code, &facts)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn native_direct_call_return_kind(
    functions: &[Function32Data],
    instr: Instr32,
    caller_kinds: &[Option<NativeScalarKind>],
    caller_static_values: &[Option<NativeStraightlineValue>],
    global_kinds: &[Option<NativeScalarKind>],
    static_globals: &[Option<NativeStraightlineValue>],
    global_count: usize,
    global_names: &[String],
    captures: &[NativeStraightlineValue],
    depth: usize,
    recursive_hints: &[(u16, Option<NativeScalarKind>)],
) -> Option<NativeScalarKind> {
    if depth >= 8 {
        if let Some((_, hint)) = recursive_hints.iter().find(|(idx, _)| *idx as u8 == instr.b()) {
            return *hint;
        }
        return None;
    }
    let callee = functions.get(instr.b() as usize)?;
    if callee.capture_count as usize != captures.len() || instr.c() as u16 != callee.param_count {
        return None;
    }
    let code = callee
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut callee_kinds = vec![None; callee.register_count as usize];
    let mut callee_static_values = vec![None; callee.register_count as usize];
    let mut callee_recursive_hints = recursive_hints.to_vec();
    let callee_index = instr.b() as u16;
    if !callee_recursive_hints.iter().any(|(idx, _)| *idx == callee_index) {
        for callee_instr in callee.code.iter().copied() {
            if let Ok(ci) = Instr32::try_from_raw(callee_instr)
                && ci.opcode() == Opcode32::CallDirect
                && ci.b() as u16 == callee_index
            {
                callee_recursive_hints.push((callee_index, Some(NativeScalarKind::I64)));
                break;
            }
        }
    }
    for arg in 0..instr.c() as usize {
        let caller_reg = instr.a() as usize + 1 + arg;
        let static_value = caller_static_values.get(caller_reg).cloned().flatten();
        if let Some(kind) = caller_kinds.get(caller_reg).copied().flatten() {
            *callee_kinds.get_mut(arg)? = Some(kind);
        } else if static_value.is_none() {
            return None;
        }
        *callee_static_values.get_mut(arg)? = static_value;
    }
    let facts = native_scalar_block_facts_with_initial(
        callee.register_count as usize,
        global_count,
        global_names,
        &callee.consts.ints,
        &callee.consts.strings,
        &callee.consts.heap_values,
        &code,
        callee_kinds,
        callee_static_values,
        global_kinds.to_vec(),
        static_globals.to_vec(),
        Some(functions),
        captures,
        depth + 1,
        &callee_recursive_hints,
    )?;
    native_return_kind_from_facts(&code, &facts)
}

pub(in crate::llvm) fn peek_recursive_function_base_return_kind(
    functions: &[Function32Data],
    function_index: u16,
    global_count: usize,
    global_names: &[String],
    global_kinds: Vec<Option<NativeScalarKind>>,
) -> Option<NativeScalarKind> {
    let Some(callee) = functions.get(function_index as usize) else {
        return None;
    };
    let code: Vec<Instr32> = callee
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let static_globals: Vec<Option<NativeStraightlineValue>> = vec![None; global_count];
    let candidates = [NativeScalarKind::I64, NativeScalarKind::F64, NativeScalarKind::Bool];
    let param_profiles = recursive_param_profiles(callee.param_count as usize);
    for candidate in candidates {
        let hint: Vec<(u16, Option<NativeScalarKind>)> = vec![(function_index, Some(candidate))];
        for params in &param_profiles {
            if let Some(kind) = try_analyze_with_params(
                functions,
                callee,
                &code,
                global_count,
                global_names,
                &global_kinds,
                &static_globals,
                &hint,
                params,
            ) && kind == candidate
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn recursive_param_profiles(param_count: usize) -> Vec<Vec<(NativeScalarKind, NativeStraightlineValue)>> {
    fn candidate_values() -> Vec<(NativeScalarKind, NativeStraightlineValue)> {
        vec![
            (NativeScalarKind::I64, NativeStraightlineValue::I64("0".to_string())),
            (NativeScalarKind::F64, NativeStraightlineValue::F64("0.0".to_string())),
            (NativeScalarKind::Bool, NativeStraightlineValue::Bool("0".to_string())),
            (
                NativeScalarKind::I64,
                NativeStraightlineValue::DynamicList {
                    id: usize::MAX,
                    element: NativeListElementKind::I64,
                },
            ),
        ]
    }
    if param_count == 0 {
        return vec![Vec::new()];
    }

    let candidates = candidate_values();
    let mut profiles = Vec::new();
    let mut current = Vec::with_capacity(param_count);
    build_recursive_param_profiles(param_count, &candidates, &mut current, &mut profiles);
    profiles
}

fn build_recursive_param_profiles(
    remaining: usize,
    candidates: &[(NativeScalarKind, NativeStraightlineValue)],
    current: &mut Vec<(NativeScalarKind, NativeStraightlineValue)>,
    profiles: &mut Vec<Vec<(NativeScalarKind, NativeStraightlineValue)>>,
) {
    if remaining == 0 {
        profiles.push(current.clone());
        return;
    }
    for candidate in candidates {
        current.push(candidate.clone());
        build_recursive_param_profiles(remaining - 1, candidates, current, profiles);
        current.pop();
    }
}

fn try_analyze_with_params(
    functions: &[Function32Data],
    callee: &Function32Data,
    code: &[Instr32],
    global_count: usize,
    global_names: &[String],
    global_kinds: &[Option<NativeScalarKind>],
    static_globals: &[Option<NativeStraightlineValue>],
    hint: &[(u16, Option<NativeScalarKind>)],
    params: &[(NativeScalarKind, NativeStraightlineValue)],
) -> Option<NativeScalarKind> {
    if params.len() != callee.param_count as usize {
        return None;
    }
    let mut callee_kinds = vec![None; callee.register_count as usize];
    let mut callee_static_values = vec![None; callee.register_count as usize];
    for (arg, (param_kind, param_value)) in params.iter().cloned().enumerate() {
        *callee_kinds.get_mut(arg)? = Some(param_kind);
        *callee_static_values.get_mut(arg)? = Some(param_value);
    }
    let facts = native_scalar_block_facts_with_initial(
        callee.register_count as usize,
        global_count,
        global_names,
        &callee.consts.ints,
        &callee.consts.strings,
        &callee.consts.heap_values,
        code,
        callee_kinds,
        callee_static_values,
        global_kinds.to_vec(),
        static_globals.to_vec(),
        Some(functions),
        &[],
        0,
        hint,
    )?;
    native_return_kind_from_facts(code, &facts)
}
