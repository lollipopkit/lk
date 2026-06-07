use crate::llvm::{
    const_display::llvm_string_constant,
    dynamic_containers::{emit_dynamic_int_list_set, emit_dynamic_string_int_map_set},
    ir_text::{emit_branch_to_next, next_tmp},
    known_key::native_known_string_key,
    scalar::{
        block_helpers::{local_register_kind_before, local_static_i64_before, three_regs_in_bounds},
        contains::local_static_string_before,
        facts::{NativeScalarFacts, NativeScalarKind},
    },
    straightline_value::{
        NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue, NativeStringKeyKind,
        NativeTextPart, native_static_i64_binary, native_static_index, native_static_set_index,
    },
};
use crate::vm::{FunctionData, Instr, Opcode};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_set_index_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    function: &FunctionData,
    register_count: usize,
    pc: usize,
    instr: Instr,
    code_len: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    let Some(target) = static_regs.get(instr.a() as usize).and_then(Clone::clone) else {
        return false;
    };
    if let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    } = target
    {
        if facts.register_kind_before(pc, instr.b()) != Some(NativeScalarKind::I64)
            || facts.register_kind_before(pc, instr.c()) != Some(NativeScalarKind::I64)
            || emit_dynamic_int_list_set(ir, id, instr.b(), instr.c(), tmp_index).is_none()
        {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        });
    } else if let NativeStraightlineValue::DynamicMap {
        id,
        key: NativeMapKeyKind::Str,
        value: NativeMapValueKind::I64,
    } = target
    {
        let key = dynamic_string_map_set_key(ir, extra_globals, function, pc, instr.b(), tmp_index);
        let Some(value_kind) = facts.register_kind_before(pc, instr.c()) else {
            return false;
        };
        if value_kind != NativeScalarKind::I64
            || emit_dynamic_string_int_map_set(ir, extra_globals, id, instr.c(), key, tmp_index).is_none()
        {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicMap {
            id,
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
        });
    } else if let NativeStraightlineValue::ArgList { mut elements } = target {
        let Some(NativeStraightlineValue::I64(index)) = static_regs
            .get(instr.b() as usize)
            .and_then(Clone::clone)
            .or_else(|| local_static_i64_before(code, int_consts, pc, instr.b()))
        else {
            return false;
        };
        let Some(value) = static_set_value(
            &NativeStraightlineValue::ArgList {
                elements: elements.clone(),
            },
            static_regs,
            code,
            int_consts,
            strings,
            pc,
            instr.c(),
        ) else {
            return false;
        };
        let index = index.parse::<usize>().ok();
        let Some(slot) = index.and_then(|index| elements.get_mut(index)) else {
            return false;
        };
        *slot = value;
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList { elements });
    } else {
        let Some(key) = dynamic_string_set_key(
            ir,
            extra_globals,
            static_regs,
            code,
            strings,
            function,
            pc,
            instr.b(),
            facts,
            tmp_index,
        ) else {
            return false;
        };
        let Some(value) = static_set_value(&target, static_regs, code, int_consts, strings, pc, instr.c()) else {
            return false;
        };
        let Some(value) = native_static_set_index(target, key, value) else {
            return false;
        };
        static_regs[instr.a() as usize] = Some(value.clone());
        update_recent_move_alias(static_regs, code, pc, instr.a(), value);
    }
    emit_branch_to_next(ir, pc, code_len);
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_set_field_k_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    function: &FunctionData,
    register_count: usize,
    pc: usize,
    instr: Instr,
    code_len: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    let _ = function;
    if instr.a() as usize >= register_count || instr.b() as usize >= register_count {
        return false;
    }
    let Some(target) = static_regs.get(instr.a() as usize).and_then(Clone::clone) else {
        return false;
    };
    let Some(key) = set_field_key(strings, instr) else {
        return false;
    };
    if let NativeStraightlineValue::DynamicMap {
        id,
        key: NativeMapKeyKind::Str,
        value: NativeMapValueKind::I64,
    } = target
    {
        let Some(value_kind) = facts.register_kind_before(pc, instr.b()) else {
            return false;
        };
        if value_kind != NativeScalarKind::I64
            || emit_dynamic_string_int_map_set(ir, extra_globals, id, instr.b(), key, tmp_index).is_none()
        {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicMap {
            id,
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
        });
    } else {
        let Some(value) = static_set_value(&target, static_regs, code, int_consts, strings, pc, instr.b()) else {
            return false;
        };
        let Some(value) = native_static_set_index(target, key, value) else {
            return false;
        };
        static_regs[instr.a() as usize] = Some(value.clone());
        update_recent_move_alias(static_regs, code, pc, instr.a(), value);
    }
    emit_branch_to_next(ir, pc, code_len);
    true
}

