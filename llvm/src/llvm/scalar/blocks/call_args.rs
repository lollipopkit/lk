use crate::{
    llvm::{
        const_display::llvm_string_constant,
        ir_text::emit_branch_to_next,
        output::{emit_native_static_core_call_method, emit_native_static_parse_builtin},
        scalar::{
            block_helpers::{
                local_static_container_before, scalar_arg_value, static_call_args, static_string_value_trusted_at_call,
                store_native_scalar_call_result,
            },
            contains::{local_static_iter_zip_before, static_iter_builtin_call},
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{
            NativeBuiltin, NativeStraightlineValue, native_runtime_const_value, native_runtime_string_key_kind,
            native_static_global, native_static_index, native_static_object_from_fields,
        },
    },
    vm::{ConstHeapValueData, Instr, ModuleArtifact, Opcode},
};

use super::{
    i64_list_methods::{emit_dynamic_i64_list_builtin_call, emit_dynamic_i64_list_builtin_call_from_regs},
    list_methods::{
        emit_dynamic_f64_list_builtin_call, emit_dynamic_f64_list_builtin_call_from_regs,
        emit_dynamic_ptr_list_builtin_call, emit_dynamic_ptr_list_builtin_call_from_regs,
    },
    map_methods::{
        emit_dynamic_map_delete_call, emit_dynamic_map_get_call, emit_dynamic_map_has_call, emit_dynamic_map_set_call,
        emit_dynamic_map_values_call,
    },
    runtime_builtins::emit_runtime_builtin_call,
};
use crate::llvm::{output::emit_native_builtin_call, scalar::block_helpers::emit_static_formatted_print};

pub(super) fn static_or_recovered_call_args(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    pc: usize,
    callee: u8,
    count: u8,
) -> Option<Vec<NativeStraightlineValue>> {
    let direct = static_call_args(static_regs, callee, count);
    direct.or_else(|| {
        let start = callee as usize + 1;
        let end = start.checked_add(count as usize)?;
        (start..end)
            .map(|reg| {
                static_regs
                    .get(reg)
                    .cloned()
                    .flatten()
                    .or_else(|| {
                        local_static_value_before(
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            global_names,
                            &[],
                            pc,
                            u8::try_from(reg).ok()?,
                        )
                    })
            })
            .collect::<Option<Vec<_>>>()
    })
}

pub(super) fn static_or_recovered_call_target(
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    callee: u8,
) -> Option<NativeStraightlineValue> {
    static_regs
        .get(callee as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_value_before(code, int_consts, strings, heap_values, global_names, static_globals, pc, callee))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_recovered_builtin_call_block(
    artifact: &ModuleArtifact,
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    facts: &NativeScalarFacts,
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    builtin: NativeBuiltin,
    register_count: usize,
    tmp_index: &mut usize,
) -> bool {
    if matches!(builtin, NativeBuiltin::Print | NativeBuiltin::Println)
        && instr.c() == 1
        && emit_runtime_builtin_call(ir, builtin, instr, register_count, facts, pc, tmp_index)
    {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_runtime_formatted_print_call(
        ir,
        extra_globals,
        builtin,
        instr,
        code,
        strings,
        heap_values,
        static_regs,
        facts,
        pc,
        register_count,
        tmp_index,
    ) {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_dynamic_ptr_list_builtin_call_from_regs(
        ir,
        extra_globals,
        static_regs,
        instr,
        builtin,
        facts,
        pc,
        tmp_index,
    )
    .is_some()
    {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_dynamic_i64_list_builtin_call_from_regs(
        ir,
        static_regs,
        code,
        heap_values,
        instr,
        builtin,
        facts,
        pc,
        tmp_index,
    )
    .is_some()
    {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if emit_dynamic_f64_list_builtin_call_from_regs(ir, static_regs, instr, builtin, facts, pc, tmp_index).is_some() {
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let static_args = (instr.b() as usize + 1..instr.b() as usize + 1 + instr.c() as usize)
        .map(|reg| {
            local_static_iter_zip_before(
                global_names,
                code,
                int_consts,
                strings,
                heap_values,
                pc,
                u8::try_from(reg).ok()?,
                static_regs,
            )
            .or_else(|| {
                let reg = u8::try_from(reg).ok()?;
                local_static_heap_const_before(code, heap_values, pc, reg)
                    .or_else(|| local_static_container_before(code, heap_values, pc, reg))
            })
            .or_else(|| {
                let reg_u8 = u8::try_from(reg).ok()?;
                static_regs
                    .get(reg)
                    .cloned()
                    .flatten()
                    .and_then(|value| trusted_static_call_arg(code, pc, reg_u8, value))
            })
            .or_else(|| {
                let reg = u8::try_from(reg).ok()?;
                (!matches!(builtin, NativeBuiltin::Print | NativeBuiltin::Println)
                    && static_string_value_trusted_at_call(code, pc, reg))
                .then(|| local_static_string_before(code, strings, pc, reg))
                .flatten()
            })
            .or_else(|| {
                let reg = u8::try_from(reg).ok()?;
                static_string_value_trusted_at_call(code, pc, reg)
                    .then(|| local_static_i64_before(code, int_consts, pc, reg))
                    .flatten()
            })
        })
        .collect::<Option<Vec<_>>>();
    if let Some(args) = static_args {
        if let Some(value) = static_iter_builtin_call(
            artifact,
            code,
            int_consts,
            strings,
            heap_values,
            builtin,
            &args,
            static_globals,
            ir,
            tmp_index,
        ) {
            store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index);
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        let defer_formatted_print = matches!(builtin, NativeBuiltin::Print | NativeBuiltin::Println) && args.len() > 1;
        if !defer_formatted_print
            && let Some(value) = emit_static_formatted_print(ir, extra_globals, builtin, &args, tmp_index)
        {
            store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index);
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_set_call(ir, extra_globals, static_regs, instr, pc, builtin, &args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_values_call(ir, static_regs, instr, pc, builtin, &args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_get_call(ir, extra_globals, static_regs, instr, pc, builtin, &args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_has_call(ir, extra_globals, static_regs, instr, pc, builtin, &args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_delete_call(ir, extra_globals, static_regs, instr, pc, builtin, &args, tmp_index).is_some()
        {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_ptr_list_builtin_call(ir, extra_globals, static_regs, instr, pc, builtin, &args, tmp_index)
            .is_some()
        {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_i64_list_builtin_call(ir, static_regs, code, heap_values, instr, pc, builtin, &args, tmp_index)
            .is_some()
        {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_f64_list_builtin_call(ir, static_regs, instr, pc, builtin, &args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if let Some(value) = emit_native_builtin_call(ir, builtin, &args, tmp_index) {
            store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index);
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
    }
    let scalar_args = (instr.b() as usize + 1..instr.b() as usize + 1 + instr.c() as usize)
        .enumerate()
        .map(|(arg_index, reg)| {
            let force_slot_string =
                matches!(builtin, NativeBuiltin::Print | NativeBuiltin::Println) && (instr.c() == 1 || arg_index > 0);
            trusted_scalar_arg_value(ir, facts, pc, static_regs, reg, code, force_slot_string, tmp_index)
        })
        .collect::<Option<Vec<_>>>();
    if let Some(args) = scalar_args.as_ref() {
        if emit_dynamic_map_set_call(ir, extra_globals, static_regs, instr, pc, builtin, args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_values_call(ir, static_regs, instr, pc, builtin, args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_get_call(ir, extra_globals, static_regs, instr, pc, builtin, args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_has_call(ir, extra_globals, static_regs, instr, pc, builtin, args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_map_delete_call(ir, extra_globals, static_regs, instr, pc, builtin, args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_ptr_list_builtin_call(ir, extra_globals, static_regs, instr, pc, builtin, args, tmp_index)
            .is_some()
        {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_i64_list_builtin_call(ir, static_regs, code, heap_values, instr, pc, builtin, args, tmp_index)
            .is_some()
        {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_f64_list_builtin_call(ir, static_regs, instr, pc, builtin, args, tmp_index).is_some() {
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if let Some(value) = emit_native_builtin_call(ir, builtin, args, tmp_index) {
            store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index);
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
    }
    if let Some(args) = scalar_args
        && let Some(value) = emit_static_formatted_print(ir, extra_globals, builtin, &args, tmp_index)
    {
        store_native_scalar_call_result(ir, extra_globals, static_regs, instr.a(), value, tmp_index);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    if !emit_runtime_builtin_call(ir, builtin, instr, register_count, facts, pc, tmp_index) {
        return false;
    }
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn trusted_static_call_arg(
    code: &[Instr],
    pc: usize,
    reg: u8,
    value: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    match value {
        NativeStraightlineValue::I64(_)
        | NativeStraightlineValue::F64(_)
        | NativeStraightlineValue::Bool(_)
        | NativeStraightlineValue::Nil
        | NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DisplayMap { .. }
        | NativeStraightlineValue::Object { .. } => static_string_value_trusted_at_call(code, pc, reg).then_some(value),
        _ => Some(value),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_runtime_formatted_print_call(
    ir: &mut String,
    extra_globals: &mut String,
    builtin: NativeBuiltin,
    instr: Instr,
    code: &[Instr],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    static_regs: &[Option<NativeStraightlineValue>],
    facts: &NativeScalarFacts,
    pc: usize,
    register_count: usize,
    tmp_index: &mut usize,
) -> bool {
    if !matches!(builtin, NativeBuiltin::Print | NativeBuiltin::Println) || instr.c() <= 1 {
        return false;
    }
    let format_reg = instr.b().saturating_add(1);
    let format_value = local_static_string_before(code, strings, pc, format_reg)
        .or_else(|| local_static_heap_const_before(code, heap_values, pc, format_reg));
    let Some(NativeStraightlineValue::String { value: format, .. }) = format_value else {
        return false;
    };
    let mut next_arg = instr.b().saturating_add(2);
    let max_arg = instr.b().saturating_add(instr.c());
    let mut check_arg = next_arg;
    let mut needs_runtime_slot_arg = false;
    while check_arg <= max_arg {
        let Some(uses_runtime_slot) =
            runtime_formatted_print_arg_support(facts, static_regs, code, strings, pc, check_arg)
        else {
            return false;
        };
        needs_runtime_slot_arg |= uses_runtime_slot;
        check_arg = check_arg.saturating_add(1);
    }
    if !needs_runtime_slot_arg {
        return false;
    }
    let mut remaining = format.as_str();
    while let Some(pos) = remaining.find("{}") {
        let (chunk, after_chunk) = remaining.split_at(pos);
        emit_runtime_print_chunk(ir, extra_globals, chunk, tmp_index);
        if next_arg > max_arg || next_arg as usize >= register_count {
            return false;
        }
        if !emit_runtime_print_arg(ir, facts, pc, next_arg, tmp_index) {
            return false;
        }
        next_arg = next_arg.saturating_add(1);
        remaining = &after_chunk[2..];
    }
    if next_arg <= max_arg {
        return false;
    }
    emit_runtime_print_chunk(ir, extra_globals, remaining, tmp_index);
    if builtin == NativeBuiltin::Println {
        ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    true
}

fn runtime_formatted_print_arg_support(
    facts: &NativeScalarFacts,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<bool> {
    match static_regs.get(reg as usize).and_then(|value| value.as_ref()) {
        Some(NativeStraightlineValue::String { .. } | NativeStraightlineValue::Text(_)) => {
            return facts.register_kind_before(pc, reg).is_some().then_some(true);
        }
        Some(
            NativeStraightlineValue::List { .. }
            | NativeStraightlineValue::ArgList { .. }
            | NativeStraightlineValue::Map { .. }
            | NativeStraightlineValue::DisplayMap { .. }
            | NativeStraightlineValue::Object { .. }
            | NativeStraightlineValue::DynamicList { .. }
            | NativeStraightlineValue::DynamicMap { .. }
            | NativeStraightlineValue::Builtin(_)
            | NativeStraightlineValue::Module(_)
            | NativeStraightlineValue::Function(_)
            | NativeStraightlineValue::Closure { .. },
        ) => return None,
        _ => {}
    }
    if local_static_string_before(code, strings, pc, reg).is_some()
        && matches!(
            facts.register_kind_before(pc, reg),
            Some(NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr)
        )
    {
        return Some(true);
    }
    facts.register_kind_before(pc, reg).is_some().then_some(false)
}

fn emit_runtime_print_chunk(ir: &mut String, extra_globals: &mut String, chunk: &str, tmp_index: &mut usize) {
    if chunk.is_empty() {
        return;
    }
    let symbol = format!("@lk_fmt_chunk_{}", *tmp_index);
    *tmp_index += 1;
    extra_globals.push_str(&llvm_string_constant(&symbol, chunk));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
    ));
}

fn emit_runtime_print_arg(
    ir: &mut String,
    facts: &NativeScalarFacts,
    pc: usize,
    reg: u8,
    tmp_index: &mut usize,
) -> bool {
    let Some(kind) = facts.register_kind_before(pc, reg) else {
        return false;
    };
    match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => {
            let value = crate::llvm::ir_text::next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 {value})\n"
            ));
        }
        NativeScalarKind::F64 => {
            let value = crate::llvm::ir_text::next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load double, ptr %r{reg}.slot\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double {value})\n"
            ));
        }
        NativeScalarKind::Bool => {
            let value = crate::llvm::ir_text::next_tmp(tmp_index);
            let cond = crate::llvm::ir_text::next_tmp(tmp_index);
            let text = crate::llvm::ir_text::next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
            ir.push_str(&format!(
                "  {text} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {text})\n"
            ));
        }
        NativeScalarKind::Nil => {
            ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr @lk_nil_text)\n");
        }
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => {
            let value = crate::llvm::ir_text::next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {value})\n"
            ));
        }
    }
    true
}

fn trusted_scalar_arg_value(
    ir: &mut String,
    facts: &NativeScalarFacts,
    pc: usize,
    static_regs: &[Option<NativeStraightlineValue>],
    reg: usize,
    code: &[Instr],
    force_slot_string: bool,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let reg_u8 = u8::try_from(reg).ok()?;
    if let Some(value) = static_regs.get(reg).cloned().flatten()
        && force_slot_string
        && matches!(
            value,
            NativeStraightlineValue::String { .. } | NativeStraightlineValue::Text(_)
        )
    {
        return scalar_string_slot_arg_value(ir, reg_u8, tmp_index);
    }
    if let Some(value) = static_regs.get(reg).cloned().flatten()
        && trusted_static_call_arg(code, pc, reg_u8, value).is_none()
    {
        return scalar_slot_arg_value(ir, facts, pc, reg_u8, tmp_index);
    }
    scalar_arg_value(ir, "", facts, pc, static_regs, reg, tmp_index)
}

fn scalar_string_slot_arg_value(ir: &mut String, reg: u8, tmp_index: &mut usize) -> Option<NativeStraightlineValue> {
    let value = crate::llvm::ir_text::next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
    Some(NativeStraightlineValue::StringPtr(value))
}

fn scalar_slot_arg_value(
    ir: &mut String,
    facts: &NativeScalarFacts,
    pc: usize,
    reg: u8,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let kind = facts.register_kind_before(pc, reg)?;
    if kind == crate::llvm::scalar::facts::NativeScalarKind::Nil {
        return Some(NativeStraightlineValue::Nil);
    }
    let value = crate::llvm::ir_text::next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %r{reg}.slot\n"));
    match kind {
        crate::llvm::scalar::facts::NativeScalarKind::I64 | crate::llvm::scalar::facts::NativeScalarKind::MaybeI64 => {
            Some(NativeStraightlineValue::I64(value))
        }
        crate::llvm::scalar::facts::NativeScalarKind::F64 => Some(NativeStraightlineValue::F64(value)),
        crate::llvm::scalar::facts::NativeScalarKind::Bool => Some(NativeStraightlineValue::Bool(value)),
        crate::llvm::scalar::facts::NativeScalarKind::Nil => Some(NativeStraightlineValue::Nil),
        crate::llvm::scalar::facts::NativeScalarKind::StrPtr
        | crate::llvm::scalar::facts::NativeScalarKind::MaybeStrPtr => Some(NativeStraightlineValue::StringPtr(value)),
    }
}

fn local_static_call_result_before(
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::Move if prev.b() != reg => {
                local_static_call_result_before(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.b(),
                )
            }
            Opcode::Call if prev.a() == prev.b() => {
                let target =
                    local_static_value_before(
                        code,
                        int_consts,
                        strings,
                        heap_values,
                        global_names,
                        static_globals,
                        prev_pc,
                        prev.b(),
                    )?;
                let args = local_static_call_args(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.b(),
                    prev.c(),
                )?;
                match target {
                    NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod) => {
                        let mut tmp_index = 0usize;
                        emit_native_static_core_call_method(&args, &mut tmp_index)
                    }
                    NativeStraightlineValue::Builtin(builtin) => emit_native_static_parse_builtin(builtin, &args),
                    _ => None,
                }
            }
            _ => None,
        };
    }
    None
}

fn local_static_call_args(
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    callee: u8,
    count: u8,
) -> Option<Vec<NativeStraightlineValue>> {
    let start = callee as usize + 1;
    let end = start.checked_add(count as usize)?;
    (start..end)
        .map(|reg| {
            local_static_value_before(
                code,
                int_consts,
                strings,
                heap_values,
                global_names,
                static_globals,
                pc,
                u8::try_from(reg).ok()?,
            )
        })
        .collect()
}

fn local_static_value_before(
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    local_static_string_before(code, strings, pc, reg)
        .or_else(|| local_static_i64_before(code, int_consts, pc, reg))
        .or_else(|| local_static_heap_const_before(code, heap_values, pc, reg))
        .or_else(|| local_static_global_before(code, global_names, static_globals, pc, reg))
        .or_else(|| local_static_index_before(code, int_consts, strings, heap_values, global_names, static_globals, pc, reg))
        .or_else(|| {
            local_static_new_list_before(code, int_consts, strings, heap_values, global_names, static_globals, pc, reg)
        })
        .or_else(|| local_static_object_before(code, int_consts, strings, heap_values, global_names, static_globals, pc, reg))
        .or_else(|| {
            local_static_call_result_before(code, int_consts, strings, heap_values, global_names, static_globals, pc, reg)
        })
}

fn local_static_object_before(
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::NewObject => {
                let start = prev.b() as usize;
                let width = (prev.c() as usize).checked_mul(2)?.checked_add(1)?;
                let fields = (start..start.checked_add(width)?)
                    .map(|field_reg| {
                        local_static_value_before(
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            global_names,
                            static_globals,
                            prev_pc,
                            u8::try_from(field_reg).ok()?,
                        )
                    })
                    .collect::<Option<Vec<_>>>()?;
                native_static_object_from_fields(&fields, String::new())
            }
            Opcode::Move if prev.b() != reg => {
                local_static_object_before(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.b(),
                )
            }
            _ => None,
        };
    }
    None
}

fn local_static_new_list_before(
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::NewList => {
                let start = prev.b();
                let end = start.checked_add(prev.c())?;
                let elements = (start..end)
                    .map(|item| {
                        local_static_value_before(
                            code,
                            int_consts,
                            strings,
                            heap_values,
                            global_names,
                            static_globals,
                            prev_pc,
                            item,
                        )
                        .and_then(|value| native_runtime_const_value(&value))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(NativeStraightlineValue::List {
                    symbol: String::new(),
                    value: String::new(),
                    elements,
                })
            }
            Opcode::Move if prev.b() != reg => {
                local_static_new_list_before(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.b(),
                )
            }
            _ => None,
        };
    }
    None
}

fn local_static_index_before(
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::GetIndex | Opcode::GetList => {
                let target = local_static_value_before(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.b(),
                )?;
                let key = local_static_value_before(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.c(),
                )?;
                native_static_index(target, key, String::new())
            }
            Opcode::Move if prev.b() != reg => {
                local_static_index_before(
                    code,
                    int_consts,
                    strings,
                    heap_values,
                    global_names,
                    static_globals,
                    prev_pc,
                    prev.b(),
                )
            }
            _ => None,
        };
    }
    None
}

fn local_static_global_before(
    code: &[Instr],
    global_names: &[String],
    static_globals: &[Option<NativeStraightlineValue>],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::GetGlobal => static_globals
                .get(prev.bx() as usize)
                .and_then(Clone::clone)
                .or_else(|| global_names.get(prev.bx() as usize).and_then(|name| native_static_global(name))),
            Opcode::Move if prev.b() != reg => {
                local_static_global_before(code, global_names, static_globals, prev_pc, prev.b())
            }
            _ => None,
        };
    }
    None
}

fn local_static_i64_before(code: &[Instr], int_consts: &[i64], pc: usize, reg: u8) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadInt => int_consts
                .get(prev.bx() as usize)
                .map(|value| NativeStraightlineValue::I64(value.to_string())),
            Opcode::LoadBool => Some(NativeStraightlineValue::Bool(i64::from(prev.b() != 0).to_string())),
            Opcode::Move if prev.b() != reg => local_static_i64_before(code, int_consts, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

fn local_static_string_before(
    code: &[Instr],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadString => {
                let value = strings.get(prev.bx() as usize)?;
                Some(NativeStraightlineValue::String {
                    symbol: String::new(),
                    value: value.clone(),
                    len: value.chars().count(),
                    key_kind: native_runtime_string_key_kind(value),
                })
            }
            Opcode::Move if prev.b() != reg => local_static_string_before(code, strings, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

fn local_static_heap_const_before(
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(128)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadHeapConst => crate::llvm::straightline_value::native_straightline_heap_const_value(
                0,
                prev.bx(),
                heap_values.get(prev.bx() as usize)?,
            ),
            Opcode::Move if prev.b() != reg => local_static_heap_const_before(code, heap_values, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}
