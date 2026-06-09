use crate::{
    llvm::{
        const_display::llvm_string_constant,
        dynamic_containers::{
            emit_dynamic_f64_list_concat, emit_dynamic_f64_list_contains, emit_dynamic_f64_list_index_of,
            emit_dynamic_f64_list_insert, emit_dynamic_f64_list_pop, emit_dynamic_f64_list_push_new,
            emit_dynamic_f64_list_remove_at, emit_dynamic_f64_list_reverse, emit_dynamic_f64_list_set_new,
            emit_dynamic_f64_list_slice, emit_dynamic_f64_list_slice_range, emit_dynamic_f64_list_sort,
            emit_dynamic_f64_list_take, emit_dynamic_f64_list_unique, emit_dynamic_i64_list_contains,
            emit_dynamic_i64_list_index_of, emit_dynamic_i64_list_pop, emit_dynamic_i64_list_reverse,
            emit_dynamic_int_list_concat, emit_dynamic_int_list_slice, emit_dynamic_int_list_take,
            emit_dynamic_ptr_list_concat, emit_dynamic_ptr_list_contains, emit_dynamic_ptr_list_index_of,
            emit_dynamic_ptr_list_insert, emit_dynamic_ptr_list_pop, emit_dynamic_ptr_list_push_new,
            emit_dynamic_ptr_list_remove_at, emit_dynamic_ptr_list_reverse, emit_dynamic_ptr_list_set_new,
            emit_dynamic_ptr_list_slice, emit_dynamic_ptr_list_slice_range, emit_dynamic_ptr_list_sort,
            emit_dynamic_ptr_list_take,
        },
        ir_text::{emit_branch_to_next, llvm_float_literal, next_tmp},
        scalar::{
            contains::static_int_list_zip_method,
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{NativeBuiltin, NativeListElementKind, NativeStraightlineValue},
    },
    vm::{ConstHeapValueData, ConstRuntimeValueData, Instr, Opcode},
};

pub(super) fn emit_dynamic_string_list_method_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let [
        NativeStraightlineValue::DynamicList { id, element },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    if !matches!(element, NativeListElementKind::StrPtr | NativeListElementKind::Text) {
        return None;
    }
    match method.as_str() {
        "take" => {
            let count_reg = dynamic_list_method_i64_arg_reg(code, pc, instr)?;
            emit_dynamic_ptr_list_take(ir, *id, pc, count_reg, tmp_index)?;
        }
        "skip" => {
            let start_reg = dynamic_list_method_i64_arg_reg(code, pc, instr)?;
            emit_dynamic_ptr_list_slice(ir, *id, pc, start_reg, tmp_index)?;
        }
        "concat" | "chain" => {
            let NativeStraightlineValue::ArgList { elements } = method_args else {
                return None;
            };
            let Some(NativeStraightlineValue::DynamicList {
                id: rhs_id,
                element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
            }) = elements.first()
            else {
                return None;
            };
            emit_dynamic_ptr_list_concat(ir, *id, *rhs_id, pc, tmp_index)?;
        }
        "unique" => {
            emit_dynamic_ptr_list_slice(ir, *id, pc, instr.a(), tmp_index)?;
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::StrPtr,
    });
    Some(())
}

pub(super) fn emit_dynamic_list_method_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_list_method_call(ir, static_regs, code, instr, pc, args, tmp_index)
        .or_else(|| emit_dynamic_i64_list_method_call(ir, static_regs, code, instr, pc, args, tmp_index))
        .or_else(|| emit_dynamic_f64_list_method_call(ir, static_regs, code, instr, pc, args, tmp_index))
        .or_else(|| emit_static_i64_list_method_call(ir, static_regs, instr, pc, args, tmp_index))
}

pub(super) fn emit_dynamic_list_method_call_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> bool {
    if emit_dynamic_list_method_call(ir, static_regs, code, instr, pc, args, tmp_index).is_none() {
        return false;
    }
    emit_branch_to_next(ir, pc, code.len());
    true
}