fn dynamic_string_map_set_key(
    ir: &mut String,
    extra_globals: &mut String,
    function: &FunctionData,
    pc: usize,
    reg: u8,
    tmp_index: &mut usize,
) -> NativeStraightlineValue {
    if let Some(key) = known_set_key(extra_globals, function, pc) {
        return key;
    }
    let ptr = next_tmp(tmp_index);
    ir.push_str(&format!("  {ptr} = load ptr, ptr %r{reg}.slot\n"));
    NativeStraightlineValue::StringPtr(ptr)
}

#[allow(clippy::too_many_arguments)]
fn dynamic_string_set_key(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    strings: &[String],
    function: &FunctionData,
    pc: usize,
    reg: u8,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(key) = known_set_key(extra_globals, function, pc) {
        return Some(key);
    }
    if let Some(key) = static_regs.get(reg as usize).and_then(Clone::clone) {
        if matches!(key, NativeStraightlineValue::Text(_)) {
            let ptr = next_tmp(tmp_index);
            ir.push_str(&format!("  {ptr} = load ptr, ptr %r{reg}.slot\n"));
            return Some(NativeStraightlineValue::StringPtr(ptr));
        }
        return Some(key);
    }
    dynamic_text_key_before(ir, static_regs, code, strings, pc, reg, facts, tmp_index).or_else(|| {
        let kind = facts
            .register_kind_before(pc, reg)
            .or_else(|| local_register_kind_before(code, pc, reg));
        matches!(kind, Some(NativeScalarKind::StrPtr)).then(|| {
            let key = next_tmp(tmp_index);
            ir.push_str(&format!("  {key} = load ptr, ptr %r{reg}.slot\n"));
            NativeStraightlineValue::StringPtr(key)
        })
    })
}

fn dynamic_text_key_before(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    strings: &[String],
    pc: usize,
    reg: u8,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let (write_pc, prev) = last_write_before(code, pc, reg)?;
    match prev.opcode() {
        Opcode::Move if prev.b() != reg => {
            dynamic_text_key_before(ir, static_regs, code, strings, write_pc, prev.b(), facts, tmp_index)
        }
        Opcode::ConcatString => {
            let mut parts =
                dynamic_text_parts_before(ir, static_regs, code, strings, write_pc, prev.b(), facts, tmp_index)?;
            parts.extend(dynamic_text_parts_before(
                ir,
                static_regs,
                code,
                strings,
                write_pc,
                prev.c(),
                facts,
                tmp_index,
            )?);
            Some(NativeStraightlineValue::Text(parts))
        }
        Opcode::ToString => {
            dynamic_text_parts_before(ir, static_regs, code, strings, write_pc, prev.b(), facts, tmp_index)
                .map(NativeStraightlineValue::Text)
        }
        _ => None,
    }
}

fn dynamic_text_parts_before(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    strings: &[String],
    pc: usize,
    reg: u8,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<Vec<NativeTextPart>> {
    if let Some(value) = static_regs.get(reg as usize).and_then(Clone::clone) {
        return text_parts_from_value(value);
    }
    if let Some(NativeStraightlineValue::String { symbol, value, .. }) =
        local_static_string_before(code, strings, pc, reg)
    {
        return Some(vec![NativeTextPart::String { symbol, value }]);
    }
    let kind = facts
        .register_kind_before(pc, reg)
        .or_else(|| local_register_kind_before(code, pc, reg))?;
    match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
            Some(vec![NativeTextPart::I64(value)])
        }
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
            Some(vec![NativeTextPart::StrPtr(value)])
        }
        NativeScalarKind::Bool => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
            Some(vec![NativeTextPart::Bool(value)])
        }
        NativeScalarKind::Nil => Some(vec![NativeTextPart::Nil]),
        NativeScalarKind::F64 => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load double, ptr %r{reg}.slot\n"));
            Some(vec![NativeTextPart::F64(value)])
        }
    }
}

fn text_parts_from_value(value: NativeStraightlineValue) -> Option<Vec<NativeTextPart>> {
    match value {
        NativeStraightlineValue::Text(parts) => Some(parts),
        NativeStraightlineValue::String { symbol, value, .. } => Some(vec![NativeTextPart::String { symbol, value }]),
        NativeStraightlineValue::StringPtr(value) => Some(vec![NativeTextPart::StrPtr(value)]),
        NativeStraightlineValue::I64(value) | NativeStraightlineValue::MaybeI64 { value, .. } => {
            Some(vec![NativeTextPart::I64(value)])
        }
        NativeStraightlineValue::F64(value) | NativeStraightlineValue::MaybeF64 { value, .. } => {
            Some(vec![NativeTextPart::F64(value)])
        }
        NativeStraightlineValue::Bool(value) | NativeStraightlineValue::MaybeBool { value, .. } => {
            Some(vec![NativeTextPart::Bool(value)])
        }
        NativeStraightlineValue::Nil => Some(vec![NativeTextPart::Nil]),
        _ => None,
    }
}

