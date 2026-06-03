use crate::{
    llvm::{
        dynamic_containers::{
            emit_dynamic_f64_list_push, emit_dynamic_int_list_push, emit_dynamic_pair_list_push,
            emit_dynamic_ptr_list_push, emit_dynamic_ptr_list_push_value, emit_dynamic_text_list_push,
            emit_dynamic_text_list_push_len,
        },
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::{local_static_container_before, local_static_i64_before, text_value_from_reg},
            blocks::const_lists::emit_const_list_element_index,
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{NativeListElementKind, NativeStraightlineValue, native_static_list_push},
    },
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Opcode32},
};

pub(super) fn emit_list_push_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return false;
    }
    let target = static_regs
        .get(instr.a() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.a()));
    let Some(NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    }) = target.clone()
    else {
        if let Some(target) = target.clone()
            && let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone)
        {
            if let NativeStraightlineValue::ArgList { mut elements } = target {
                elements.push(value);
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList { elements });
                emit_branch_to_next(ir, pc, code.len());
                return true;
            }
            if let Some(value) = native_static_list_push(target, value) {
                static_regs[instr.a() as usize] = Some(value);
                emit_branch_to_next(ir, pc, code.len());
                return true;
            }
        }
        if let Some(NativeStraightlineValue::DynamicPairList { id, first, second }) = target.clone()
            && let Some((field_first, field_second)) =
                dynamic_pair_fields(static_regs.get(instr.b() as usize).and_then(Clone::clone))
        {
            let Some(next_first) = dynamic_pair_field_kind(&field_first) else {
                return false;
            };
            let Some(next_second) = dynamic_pair_field_kind(&field_second) else {
                return false;
            };
            if first != next_first || second != next_second {
                return false;
            }
            if emit_dynamic_pair_list_push(ir, id, &field_first, &field_second, tmp_index).is_none() {
                return false;
            }
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicPairList { id, first, second });
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if let Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Text,
        }) = target.clone()
        {
            let Some(value) = static_regs.get(instr.b() as usize).and_then(Clone::clone) else {
                return false;
            };
            if emit_dynamic_text_list_push(ir, id, value, tmp_index).is_none() {
                return false;
            }
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id,
                element: NativeListElementKind::Text,
            });
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if let Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::F64,
        }) = target.clone()
        {
            if facts.register_kind_before(pc, instr.b()) != Some(NativeScalarKind::F64)
                || emit_dynamic_f64_list_push(ir, id, instr.b(), tmp_index).is_none()
            {
                return false;
            }
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id,
                element: NativeListElementKind::F64,
            });
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if let Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Bool,
        }) = target.clone()
        {
            if facts.register_kind_before(pc, instr.b()) != Some(NativeScalarKind::Bool)
                || emit_dynamic_int_list_push(ir, id, instr.b(), tmp_index).is_none()
            {
                return false;
            }
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id,
                element: NativeListElementKind::Bool,
            });
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if let Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::StrPtr,
        }) = target
        {
            if facts.register_kind_before(pc, instr.b()) != Some(NativeScalarKind::StrPtr) {
                return false;
            }
            let pushed = static_regs
                .get(instr.b() as usize)
                .and_then(Clone::clone)
                .and_then(|value| dynamic_ptr_value_expr(&value))
                .and_then(|value| emit_dynamic_ptr_list_push_value(ir, id, &value, tmp_index))
                .or_else(|| {
                    emit_nested_const_list_string_field_for_push(
                        ir,
                        extra_globals,
                        code,
                        int_consts,
                        heap_values,
                        pc,
                        instr.b(),
                        tmp_index,
                    )
                    .and_then(|()| emit_dynamic_ptr_list_push(ir, id, instr.b(), tmp_index))
                })
                .or_else(|| emit_dynamic_ptr_list_push(ir, id, instr.b(), tmp_index));
            if pushed.is_none() {
                return false;
            }
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id,
                element: NativeListElementKind::StrPtr,
            });
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        return false;
    };
    if let Some((first, second)) = dynamic_pair_fields(static_regs.get(instr.b() as usize).and_then(Clone::clone))
        && emit_dynamic_pair_list_push(ir, id, &first, &second, tmp_index).is_some()
    {
        let first = dynamic_pair_field_kind(&first).unwrap_or(NativeListElementKind::StrPtr);
        let second = dynamic_pair_field_kind(&second).unwrap_or(NativeListElementKind::I64);
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicPairList { id, first, second });
    } else if let Some(NativeStraightlineValue::DynamicArgListElement { elements, .. }) =
        static_regs.get(instr.b() as usize).and_then(Clone::clone)
    {
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList { elements });
    } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::I64)
        && !matches!(
            static_regs.get(instr.b() as usize).and_then(Clone::clone),
            Some(
                NativeStraightlineValue::DynamicTextChar
                    | NativeStraightlineValue::Text(_)
                    | NativeStraightlineValue::String { .. }
                    | NativeStraightlineValue::StringPtr(_)
            )
        )
    {
        if emit_dynamic_int_list_push(ir, id, instr.b(), tmp_index).is_none() {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        });
    } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::F64) {
        if emit_dynamic_f64_list_push(ir, id, instr.b(), tmp_index).is_none() {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::F64,
        });
    } else if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::Bool) {
        if emit_dynamic_int_list_push(ir, id, instr.b(), tmp_index).is_none() {
            return false;
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Bool,
        });
    } else {
        let Some(value) = text_value_from_reg(
            ir,
            instr.b(),
            facts.register_kind_before(pc, instr.b()),
            static_regs,
            tmp_index,
        ) else {
            return false;
        };
        if matches!(
            value,
            NativeStraightlineValue::Text(_) | NativeStraightlineValue::StringPtr(_)
        ) && facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr)
        {
            let pushed = static_regs
                .get(instr.b() as usize)
                .and_then(Clone::clone)
                .and_then(|value| dynamic_ptr_value_expr(&value))
                .and_then(|value| emit_dynamic_ptr_list_push_value(ir, id, &value, tmp_index))
                .or_else(|| {
                    emit_nested_const_list_string_field_for_push(
                        ir,
                        extra_globals,
                        code,
                        int_consts,
                        heap_values,
                        pc,
                        instr.b(),
                        tmp_index,
                    )
                    .and_then(|()| emit_dynamic_ptr_list_push(ir, id, instr.b(), tmp_index))
                })
                .or_else(|| emit_dynamic_ptr_list_push(ir, id, instr.b(), tmp_index));
            if pushed.is_none() {
                return false;
            }
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id,
                element: NativeListElementKind::StrPtr,
            });
            emit_branch_to_next(ir, pc, code.len());
            return true;
        }
        if emit_dynamic_text_list_push(ir, id, value, tmp_index).is_none() {
            if facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr) {
                emit_dynamic_text_list_push_len(ir, id, "1", tmp_index);
            } else {
                return false;
            }
        }
        static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Text,
        });
    }
    emit_branch_to_next(ir, pc, code.len());
    true
}

