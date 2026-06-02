use crate::llvm::{
    dynamic_containers::{emit_dynamic_int_list_set, emit_dynamic_string_int_map_set},
    ir_text::{emit_branch_to_next, next_tmp},
    scalar::{
        block_helpers::{local_static_i64_before, three_regs_in_bounds},
        contains::local_static_string_before,
        facts::{NativeScalarFacts, NativeScalarKind},
    },
    straightline_value::{
        NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue, native_static_i64_binary,
        native_static_index, native_static_set_index,
    },
};
use crate::vm::{Instr32, Opcode32};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_set_index_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    register_count: usize,
    pc: usize,
    instr: Instr32,
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
        let key = if let Some(key) = static_regs.get(instr.b() as usize).and_then(Clone::clone) {
            key
        } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
            let key = next_tmp(tmp_index);
            ir.push_str(&format!("  {key} = load ptr, ptr %r{}.slot\n", instr.b()));
            NativeStraightlineValue::StringPtr(key)
        } else {
            return false;
        };
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
        let key = if let Some(key) = static_regs.get(instr.b() as usize).and_then(Clone::clone) {
            key
        } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
            let key = next_tmp(tmp_index);
            ir.push_str(&format!("  {key} = load ptr, ptr %r{}.slot\n", instr.b()));
            NativeStraightlineValue::StringPtr(key)
        } else {
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

fn update_recent_move_alias(
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
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
        if prev.opcode() == Opcode32::Move && prev.b() != reg {
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
    code: &[Instr32],
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
    code: &[Instr32],
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
            Opcode32::LoadInt => local_static_i64_before(code, int_consts, pc, reg),
            Opcode32::Move if prev.b() != reg => {
                local_static_i64_expr_before(target, static_regs, code, int_consts, strings, prev_pc, prev.b())
            }
            Opcode32::GetIndex => local_static_object_index(target, code, strings, prev_pc, prev.c()),
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
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
    code: &[Instr32],
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
    code: &[Instr32],
    strings: &[String],
    pc: usize,
    key_reg: u8,
) -> Option<NativeStraightlineValue> {
    let key = local_static_string_before(code, strings, pc, key_reg)?;
    native_static_index(target.clone(), key, String::new())
}