fn last_write_before(code: &[Instr], pc: usize, reg: u8) -> Option<(usize, Instr)> {
    code.iter().copied().take(pc).enumerate().rev().find(|(_, instr)| {
        instr.a() == reg
            && !matches!(
                instr.opcode(),
                Opcode::Nop | Opcode::Jmp | Opcode::Test | Opcode::BrFalse | Opcode::BrTrue
            )
    })
}

fn known_set_key(extra_globals: &mut String, function: &FunctionData, pc: usize) -> Option<NativeStraightlineValue> {
    let key = native_known_string_key(function, pc, format!("@lk_known_set_key_{pc}"))?;
    if let NativeStraightlineValue::String { symbol, value, .. } = &key {
        extra_globals.push_str(&llvm_string_constant(symbol, value));
    }
    Some(key)
}

fn set_field_key(strings: &[String], instr: Instr) -> Option<NativeStraightlineValue> {
    let value = strings.get(instr.c() as usize)?;
    Some(NativeStraightlineValue::String {
        symbol: String::new(),
        value: value.clone(),
        len: value.len(),
        key_kind: NativeStringKeyKind::Short,
    })
}

fn update_recent_move_alias(
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    reg: u8,
    value: NativeStraightlineValue,
) {
    for prev_pc in (pc.saturating_sub(16)..pc).rev() {
        let Some(prev) = code.get(prev_pc).copied() else {
            return;
        };
        if prev.a() != reg {
            continue;
        }
        if prev.opcode() == Opcode::Move && prev.b() != reg {
            if let Some(slot) = static_regs.get_mut(prev.b() as usize) {
                *slot = Some(value);
            }
        }
        return;
    }
}

fn static_set_value(
    target: &NativeStraightlineValue,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    static_regs
        .get(reg as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_expr_before(target, static_regs, code, int_consts, strings, pc, reg))
}

fn local_static_i64_expr_before(
    target: &NativeStraightlineValue,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::LoadInt => local_static_i64_before(code, int_consts, pc, reg),
            Opcode::Move if prev.b() != reg => {
                local_static_i64_expr_before(target, static_regs, code, int_consts, strings, prev_pc, prev.b())
            }
            Opcode::GetIndex => local_static_object_index(target, code, strings, prev_pc, prev.c()),
            Opcode::GetFieldK => local_static_object_field(target, strings, prev),
            Opcode::AddInt | Opcode::SubInt | Opcode::MulInt | Opcode::DivInt | Opcode::ModInt => {
                let NativeStraightlineValue::I64(lhs) =
                    static_i64_operand(target, static_regs, code, int_consts, strings, prev_pc, prev.b())?
                else {
                    return None;
                };
                let NativeStraightlineValue::I64(rhs) =
                    static_i64_operand(target, static_regs, code, int_consts, strings, prev_pc, prev.c())?
                else {
                    return None;
                };
                native_static_i64_binary(&lhs, &rhs, prev.opcode()).map(NativeStraightlineValue::I64)
            }
            _ => None,
        };
    }
    None
}

fn static_i64_operand(
    target: &NativeStraightlineValue,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    static_regs
        .get(reg as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_before(code, int_consts, pc, reg))
        .or_else(|| local_static_i64_expr_before(target, static_regs, code, int_consts, strings, pc, reg))
}

fn local_static_object_index(
    target: &NativeStraightlineValue,
    code: &[Instr],
    strings: &[String],
    pc: usize,
    key_reg: u8,
) -> Option<NativeStraightlineValue> {
    let key = local_static_string_before(code, strings, pc, key_reg)?;
    native_static_index(target.clone(), key, String::new())
}

fn local_static_object_field(
    target: &NativeStraightlineValue,
    strings: &[String],
    instr: Instr,
) -> Option<NativeStraightlineValue> {
    let key_text = strings.get(instr.c() as usize)?;
    let key = NativeStraightlineValue::String {
        symbol: String::new(),
        value: key_text.clone(),
        len: key_text.len(),
        key_kind: NativeStringKeyKind::Short,
    };
    native_static_index(target.clone(), key, String::new())
}