pub(super) fn emit_static_i64_list_zip_arglist_call(
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    instr: Instr,
    args: &[NativeStraightlineValue],
) -> bool {
    let [
        target,
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return false;
    };
    if method != "zip" {
        return false;
    }
    let elements = match method_args {
        NativeStraightlineValue::List { elements, .. } => elements,
        NativeStraightlineValue::ArgList { elements } => {
            let [NativeStraightlineValue::List { elements, .. }] = elements.as_slice() else {
                return false;
            };
            elements
        }
        _ => return false,
    };
    let Some(value) = static_int_list_zip_method(code, int_consts, strings, heap_values, target.clone(), elements)
    else {
        return false;
    };
    static_regs[instr.a() as usize] = Some(value);
    true
}

pub(super) fn emit_dynamic_f64_list_builtin_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::F64,
    } = args.first()?
    else {
        return None;
    };
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => {
            if args.len() != 2 {
                return None;
            }
            let needle = f64_list_f64_arg(&args[1])?;
            if builtin == NativeBuiltin::ListContains {
                emit_dynamic_f64_list_contains(ir, *id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_f64_list_index_of(ir, *id, instr.a(), &needle, tmp_index)?;
            }
        }
        NativeBuiltin::ListPush => {
            if args.len() != 2 {
                return None;
            }
            let value = f64_list_f64_arg(&args[1])?;
            emit_dynamic_f64_list_push_new(ir, *id, pc, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
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
            emit_dynamic_f64_list_slice_range(ir, *id, pc, &start, end.as_deref(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
            return Some(());
        }
        NativeBuiltin::ListInsert => {
            if args.len() != 3 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let value = f64_list_f64_arg(&args[2])?;
            emit_dynamic_f64_list_insert(ir, *id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
            return Some(());
        }
        NativeBuiltin::ListRemoveAt => {
            if args.len() != 2 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let removed = emit_dynamic_f64_list_remove_at(ir, *id, pc, &index, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::F64,
                    },
                    NativeStraightlineValue::F64(removed),
                ],
            });
            return Some(());
        }
        NativeBuiltin::ListSet => {
            if args.len() != 3 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let value = f64_list_f64_arg(&args[2])?;
            let old = emit_dynamic_f64_list_set_new(ir, *id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::F64,
                    },
                    NativeStraightlineValue::F64(old),
                ],
            });
            return Some(());
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = None;
    Some(())
}

pub(super) fn emit_dynamic_f64_list_builtin_call_from_regs(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
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
    let Some(NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::F64,
    }) = static_regs.get(list_reg as usize).cloned().flatten()
    else {
        return None;
    };
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => {
            let needle_reg = instr.b().checked_add(2)?;
            let needle = if let Some(NativeStraightlineValue::F64(value)) =
                static_regs.get(needle_reg as usize).cloned().flatten()
            {
                value
            } else if facts.register_kind_before(pc, needle_reg) == Some(NativeScalarKind::F64) {
                let value = next_tmp(tmp_index);
                ir.push_str(&format!("  {value} = load double, ptr %r{needle_reg}.slot\n"));
                value
            } else {
                return None;
            };
            if builtin == NativeBuiltin::ListContains {
                emit_dynamic_f64_list_contains(ir, id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_f64_list_index_of(ir, id, instr.a(), &needle, tmp_index)?;
            }
        }
        NativeBuiltin::ListPop => {
            emit_dynamic_f64_list_pop(ir, id, instr.a(), tmp_index)?;
        }
        NativeBuiltin::ListReverse => {
            emit_dynamic_f64_list_reverse(ir, id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
            return Some(());
        }
        NativeBuiltin::ListSort => {
            emit_dynamic_f64_list_sort(ir, id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
            return Some(());
        }
        NativeBuiltin::ListPush => {
            let value_reg = instr.b().checked_add(2)?;
            let value = f64_list_arg_from_reg(ir, static_regs, facts, pc, value_reg, tmp_index)?;
            emit_dynamic_f64_list_push_new(ir, id, pc, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
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
            emit_dynamic_f64_list_slice_range(ir, id, pc, &start, end.as_deref(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
            return Some(());
        }
        NativeBuiltin::ListInsert => {
            let index_reg = instr.b().checked_add(2)?;
            let value_reg = instr.b().checked_add(3)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let value = f64_list_arg_from_reg(ir, static_regs, facts, pc, value_reg, tmp_index)?;
            emit_dynamic_f64_list_insert(ir, id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            });
            return Some(());
        }
        NativeBuiltin::ListRemoveAt => {
            let index_reg = instr.b().checked_add(2)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let removed = emit_dynamic_f64_list_remove_at(ir, id, pc, &index, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::F64,
                    },
                    NativeStraightlineValue::F64(removed),
                ],
            });
            return Some(());
        }
        NativeBuiltin::ListSet => {
            let index_reg = instr.b().checked_add(2)?;
            let value_reg = instr.b().checked_add(3)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let value = f64_list_arg_from_reg(ir, static_regs, facts, pc, value_reg, tmp_index)?;
            let old = emit_dynamic_f64_list_set_new(ir, id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::F64,
                    },
                    NativeStraightlineValue::F64(old),
                ],
            });
            return Some(());
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = None;
    Some(())
}

