use crate::vm::{
    ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Module32Artifact, Opcode32, RuntimeMapKeyData,
};

use super::{
    callee_eval::native_straightline_function_return,
    const_display::llvm_string_constant,
    ir_text::{native_relative_target, next_tmp, reg_in_bounds},
    scalar_facts::NativeScalarFacts,
    scalar_facts::NativeScalarKind,
    straightline_value::{NativeBuiltin, NativeModule, NativeStraightlineValue, NativeTextPart},
};

pub(super) fn control_flow_static_boundaries(code: &[Instr32]) -> Vec<bool> {
    let mut boundaries = vec![false; code.len()];
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode32::Test => {
                if pc + 1 < code.len() {
                    boundaries[pc + 1] = true;
                }
                if let Some(target) = native_relative_target(pc, instr.c() as i8 as i32, code.len())
                    && target < code.len()
                    && target > pc
                {
                    boundaries[target] = true;
                }
            }
            Opcode32::Jmp => {
                if let Some(target) = native_relative_target(pc, instr.sj_arg(), code.len())
                    && target < code.len()
                    && target > pc
                {
                    boundaries[target] = true;
                }
            }
            _ => {}
        }
    }
    if !boundaries.is_empty() {
        boundaries[0] = false;
    }
    boundaries
}

pub(super) fn clear_control_flow_static_values(values: &mut [Option<NativeStraightlineValue>]) {
    let _ = values;
}

pub(super) fn mark_static_untaken_return_path(
    skip_pcs: &mut [bool],
    boundaries: &[bool],
    code: &[Instr32],
    start: usize,
) {
    let Some(path) = static_untaken_return_path(boundaries, code, start) else {
        return;
    };
    for pc in path {
        if let Some(skip) = skip_pcs.get_mut(pc) {
            *skip = true;
        }
    }
}

fn static_untaken_return_path(boundaries: &[bool], code: &[Instr32], start: usize) -> Option<Vec<usize>> {
    let instr = *code.get(start)?;
    if instr.opcode() == Opcode32::Jmp {
        let mut path = vec![start];
        let target = native_relative_target(start, instr.sj_arg(), code.len())?;
        path.extend(static_untaken_linear_return_path(boundaries, code, target)?);
        return Some(path);
    }
    static_untaken_linear_return_path(boundaries, code, start)
}

fn static_untaken_linear_return_path(boundaries: &[bool], code: &[Instr32], start: usize) -> Option<Vec<usize>> {
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
            Opcode32::Return => return Some(path),
            Opcode32::Jmp | Opcode32::Test => return None,
            _ => {
                pc = pc.checked_add(1)?;
                first = false;
            }
        }
    }
}

pub(super) fn static_string_value_trusted_at_call(code: &[Instr32], call_pc: usize, reg: u8) -> bool {
    static_string_value_trusted_before(code, call_pc, reg, 0)
}

pub(super) fn three_regs_in_bounds(register_count: usize, instr: Instr32) -> bool {
    reg_in_bounds(register_count, instr.a())
        && reg_in_bounds(register_count, instr.b())
        && reg_in_bounds(register_count, instr.c())
}

