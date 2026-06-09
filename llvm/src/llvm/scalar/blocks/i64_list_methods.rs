use crate::{
    llvm::{
        dynamic_containers::{
            emit_dynamic_i64_list_contains, emit_dynamic_i64_list_index_of, emit_dynamic_i64_list_insert,
            emit_dynamic_i64_list_pop, emit_dynamic_i64_list_push_new, emit_dynamic_i64_list_remove_at,
            emit_dynamic_i64_list_reverse, emit_dynamic_i64_list_set_new, emit_dynamic_i64_list_slice_range,
            emit_dynamic_i64_list_sort,
        },
        scalar::{block_helpers::scalar_arg_value, facts::NativeScalarFacts},
        straightline_value::{NativeBuiltin, NativeListElementKind, NativeStraightlineValue},
    },
    vm::{ConstHeapValueData, Instr, Opcode},
};

use super::list_methods::{ptr_list_i64_arg, ptr_list_i64_arg_from_reg};

pub(super) fn emit_dynamic_i64_list_builtin_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    instr: Instr,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeStraightlineValue::DynamicList { id, element } = args.first()? else {
        return None;
    };
    if !dynamic_i64_storage_list_builtin_supported(code, heap_values, *id, *element) {
        return None;
    }
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => {
            if args.len() != 2 {
                return None;
            }
            let needle = i64_storage_list_arg(&args[1], *element)?;
            if builtin == NativeBuiltin::ListContains {
                emit_dynamic_i64_list_contains(ir, *id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_i64_list_index_of(ir, *id, instr.a(), &needle, tmp_index)?;
            }
        }
        NativeBuiltin::ListPop => {
            if args.len() != 1 {
                return None;
            }
            let result = emit_dynamic_i64_list_pop(ir, *id, instr.a(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(list_pop_result(*element, result));
            return Some(());
        }
        NativeBuiltin::ListReverse => {
            if args.len() != 1 {
                return None;
            }
            emit_dynamic_i64_list_reverse(ir, *id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: *element,
            });
            return Some(());
        }
        NativeBuiltin::ListSort => {
            if args.len() != 1 {
                return None;
            }
            emit_dynamic_i64_list_sort(ir, *id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, *element));
            return Some(());
        }
        NativeBuiltin::ListPush => {
            if args.len() != 2 {
                return None;
            }
            let value = i64_storage_list_arg(&args[1], *element)?;
            emit_dynamic_i64_list_push_new(ir, *id, pc, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, *element));
            return Some(());
        }
        NativeBuiltin::ListSlice => {
            if args.len() != 2 && args.len() != 3 {
                return None;
            }
            let start = ptr_list_i64_arg(&args[1])?;
            let end = if args.len() == 3 {
                Some(ptr_list_i64_arg(&args[2])?)
            } else {
                None
            };
            emit_dynamic_i64_list_slice_range(ir, *id, pc, &start, end.as_deref(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, *element));
            return Some(());
        }
        NativeBuiltin::ListInsert => {
            if args.len() != 3 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let value = i64_storage_list_arg(&args[2], *element)?;
            emit_dynamic_i64_list_insert(ir, *id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, *element));
            return Some(());
        }
        NativeBuiltin::ListRemoveAt => {
            if args.len() != 2 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let removed = emit_dynamic_i64_list_remove_at(ir, *id, pc, &index, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_with_old_value(pc, *element, removed));
            return Some(());
        }
        NativeBuiltin::ListSet => {
            if args.len() != 3 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let value = i64_storage_list_arg(&args[2], *element)?;
            let old = emit_dynamic_i64_list_set_new(ir, *id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_with_old_value(pc, *element, old));
            return Some(());
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = None;
    Some(())
}