pub(super) fn emit_dynamic_ptr_list_builtin_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
    } = args.first()?
    else {
        return None;
    };
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => {
            if args.len() != 2 {
                return None;
            }
            let needle = ptr_list_needle(extra_globals, pc, &args[1], tmp_index)?;
            if builtin == NativeBuiltin::ListContains {
                emit_dynamic_ptr_list_contains(ir, *id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_ptr_list_index_of(ir, *id, instr.a(), &needle, tmp_index)?;
            }
        }
        NativeBuiltin::ListPop => {
            if args.len() != 1 {
                return None;
            }
            let result = emit_dynamic_ptr_list_pop(ir, *id, instr.a(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(result));
            return Some(());
        }
        NativeBuiltin::ListReverse => {
            if args.len() != 1 {
                return None;
            }
            emit_dynamic_ptr_list_reverse(ir, *id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListSort => {
            if args.len() != 1 {
                return None;
            }
            emit_dynamic_ptr_list_sort(ir, *id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListPush => {
            if args.len() != 2 {
                return None;
            }
            let value = ptr_list_needle(extra_globals, pc, &args[1], tmp_index)?;
            emit_dynamic_ptr_list_push_new(ir, *id, pc, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
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
            emit_dynamic_ptr_list_slice_range(ir, *id, pc, &start, end.as_deref(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListInsert => {
            if args.len() != 3 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let value = ptr_list_needle(extra_globals, pc, &args[2], tmp_index)?;
            emit_dynamic_ptr_list_insert(ir, *id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListRemoveAt => {
            if args.len() != 2 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let removed = emit_dynamic_ptr_list_remove_at(ir, *id, pc, &index, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::StrPtr,
                    },
                    NativeStraightlineValue::StringPtr(removed),
                ],
            });
            return Some(());
        }
        NativeBuiltin::ListSet => {
            if args.len() != 3 {
                return None;
            }
            let index = ptr_list_i64_arg(&args[1])?;
            let value = ptr_list_needle(extra_globals, pc, &args[2], tmp_index)?;
            let old = emit_dynamic_ptr_list_set_new(ir, *id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::StrPtr,
                    },
                    NativeStraightlineValue::StringPtr(old),
                ],
            });
            return Some(());
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = None;
    Some(())
}

pub(super) fn emit_dynamic_ptr_list_builtin_call_from_regs(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
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
    let Some(NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
    }) = static_regs.get(list_reg as usize).cloned().flatten()
    else {
        return None;
    };
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => {
            let needle_reg = instr.b().checked_add(2)?;
            let needle = if let Some(value) = static_regs.get(needle_reg as usize).cloned().flatten() {
                ptr_list_needle(extra_globals, pc, &value, tmp_index)?
            } else if matches!(
                facts.register_kind_before(pc, needle_reg),
                Some(NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr)
            ) {
                let value = next_tmp(tmp_index);
                ir.push_str(&format!("  {value} = load ptr, ptr %r{needle_reg}.slot\n"));
                value
            } else {
                return None;
            };
            if builtin == NativeBuiltin::ListContains {
                emit_dynamic_ptr_list_contains(ir, id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_ptr_list_index_of(ir, id, instr.a(), &needle, tmp_index)?;
            }
        }
        NativeBuiltin::ListPop => {
            let result = emit_dynamic_ptr_list_pop(ir, id, instr.a(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(result));
            return Some(());
        }
        NativeBuiltin::ListReverse => {
            emit_dynamic_ptr_list_reverse(ir, id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListSort => {
            emit_dynamic_ptr_list_sort(ir, id, pc, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListPush => {
            let value_reg = instr.b().checked_add(2)?;
            let value = ptr_list_arg_from_reg(ir, extra_globals, static_regs, facts, pc, value_reg, tmp_index)?;
            emit_dynamic_ptr_list_push_new(ir, id, pc, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
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
            emit_dynamic_ptr_list_slice_range(ir, id, pc, &start, end.as_deref(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListInsert => {
            let index_reg = instr.b().checked_add(2)?;
            let value_reg = instr.b().checked_add(3)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let value = ptr_list_arg_from_reg(ir, extra_globals, static_regs, facts, pc, value_reg, tmp_index)?;
            emit_dynamic_ptr_list_insert(ir, id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            });
            return Some(());
        }
        NativeBuiltin::ListRemoveAt => {
            let index_reg = instr.b().checked_add(2)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let removed = emit_dynamic_ptr_list_remove_at(ir, id, pc, &index, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::StrPtr,
                    },
                    NativeStraightlineValue::StringPtr(removed),
                ],
            });
            return Some(());
        }
        NativeBuiltin::ListSet => {
            let index_reg = instr.b().checked_add(2)?;
            let value_reg = instr.b().checked_add(3)?;
            let index = ptr_list_i64_arg_from_reg(ir, static_regs, facts, pc, index_reg, tmp_index)?;
            let value = ptr_list_arg_from_reg(ir, extra_globals, static_regs, facts, pc, value_reg, tmp_index)?;
            let old = emit_dynamic_ptr_list_set_new(ir, id, pc, &index, &value, tmp_index)?;
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::StrPtr,
                    },
                    NativeStraightlineValue::StringPtr(old),
                ],
            });
            return Some(());
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = None;
    Some(())
}

fn ptr_list_arg_from_reg(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    facts: &NativeScalarFacts,
    pc: usize,
    reg: u8,
    tmp_index: &mut usize,
) -> Option<String> {
    if let Some(value) = static_regs.get(reg as usize).cloned().flatten() {
        return ptr_list_needle(extra_globals, pc, &value, tmp_index);
    }
    if matches!(
        facts.register_kind_before(pc, reg),
        Some(NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr)
    ) {
        let value = next_tmp(tmp_index);
        ir.push_str(&format!("  {value} = load ptr, ptr %r{reg}.slot\n"));
        return Some(value);
    }
    None
}

pub(super) fn ptr_list_i64_arg_from_reg(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    facts: &NativeScalarFacts,
    pc: usize,
    reg: u8,
    tmp_index: &mut usize,
) -> Option<String> {
    if let Some(NativeStraightlineValue::I64(value) | NativeStraightlineValue::Bool(value)) =
        static_regs.get(reg as usize).cloned().flatten()
    {
        return Some(value);
    }
    if matches!(
        facts.register_kind_before(pc, reg),
        Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64 | NativeScalarKind::Bool)
    ) {
        let value = next_tmp(tmp_index);
        ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
        return Some(value);
    }
    None
}

fn ptr_list_needle(
    extra_globals: &mut String,
    pc: usize,
    value: &NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<String> {
    match value {
        NativeStraightlineValue::StringPtr(value) => Some(value.clone()),
        NativeStraightlineValue::String { symbol, value, .. } if !symbol.is_empty() => Some(symbol.clone()),
        NativeStraightlineValue::String { value, .. } => {
            let symbol = format!("@lk_ptr_list_needle_{pc}_{}", *tmp_index);
            *tmp_index += 1;
            extra_globals.push_str(&llvm_string_constant(&symbol, value));
            Some(symbol)
        }
        NativeStraightlineValue::Text(parts) if parts.len() == 1 => match &parts[0] {
            crate::llvm::straightline_value::NativeTextPart::String { symbol, value } if !symbol.is_empty() => {
                Some(symbol.clone())
            }
            crate::llvm::straightline_value::NativeTextPart::String { value, .. } => {
                let symbol = format!("@lk_ptr_list_needle_{pc}_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, value));
                Some(symbol)
            }
            crate::llvm::straightline_value::NativeTextPart::StrPtr(value) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn ptr_list_i64_arg(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::I64(value) | NativeStraightlineValue::Bool(value) => Some(value.clone()),
        _ => None,
    }
}

fn f64_list_f64_arg(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::F64(value) => Some(value.clone()),
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            value.parse::<i64>().ok().map(|value| llvm_float_literal(value as f64))
        }
        _ => None,
    }
}

fn f64_list_arg_from_reg(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    facts: &NativeScalarFacts,
    pc: usize,
    reg: u8,
    tmp_index: &mut usize,
) -> Option<String> {
    if let Some(value) = static_regs.get(reg as usize).cloned().flatten() {
        return f64_list_f64_arg(&value);
    }
    match facts.register_kind_before(pc, reg) {
        Some(NativeScalarKind::F64) => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load double, ptr %r{reg}.slot\n"));
            Some(value)
        }
        Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64) => {
            let int_value = next_tmp(tmp_index);
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {int_value} = load i64, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  {value} = sitofp i64 {int_value} to double\n"));
            Some(value)
        }
        _ => None,
    }
}

pub(super) fn function_has_list_return_shape(function: &crate::vm::FunctionData) -> bool {
    super::super::list_shape::function_returns_pushed_list(function)
}

fn dynamic_list_method_i64_arg_reg(code: &[Instr], pc: usize, instr: Instr) -> Option<u8> {
    if instr.c() != 3 {
        return None;
    }
    let arg_list_reg = instr.a().checked_add(3)?;
    single_arg_list_source_reg_before(code, pc, arg_list_reg).or(Some(arg_list_reg))
}

fn f64_list_method_f64_arg(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    method_args: &NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<String> {
    if let NativeStraightlineValue::ArgList { elements } = method_args
        && let Some(value) = elements.first().and_then(f64_list_f64_arg)
    {
        return Some(value);
    }
    if let NativeStraightlineValue::List { elements, .. } = method_args
        && let Some(value) = elements.first()
    {
        return match value {
            ConstRuntimeValueData::Float(value) => Some(llvm_float_literal(*value)),
            ConstRuntimeValueData::Int(value) => Some(llvm_float_literal(*value as f64)),
            _ => None,
        };
    }
    if instr.c() != 3 {
        return None;
    }
    let arg_list_reg = instr.a().checked_add(3)?;
    let source_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg).unwrap_or(arg_list_reg);
    if let Some(value) = static_regs
        .get(source_reg as usize)
        .and_then(|value| value.as_ref())
        .and_then(f64_list_f64_arg)
    {
        return Some(value);
    }
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load double, ptr %r{source_reg}.slot\n"));
    Some(value)
}

pub(super) fn emit_dynamic_i64_list_method_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let [
        NativeStraightlineValue::DynamicList { id, element },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    if !matches!(element, NativeListElementKind::I64 | NativeListElementKind::Bool) {
        return None;
    }
    match method.as_str() {
        "take" => {
            let count_reg = dynamic_list_method_i64_arg_reg(code, pc, instr)?;
            emit_dynamic_int_list_take(ir, *id, pc, count_reg, tmp_index)?;
        }
        "skip" => {
            let start_reg = dynamic_list_method_i64_arg_reg(code, pc, instr)?;
            emit_dynamic_int_list_slice(ir, *id, pc, start_reg, tmp_index)?;
        }
        "concat" | "chain" => {
            match method_args {
                NativeStraightlineValue::ArgList { elements } => match elements.first()? {
                    NativeStraightlineValue::DynamicList {
                        id: rhs_id,
                        element: rhs_element,
                    } if rhs_element == element => emit_dynamic_int_list_concat(ir, *id, *rhs_id, pc, tmp_index)?,
                    NativeStraightlineValue::List { elements, .. }
                        if list_elements_match_i64_storage(elements, *element) =>
                    {
                        emit_dynamic_i64_list_concat_static_rhs(ir, *id, pc, elements, *element, tmp_index)?;
                    }
                    _ => return None,
                },
                NativeStraightlineValue::DynamicList {
                    id: rhs_id,
                    element: rhs_element,
                } if rhs_element == element => {
                    emit_dynamic_int_list_concat(ir, *id, *rhs_id, pc, tmp_index)?;
                }
                NativeStraightlineValue::List { elements, .. }
                    if list_elements_match_i64_storage(elements, *element) =>
                {
                    emit_dynamic_i64_list_concat_static_rhs(ir, *id, pc, elements, *element, tmp_index)?;
                }
                _ => return None,
            };
        }
        "contains" | "index_of" => {
            let needle = i64_list_method_i64_arg(ir, static_regs, code, pc, instr, method_args, tmp_index)?;
            if method == "contains" {
                emit_dynamic_i64_list_contains(ir, *id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_i64_list_index_of(ir, *id, instr.a(), &needle, tmp_index)?;
            }
            static_regs[instr.a() as usize] = None;
            return Some(());
        }
        "pop" => {
            let result = emit_dynamic_i64_list_pop(ir, *id, instr.a(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(match element {
                NativeListElementKind::Bool => NativeStraightlineValue::Bool(result),
                _ => NativeStraightlineValue::I64(result),
            });
            return Some(());
        }
        "reverse" => {
            emit_dynamic_i64_list_reverse(ir, *id, pc, tmp_index)?;
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: *element,
    });
    Some(())
}

fn i64_list_method_i64_arg(
    ir: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    method_args: &NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<String> {
    if let NativeStraightlineValue::ArgList { elements } = method_args
        && let Some(value) = elements.first().and_then(ptr_list_i64_arg)
    {
        return Some(value);
    }
    if let NativeStraightlineValue::List { elements, .. } = method_args
        && let Some(value) = elements.first()
    {
        return match value {
            ConstRuntimeValueData::Int(value) => Some(value.to_string()),
            ConstRuntimeValueData::Bool(value) => Some(if *value { "1" } else { "0" }.to_string()),
            _ => None,
        };
    }
    if instr.c() != 3 {
        return None;
    }
    let arg_list_reg = instr.a().checked_add(3)?;
    let source_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg).unwrap_or(arg_list_reg);
    if let Some(value) = static_regs
        .get(source_reg as usize)
        .and_then(|value| value.as_ref())
        .and_then(ptr_list_i64_arg)
    {
        return Some(value);
    }
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load i64, ptr %r{source_reg}.slot\n"));
    Some(value)
}

pub(super) fn emit_dynamic_f64_list_method_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let [
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::F64,
        },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    match method.as_str() {
        "take" => {
            let count_reg = dynamic_list_method_i64_arg_reg(code, pc, instr)?;
            emit_dynamic_f64_list_take(ir, *id, pc, count_reg, tmp_index)?;
        }
        "skip" => {
            let start_reg = dynamic_list_method_i64_arg_reg(code, pc, instr)?;
            emit_dynamic_f64_list_slice(ir, *id, pc, start_reg, tmp_index)?;
        }
        "unique" => {
            emit_dynamic_f64_list_unique(ir, *id, pc, tmp_index)?;
        }
        "sort" => {
            emit_dynamic_f64_list_sort(ir, *id, pc, tmp_index)?;
        }
        "contains" | "index_of" => {
            let needle = f64_list_method_f64_arg(ir, static_regs, code, pc, instr, method_args, tmp_index)?;
            if method == "contains" {
                emit_dynamic_f64_list_contains(ir, *id, instr.a(), &needle, tmp_index)?;
            } else {
                emit_dynamic_f64_list_index_of(ir, *id, instr.a(), &needle, tmp_index)?;
            }
            static_regs[instr.a() as usize] = None;
            return Some(());
        }
        "concat" | "chain" => match method_args {
            NativeStraightlineValue::ArgList { elements } => match elements.first()? {
                NativeStraightlineValue::DynamicList {
                    id: rhs_id,
                    element: NativeListElementKind::F64,
                } => emit_dynamic_f64_list_concat(ir, *id, *rhs_id, pc, tmp_index)?,
                NativeStraightlineValue::List { elements, .. } => {
                    emit_dynamic_f64_list_concat_static_rhs(ir, *id, pc, elements, tmp_index)?;
                }
                _ => return None,
            },
            NativeStraightlineValue::DynamicList {
                id: rhs_id,
                element: NativeListElementKind::F64,
            } => emit_dynamic_f64_list_concat(ir, *id, *rhs_id, pc, tmp_index)?,
            NativeStraightlineValue::List { elements, .. } => {
                emit_dynamic_f64_list_concat_static_rhs(ir, *id, pc, elements, tmp_index)?;
            }
            _ => return None,
        },
        _ => return None,
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::F64,
    });
    Some(())
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

fn emit_dynamic_i64_list_concat_static_rhs(
    ir: &mut String,
    lhs_id: usize,
    dst_id: usize,
    elements: &[ConstRuntimeValueData],
    element_kind: NativeListElementKind,
    tmp_index: &mut usize,
) -> Option<()> {
    let rhs_values = format!("%list{dst_id}.static.rhs.value.slots");
    let rhs_len_slot = format!("%list{dst_id}.static.rhs.len.slot");
    ir.push_str(&format!("  {rhs_len_slot} = call ptr @malloc(i64 8)\n"));
    ir.push_str(&format!("  {rhs_values} = call ptr @malloc(i64 32768)\n"));
    ir.push_str(&format!("  store i64 {}, ptr {rhs_len_slot}\n", elements.len()));
    for (index, element) in elements.iter().enumerate() {
        let value = match (element_kind, element) {
            (NativeListElementKind::I64, ConstRuntimeValueData::Int(value)) => value.to_string(),
            (NativeListElementKind::Bool, ConstRuntimeValueData::Bool(value)) => {
                if *value { "1" } else { "0" }.to_string()
            }
            _ => return None,
        };
        let slot = next_tmp(tmp_index);
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x i64], ptr {rhs_values}, i64 0, i64 {index}\n"
        ));
        ir.push_str(&format!("  store i64 {value}, ptr {slot}\n"));
    }
    let lhs_len = next_tmp(tmp_index);
    let lhs_base = next_tmp(tmp_index);
    let rhs_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_len} = load i64, ptr %list{lhs_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {lhs_base} = getelementptr [4096 x i64], ptr %list{lhs_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x i64], ptr {rhs_values}, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_i64_list(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n",
        elements.len()
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

fn emit_dynamic_f64_list_concat_static_rhs(
    ir: &mut String,
    lhs_id: usize,
    dst_id: usize,
    elements: &[ConstRuntimeValueData],
    tmp_index: &mut usize,
) -> Option<()> {
    let rhs_values = format!("%list{dst_id}.static.rhs.f64.slots");
    let rhs_len_slot = format!("%list{dst_id}.static.rhs.len.slot");
    ir.push_str(&format!("  {rhs_len_slot} = call ptr @malloc(i64 8)\n"));
    ir.push_str(&format!("  {rhs_values} = call ptr @malloc(i64 32768)\n"));
    ir.push_str(&format!("  store i64 {}, ptr {rhs_len_slot}\n", elements.len()));
    for (index, element) in elements.iter().enumerate() {
        let value = match element {
            ConstRuntimeValueData::Float(value) => llvm_float_literal(*value),
            ConstRuntimeValueData::Int(value) => llvm_float_literal(*value as f64),
            _ => return None,
        };
        let slot = next_tmp(tmp_index);
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x double], ptr {rhs_values}, i64 0, i64 {index}\n"
        ));
        ir.push_str(&format!("  store double {value}, ptr {slot}\n"));
    }
    let lhs_len = next_tmp(tmp_index);
    let lhs_base = next_tmp(tmp_index);
    let rhs_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_len} = load i64, ptr %list{lhs_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {lhs_base} = getelementptr [4096 x double], ptr %list{lhs_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x double], ptr {rhs_values}, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_f64_list(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n",
        elements.len()
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

fn emit_static_i64_list_method_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let [
        NativeStraightlineValue::List { elements, .. },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    if !matches!(method.as_str(), "concat" | "chain") || !list_elements_are_i64(elements) {
        return None;
    }
    match first_list_method_arg(method_args)? {
        NativeStraightlineValue::DynamicList {
            id: rhs_id,
            element: NativeListElementKind::I64,
        } => emit_static_i64_list_concat_dynamic_rhs(ir, elements, *rhs_id, pc, tmp_index)?,
        NativeStraightlineValue::List { elements: rhs, .. } if list_elements_are_i64(rhs) => {
            emit_static_i64_list_concat_static_rhs(ir, elements, rhs, pc, tmp_index)?;
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::I64,
    });
    Some(())
}

fn first_list_method_arg(value: &NativeStraightlineValue) -> Option<&NativeStraightlineValue> {
    match value {
        NativeStraightlineValue::ArgList { elements } => elements.first(),
        value @ (NativeStraightlineValue::DynamicList { .. } | NativeStraightlineValue::List { .. }) => Some(value),
        _ => None,
    }
}

fn list_elements_are_i64(elements: &[ConstRuntimeValueData]) -> bool {
    elements
        .iter()
        .all(|value| matches!(value, ConstRuntimeValueData::Int(_)))
}

fn list_elements_match_i64_storage(elements: &[ConstRuntimeValueData], element: NativeListElementKind) -> bool {
    match element {
        NativeListElementKind::I64 => elements
            .iter()
            .all(|value| matches!(value, ConstRuntimeValueData::Int(_))),
        NativeListElementKind::Bool => elements
            .iter()
            .all(|value| matches!(value, ConstRuntimeValueData::Bool(_))),
        _ => false,
    }
}

fn emit_static_i64_list_concat_dynamic_rhs(
    ir: &mut String,
    lhs: &[ConstRuntimeValueData],
    rhs_id: usize,
    dst_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let lhs_values = emit_static_i64_list_slots(ir, "lhs", dst_id, lhs, tmp_index)?;
    let rhs_len = next_tmp(tmp_index);
    let rhs_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {rhs_len} = load i64, ptr %list{rhs_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x i64], ptr %list{rhs_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_i64_list(ptr {lhs_values}, i64 {}, ptr {rhs_base}, i64 {rhs_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n",
        lhs.len()
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

fn emit_static_i64_list_concat_static_rhs(
    ir: &mut String,
    lhs: &[ConstRuntimeValueData],
    rhs: &[ConstRuntimeValueData],
    dst_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let lhs_values = emit_static_i64_list_slots(ir, "lhs", dst_id, lhs, tmp_index)?;
    let rhs_values = emit_static_i64_list_slots(ir, "rhs", dst_id, rhs, tmp_index)?;
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_i64_list(ptr {lhs_values}, i64 {}, ptr {rhs_values}, i64 {}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n",
        lhs.len(),
        rhs.len()
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

fn emit_static_i64_list_slots(
    ir: &mut String,
    name: &str,
    dst_id: usize,
    elements: &[ConstRuntimeValueData],
    tmp_index: &mut usize,
) -> Option<String> {
    let slots = format!("%list{dst_id}.static.{name}.slots");
    ir.push_str(&format!("  {slots} = call ptr @malloc(i64 32768)\n"));
    for (index, element) in elements.iter().enumerate() {
        let ConstRuntimeValueData::Int(value) = element else {
            return None;
        };
        let slot = next_tmp(tmp_index);
        ir.push_str(&format!(
            "  {slot} = getelementptr [4096 x i64], ptr {slots}, i64 0, i64 {index}\n"
        ));
        ir.push_str(&format!("  store i64 {value}, ptr {slot}\n"));
    }
    let base = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {base} = getelementptr [4096 x i64], ptr {slots}, i64 0, i64 0\n"
    ));
    Some(base)
}