pub(super) fn i64_slot_kind(kind: NativeScalarKind) -> bool {
    matches!(kind, NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
}

pub(super) fn static_call_args(
    static_regs: &[Option<NativeStraightlineValue>],
    callee: u8,
    count: u8,
) -> Option<Vec<NativeStraightlineValue>> {
    let start = callee as usize + 1;
    let end = start.checked_add(count as usize)?;
    static_regs.get(start..end)?.iter().cloned().collect()
}

pub(super) fn static_call_target(value: NativeStraightlineValue) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match value {
        NativeStraightlineValue::Function(function_index) => Some((function_index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((function_index, captures)),
        _ => None,
    }
}

pub(super) fn native_static_closure(
    functions: &[crate::vm::Function32Data],
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

pub(super) fn static_callable_value(
    functions: &[crate::vm::Function32Data],
    instr: Instr32,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    match instr.opcode() {
        Opcode32::LoadFunction => Some(NativeStraightlineValue::Function(instr.bx())),
        Opcode32::MakeClosure => native_static_closure(functions, instr.b(), instr.c(), static_regs),
        _ => None,
    }
}

pub(super) fn emit_inline_scalar_arg_stores(
    ir: &mut String,
    caller_facts: &NativeScalarFacts,
    call_pc: usize,
    instr: Instr32,
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
pub(super) fn emit_static_named_call(
    ir: &mut String,
    extra_globals: &mut String,
    artifact: &Module32Artifact,
    scalar_facts: &NativeScalarFacts,
    pc: usize,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    tmp_index: &mut usize,
) -> Option<()> {
    let target = static_regs.get(instr.a() as usize).and_then(Clone::clone)?;
    let (function_index, captures) = static_call_target(target)?;
    let function = artifact.module.functions.get(function_index as usize)?;
    let args = scalar_named_call_args(
        function,
        ir,
        "",
        scalar_facts,
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
pub(super) fn scalar_named_call_args(
    function: &crate::vm::Function32Data,
    ir: &mut String,
    slot_prefix: &str,
    scalar_facts: &NativeScalarFacts,
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
            scalar_facts,
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
            scalar_facts,
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

fn scalar_arg_value(
    ir: &mut String,
    slot_prefix: &str,
    scalar_facts: &NativeScalarFacts,
    pc: usize,
    static_regs: &[Option<NativeStraightlineValue>],
    reg: usize,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = static_regs.get(reg).cloned().flatten() {
        return Some(value);
    }
    let reg = u8::try_from(reg).ok()?;
    let kind = scalar_facts.register_kind_before(pc, reg)?;
    if kind == NativeScalarKind::Nil {
        return Some(NativeStraightlineValue::Nil);
    }
    let value = next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %{slot_prefix}r{reg}.slot\n"));
    match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => Some(NativeStraightlineValue::I64(value)),
        NativeScalarKind::F64 => Some(NativeStraightlineValue::F64(value)),
        NativeScalarKind::Bool => Some(NativeStraightlineValue::Bool(value)),
        NativeScalarKind::Nil => Some(NativeStraightlineValue::Nil),
        NativeScalarKind::StrPtr => Some(NativeStraightlineValue::StringPtr(value)),
    }
}

pub(super) fn emit_static_scalar_value_store_if_needed(
    ir: &mut String,
    reg: u8,
    value: &NativeStraightlineValue,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{reg}.slot\n"));
        }
        NativeStraightlineValue::Bool(value) => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{reg}.slot\n"));
        }
        NativeStraightlineValue::Nil => {
            ir.push_str(&format!("  store i64 0, ptr %r{reg}.slot\n"));
        }
        NativeStraightlineValue::F64(_)
        | NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DynamicStringIntMap { .. }
        | NativeStraightlineValue::DynamicIntList { .. }
        | NativeStraightlineValue::DynamicTextList { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Error { .. }
        | NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Cell { .. } => {}
    }
    Some(())
}

fn static_string_value_trusted_before(code: &[Instr32], pc_limit: usize, reg: u8, depth: usize) -> bool {
    if depth > 8 {
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
        .skip(last_write + 1)
        .take(pc_limit.saturating_sub(last_write))
        .any(|boundary| boundary);
    if crosses_boundary {
        return false;
    }
    let instr = code[last_write];
    if instr.opcode() == Opcode32::Move {
        return static_string_value_trusted_before(code, last_write, instr.b(), depth + 1);
    }
    true
}

fn instr_writes_register(instr: Instr32, reg: u8) -> bool {
    instr.a() == reg
        && !matches!(
            instr.opcode(),
            Opcode32::Nop
                | Opcode32::Jmp
                | Opcode32::Test
                | Opcode32::Return
                | Opcode32::SetGlobal
                | Opcode32::Raise
                | Opcode32::TryBegin
                | Opcode32::TryEnd
                | Opcode32::Extra
                | Opcode32::Wide
        )
}

pub(super) fn store_native_scalar_call_result(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    dst: u8,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::F64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store double {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::Bool(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::Nil => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
            if symbol.is_empty() {
                let symbol = format!("@lk_call_str_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                ir.push_str(&format!("  store ptr {symbol}, ptr %r{dst}.slot\n"));
            } else {
                ir.push_str(&format!("  store ptr {symbol}, ptr %r{dst}.slot\n"));
            }
        }
        NativeStraightlineValue::StringPtr(value) => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::StringPtr(value.clone()));
            ir.push_str(&format!("  store ptr {value}, ptr %r{dst}.slot\n"));
        }
        _ => return None,
    }
    Some(())
}

pub(super) fn store_native_inline_scalar_value(
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
        }
        NativeStraightlineValue::F64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store double {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::Bool(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::Nil => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 0, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
            if symbol.is_empty() {
                let symbol = format!("@lk_call_inline_str_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                ir.push_str(&format!("  store ptr {symbol}, ptr %call{call_pc}.r{dst}.slot\n"));
            } else {
                ir.push_str(&format!("  store ptr {symbol}, ptr %call{call_pc}.r{dst}.slot\n"));
            }
        }
        NativeStraightlineValue::StringPtr(value) => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::StringPtr(value.clone()));
            ir.push_str(&format!("  store ptr {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        _ => return None,
    }
    Some(())
}

pub(super) fn emit_inline_i64_binary_block(ir: &mut String, call_pc: usize, instr: Instr32, tmp_index: &mut usize) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load i64, ptr %call{call_pc}.r{}.slot\n", instr.c()));
    match instr.opcode() {
        Opcode32::AddInt => ir.push_str(&format!("  {out} = add i64 {lhs}, {rhs}\n")),
        Opcode32::SubInt => ir.push_str(&format!("  {out} = sub i64 {lhs}, {rhs}\n")),
        Opcode32::MulInt => ir.push_str(&format!("  {out} = mul i64 {lhs}, {rhs}\n")),
        Opcode32::DivInt => {
            let zero = next_tmp(tmp_index);
            let label = format!("call{call_pc}.div_ok_{}", out.trim_start_matches('%'));
            ir.push_str(&format!("  {zero} = icmp eq i64 {rhs}, 0\n"));
            ir.push_str(&format!("  br i1 {zero}, label %lk_divisor_zero, label %{label}\n"));
            ir.push_str(&format!("{label}:\n"));
            ir.push_str(&format!("  {out} = sdiv i64 {lhs}, {rhs}\n"));
        }
        Opcode32::ModInt => {
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

pub(super) fn emit_mixed_numeric_int_opcode_block(
    ir: &mut String,
    slot_prefix: &str,
    instr: Instr32,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) {
    let lhs = emit_numeric_load_as_f64(ir, slot_prefix, instr.b(), lhs_kind, tmp_index);
    let rhs = emit_numeric_load_as_f64(ir, slot_prefix, instr.c(), rhs_kind, tmp_index);
    let out = next_tmp(tmp_index);
    match instr.opcode() {
        Opcode32::AddInt => ir.push_str(&format!("  {out} = fadd double {lhs}, {rhs}\n")),
        Opcode32::SubInt => ir.push_str(&format!("  {out} = fsub double {lhs}, {rhs}\n")),
        Opcode32::MulInt => ir.push_str(&format!("  {out} = fmul double {lhs}, {rhs}\n")),
        Opcode32::DivInt => ir.push_str(&format!("  {out} = fdiv double {lhs}, {rhs}\n")),
        Opcode32::ModInt => ir.push_str(&format!("  {out} = frem double {lhs}, {rhs}\n")),
        _ => unreachable!("checked by caller"),
    }
    ir.push_str(&format!(
        "  store double {out}, ptr %{}r{}.slot\n",
        slot_prefix,
        instr.a()
    ));
}

pub(super) fn emit_dynamic_string_starts_with(
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

pub(super) fn emit_static_string_i64_map_get(
    ir: &mut String,
    extra_globals: &mut String,
    entries: &[(RuntimeMapKeyData, ConstRuntimeValue32Data)],
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

pub(super) fn static_string_i64_map_supported(entries: &[(RuntimeMapKeyData, ConstRuntimeValue32Data)]) -> bool {
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

fn const_runtime_i64_value(value: &ConstRuntimeValue32Data) -> Option<i64> {
    match value {
        ConstRuntimeValue32Data::Int(value) => Some(*value),
        ConstRuntimeValue32Data::Bool(value) => Some(i64::from(*value)),
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
        NativeScalarKind::Bool | NativeScalarKind::Nil | NativeScalarKind::StrPtr | NativeScalarKind::MaybeI64 => {
            unreachable!("checked by caller")
        }
    }
}

pub(super) fn inline_text_value_from_reg(
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
        NativeScalarKind::StrPtr => NativeTextPart::StrPtr(value),
        NativeScalarKind::MaybeI64 => return None,
    };
    Some(NativeStraightlineValue::Text(vec![part]))
}

pub(super) fn text_value_from_reg(
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
        NativeScalarKind::StrPtr => NativeTextPart::StrPtr(value),
        NativeScalarKind::MaybeI64 => return None,
    };
    Some(NativeStraightlineValue::Text(vec![part]))
}

fn text_value_from_native(value: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let parts = match value {
        NativeStraightlineValue::Text(parts) => parts,
        NativeStraightlineValue::I64(value) => vec![NativeTextPart::I64(value)],
        NativeStraightlineValue::F64(value) => vec![NativeTextPart::F64(value)],
        NativeStraightlineValue::Bool(value) => vec![NativeTextPart::Bool(value)],
        NativeStraightlineValue::Nil => vec![NativeTextPart::Nil],
        NativeStraightlineValue::StringPtr(value) => vec![NativeTextPart::StrPtr(value)],
        NativeStraightlineValue::String { symbol, value, .. } => vec![NativeTextPart::String { symbol, value }],
        _ => return None,
    };
    Some(NativeStraightlineValue::Text(parts))
}

pub(super) fn concat_text_values(
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

pub(super) fn emit_native_block_core_call_method(
    ir: &mut String,
    extra_globals: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if builtin != NativeBuiltin::CoreCallMethod {
        return None;
    }
    let [
        NativeStraightlineValue::Module(NativeModule::OsEnv),
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    else {
        return None;
    };
    if method != "get" || elements.len() != 2 {
        return None;
    }
    let name = native_const_string_arg(&elements[0])?;
    let default = native_const_string_arg(&elements[1])?;
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

fn native_const_string_arg(value: &ConstRuntimeValue32Data) -> Option<String> {
    match value {
        ConstRuntimeValue32Data::ShortStr(value) => Some(value.clone()),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn native_static_string(value: &str, symbol: String) -> NativeStraightlineValue {
    NativeStraightlineValue::String {
        symbol,
        value: value.to_string(),
        len: value.chars().count(),
        key_kind: super::straightline_value::native_runtime_string_key_kind(value),
    }
}

pub(super) fn emit_inline_branch_to_next(ir: &mut String, call_pc: usize, pc: usize, code_len: usize) {
    ir.push_str(&format!(
        "  br label {}\n",
        inline_native_label(call_pc, pc + 1, code_len)
    ));
}

pub(super) fn inline_native_label(call_pc: usize, target: usize, code_len: usize) -> String {
    if target >= code_len {
        format!("%call{call_pc}.exit")
    } else {
        format!("%call{call_pc}.bb{target}")
    }
}

pub(super) fn emit_inline_scalar_equality_block(
    ir: &mut String,
    call_pc: usize,
    instr: Instr32,
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
    let pred = if instr.opcode() == Opcode32::CmpInt { "eq" } else { "ne" };
    match (lhs_kind, rhs_kind) {
        (NativeScalarKind::MaybeI64, NativeScalarKind::Nil) | (NativeScalarKind::Nil, NativeScalarKind::MaybeI64) => {
            let maybe_reg = if lhs_kind == NativeScalarKind::MaybeI64 {
                instr.b()
            } else {
                instr.c()
            };
            let present = next_tmp(tmp_index);
            let nil_equal = if instr.opcode() == Opcode32::CmpInt { "eq" } else { "ne" };
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
        (NativeScalarKind::StrPtr, _) | (NativeScalarKind::MaybeI64, _) => return None,
    }
    ir.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
    Some(())
}

pub(super) fn emit_string_ptr_equality_block(ir: &mut String, instr: Instr32, tmp_index: &mut usize) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let cmp_value = next_tmp(tmp_index);
    let is_equal = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs} = load ptr, ptr %r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load ptr, ptr %r{}.slot\n", instr.c()));
    ir.push_str(&format!("  {cmp_value} = call i32 @strcmp(ptr {lhs}, ptr {rhs})\n"));
    let pred = if instr.opcode() == Opcode32::CmpInt { "eq" } else { "ne" };
    ir.push_str(&format!("  {is_equal} = icmp {pred} i32 {cmp_value}, 0\n"));
    ir.push_str(&format!("  {out} = zext i1 {is_equal} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
}

pub(super) fn emit_inline_string_ptr_equality_block(
    ir: &mut String,
    call_pc: usize,
    instr: Instr32,
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
    let pred = if instr.opcode() == Opcode32::CmpInt { "eq" } else { "ne" };
    ir.push_str(&format!("  {is_equal} = icmp {pred} i32 {cmp_value}, 0\n"));
    ir.push_str(&format!("  {out} = zext i1 {is_equal} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %call{call_pc}.r{}.slot\n", instr.a()));
}