pub(super) fn emit_dynamic_i64_list_builtin_call_from_regs(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    instr: Instr,
    builtin: NativeBuiltin,
    facts: &NativeScalarFacts,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let expected_arity = match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => 2,
        NativeBuiltin::ListPush | NativeBuiltin::ListRemoveAt => 2,
        NativeBuiltin::ListInsert | NativeBuiltin::ListSet => 3,
        NativeBuiltin::ListSlice if instr.c() == 2 || instr.c() == 3 => instr.c(),
        NativeBuiltin::ListPop | NativeBuiltin::ListReverse | NativeBuiltin::ListSort => 1,
        _ => return None,
    };
    if instr.c() != expected_arity {
        return None;
    }
    let list_reg = instr.b().checked_add(1)?;
    let Some(NativeStraightlineValue::DynamicList { id, element }) =
        static_regs.get(list_reg as usize).cloned().flatten()
    else {
        return None;
    };
    if !dynamic_i64_storage_list_builtin_supported(code, heap_values, id, element) {
        return None;
    }
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => {
            let needle_reg = instr.b().checked_add(2)?;
            let needle = i64_storage_list_arg_from_reg(ir, static_regs, facts, pc, needle_reg, element, tmp_index)?;
            if builtin == NativeBuiltin::ListContains {
                emit_dynamic_i64_list_contains(ir, id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_i64_list_index_of(ir, id, instr.a(), &needle, tmp_index)?;
            }
        }
        NativeBuiltin::ListPop => {
            let result = emit_dynamic_i64_list_pop(ir, id, instr.a(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(list_pop_result(element, result));
            return Some(());
        }
        NativeBuiltin::ListReverse => {
            emit_dynamic_i64_list_reverse(ir, id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, element));
            return Some(());
        }
        NativeBuiltin::ListSort => {
            emit_dynamic_i64_list_sort(ir, id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, element));
            return Some(());
        }
        NativeBuiltin::ListPush => {
            let value_reg = instr.b().checked_add(2)?;
            let value = i64_storage_list_arg_from_reg(ir, static_regs, facts, pc, value_reg, element, tmp_index)?;
            emit_dynamic_i64_list_push_new(ir, id, pc, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, element));
            return Some(());
        }
        NativeBuiltin::ListSlice => {
            let start_reg = instr.b().checked_add(2)?;
            let start = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, start_reg, tmp_index)?;
            let end = if instr.c() == 3 {
                let end_reg = instr.b().checked_add(3)?;
                Some(ptr_list_i64_arg_from_reg(
                    ir,
                    static_regs,
                    facts,
                    pc,
                    end_reg,
                    tmp_index,
                )?)
            } else {
                None
            };
            emit_dynamic_i64_list_slice_range(ir, id, pc, &start, end.as_deref(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, element));
            return Some(());
        }
        NativeBuiltin::ListInsert => {
            let index_reg = instr.b().checked_add(2)?;
            let value_reg = instr.b().checked_add(3)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let value = i64_storage_list_arg_from_reg(ir, static_regs, facts, pc, value_reg, element, tmp_index)?;
            emit_dynamic_i64_list_insert(ir, id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_value(pc, element));
            return Some(());
        }
        NativeBuiltin::ListRemoveAt => {
            let index_reg = instr.b().checked_add(2)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let removed = emit_dynamic_i64_list_remove_at(ir, id, pc, &index, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_with_old_value(pc, element, removed));
            return Some(());
        }
        NativeBuiltin::ListSet => {
            let index_reg = instr.b().checked_add(2)?;
            let value_reg = instr.b().checked_add(3)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let value = i64_storage_list_arg_from_reg(ir, static_regs, facts, pc, value_reg, element, tmp_index)?;
            let old = emit_dynamic_i64_list_set_new(ir, id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(i64_storage_list_with_old_value(pc, element, old));
            return Some(());
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = None;
    Some(())
}

fn dynamic_i64_storage_list_builtin_supported(
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    id: usize,
    element: NativeListElementKind,
) -> bool {
    if !matches!(element, NativeListElementKind::I64 | NativeListElementKind::Bool) {
        return false;
    }
    let Some(instr) = code.get(id).copied() else {
        return false;
    };
    match instr.opcode() {
        Opcode::NewList | Opcode::ListPush => true,
        Opcode::LoadHeapConst => matches!(
            heap_values.get(instr.bx() as usize),
            Some(ConstHeapValueData::List(values)) if values.is_empty()
        ),
        _ => false,
    }
}

fn i64_storage_list_arg(value: &NativeStraightlineValue, element: NativeListElementKind) -> Option<String> {
    match element {
        NativeListElementKind::Bool => match value {
            NativeStraightlineValue::Bool(value) => Some(value.clone()),
            _ => None,
        },
        NativeListElementKind::I64 => ptr_list_i64_arg(value),
        _ => None,
    }
}

fn i64_storage_list_arg_from_reg(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    facts: &NativeScalarFacts,
    pc: usize,
    reg: u8,
    element: NativeListElementKind,
    tmp_index: &mut usize,
) -> Option<String> {
    match element {
        NativeListElementKind::Bool => match scalar_arg_value(ir, "", facts, pc, static_regs, reg as usize, tmp_index)?
        {
            NativeStraightlineValue::Bool(value) => Some(value),
            _ => None,
        },
        NativeListElementKind::I64 => ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, reg, tmp_index),
        _ => None,
    }
}

fn i64_storage_list_value(id: usize, element: NativeListElementKind) -> NativeStraightlineValue {
    NativeStraightlineValue::DynamicList { id, element }
}

fn i64_storage_list_with_old_value(id: usize, element: NativeListElementKind, old: String) -> NativeStraightlineValue {
    let old = match element {
        NativeListElementKind::Bool => NativeStraightlineValue::Bool(old),
        _ => NativeStraightlineValue::I64(old),
    };
    NativeStraightlineValue::ArgList {
        elements: vec![i64_storage_list_value(id, element), old],
    }
}

fn list_pop_result(element: NativeListElementKind, result: String) -> NativeStraightlineValue {
    match element {
        NativeListElementKind::Bool => NativeStraightlineValue::Bool(result),
        _ => NativeStraightlineValue::I64(result),
    }
}