fn dynamic_pair_field_kind(value: &NativeStraightlineValue) -> Option<NativeListElementKind> {
    match value {
        NativeStraightlineValue::I64(_) => Some(NativeListElementKind::I64),
        NativeStraightlineValue::Bool(_) => Some(NativeListElementKind::Bool),
        NativeStraightlineValue::F64(_) => Some(NativeListElementKind::F64),
        NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_) => {
            Some(NativeListElementKind::StrPtr)
        }
        _ => None,
    }
}

fn dynamic_pair_fields(
    value: Option<NativeStraightlineValue>,
) -> Option<(NativeStraightlineValue, NativeStraightlineValue)> {
    match value? {
        NativeStraightlineValue::ArgList { elements } => {
            let [first, second] = elements.as_slice() else {
                return None;
            };
            Some((first.clone(), second.clone()))
        }
        NativeStraightlineValue::List { elements, .. } => {
            let [first, second] = elements.as_slice() else {
                return None;
            };
            Some((native_pair_const_field(first)?, native_pair_const_field(second)?))
        }
        _ => None,
    }
}

fn native_pair_const_field(value: &ConstRuntimeValue32Data) -> Option<NativeStraightlineValue> {
    match value {
        ConstRuntimeValue32Data::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
        ConstRuntimeValue32Data::Bool(value) => Some(NativeStraightlineValue::Bool(if *value {
            "1".to_string()
        } else {
            "0".to_string()
        })),
        ConstRuntimeValue32Data::Float(value) => Some(NativeStraightlineValue::F64(value.to_string())),
        _ => None,
    }
}

fn dynamic_ptr_value_expr(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::String { symbol, .. } | NativeStraightlineValue::StringPtr(symbol) => {
            Some(symbol.clone())
        }
        _ => None,
    }
}

fn emit_nested_const_list_string_field_for_push(
    ir: &mut String,
    extra_globals: &mut String,
    code: &[Instr32],
    int_consts: &[i64],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    value_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let inner = previous_writer(code, pc, value_reg)?;
    if inner.opcode() != Opcode32::GetIndex {
        return None;
    }
    let NativeStraightlineValue::I64(field) = local_static_i64_before(code, int_consts, pc, inner.c())? else {
        return None;
    };
    let field = field.parse::<usize>().ok()?;
    let outer = previous_writer(code, pc, inner.b())?;
    if outer.opcode() != Opcode32::GetIndex {
        return None;
    }
    let NativeStraightlineValue::List { elements, .. } =
        local_static_container_before(code, heap_values, pc, outer.b())?
    else {
        return None;
    };
    let outer_index = next_tmp(tmp_index);
    ir.push_str(&format!("  {outer_index} = load i64, ptr %r{}.slot\n", outer.c()));
    let value = emit_const_list_element_index(
        ir,
        extra_globals,
        &elements,
        &outer_index,
        field,
        value_reg,
        pc,
        tmp_index,
    )?;
    matches!(value, NativeStraightlineValue::StringPtr(_)).then_some(())
}

fn previous_writer(code: &[Instr32], pc: usize, reg: u8) -> Option<Instr32> {
    for prev_pc in (pc.saturating_sub(64)..pc).rev() {
        let prev = *code.get(prev_pc)?;
        if prev.a() == reg {
            return Some(prev);
        }
    }
    None
}
