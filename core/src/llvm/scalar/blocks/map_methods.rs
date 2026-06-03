use crate::{
    llvm::{
        dynamic_containers::{
            emit_dynamic_i64_f64_map_delete_key, emit_dynamic_i64_f64_map_get_key, emit_dynamic_i64_f64_map_set,
            emit_dynamic_i64_f64_map_values, emit_dynamic_i64_int_map_delete_key, emit_dynamic_i64_int_map_get_key,
            emit_dynamic_i64_int_map_set, emit_dynamic_i64_int_map_values, emit_dynamic_i64_map_has_key,
            emit_dynamic_i64_map_keys, emit_dynamic_i64_ptr_map_delete_key, emit_dynamic_i64_ptr_map_get_key,
            emit_dynamic_i64_ptr_map_set, emit_dynamic_i64_ptr_map_values, emit_dynamic_string_f64_map_delete,
            emit_dynamic_string_f64_map_get, emit_dynamic_string_f64_map_set, emit_dynamic_string_f64_map_values,
            emit_dynamic_string_int_map_delete, emit_dynamic_string_int_map_get, emit_dynamic_string_int_map_set,
            emit_dynamic_string_int_map_values, emit_dynamic_string_map_has, emit_dynamic_string_map_keys,
            emit_dynamic_string_ptr_map_delete, emit_dynamic_string_ptr_map_get, emit_dynamic_string_ptr_map_set,
            emit_dynamic_string_ptr_map_values,
        },
        ir_text::next_tmp,
        straightline_value::{
            NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
            native_const_runtime_value, native_static_set_index,
        },
    },
    vm::{ConstRuntimeValue32Data, Instr32},
};

pub(super) fn emit_dynamic_map_set_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    if builtin != NativeBuiltin::MapSet {
        return None;
    }
    if let [
        NativeStraightlineValue::DynamicMap {
            id,
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64 | NativeMapValueKind::F64 | NativeMapValueKind::Bool,
        },
        key,
        value,
    ] = args
    {
        let map = NativeStraightlineValue::Map {
            symbol: format!("@lk_map_set_{id}"),
            value: "{}".to_string(),
            entries: Vec::new(),
        };
        if let Some(value) = native_static_set_index(map, key.clone(), value.clone()) {
            static_regs[instr.a() as usize] = Some(value);
            return Some(());
        }
    }
    let [
        NativeStraightlineValue::DynamicMap {
            id,
            key,
            value: map_value,
        },
        map_key,
        value,
    ] = args
    else {
        return None;
    };
    let value_reg = instr.b().checked_add(3)?;
    let key_reg = instr.b().checked_add(2)?;
    let (next_key, next_value) = match (key, map_key, map_value, value) {
        (NativeMapKeyKind::Str, map_key, NativeMapValueKind::I64, NativeStraightlineValue::I64(_))
            if !matches!(map_key, NativeStraightlineValue::I64(_)) =>
        {
            emit_dynamic_string_int_map_set(ir, extra_globals, *id, value_reg, map_key.clone(), tmp_index)?;
            (NativeMapKeyKind::Str, NativeMapValueKind::I64)
        }
        (
            NativeMapKeyKind::Str,
            map_key,
            NativeMapValueKind::I64 | NativeMapValueKind::Bool,
            NativeStraightlineValue::Bool(_),
        ) if !matches!(map_key, NativeStraightlineValue::I64(_)) => {
            emit_dynamic_string_int_map_set(ir, extra_globals, *id, value_reg, map_key.clone(), tmp_index)?;
            (NativeMapKeyKind::Str, NativeMapValueKind::Bool)
        }
        (
            NativeMapKeyKind::Str,
            map_key,
            NativeMapValueKind::I64 | NativeMapValueKind::F64,
            NativeStraightlineValue::F64(_),
        ) if !matches!(map_key, NativeStraightlineValue::I64(_)) => {
            emit_dynamic_string_f64_map_set(ir, extra_globals, *id, value_reg, map_key.clone(), tmp_index)?;
            (NativeMapKeyKind::Str, NativeMapValueKind::F64)
        }
        (
            NativeMapKeyKind::Str | NativeMapKeyKind::I64,
            NativeStraightlineValue::I64(_),
            NativeMapValueKind::I64 | NativeMapValueKind::F64 | NativeMapValueKind::Bool,
            NativeStraightlineValue::F64(_),
        ) => {
            emit_dynamic_i64_f64_map_set(ir, *id, value_reg, key_reg, tmp_index)?;
            (NativeMapKeyKind::I64, NativeMapValueKind::F64)
        }
        (
            NativeMapKeyKind::Str | NativeMapKeyKind::I64,
            NativeStraightlineValue::I64(_),
            NativeMapValueKind::I64 | NativeMapValueKind::Bool,
            NativeStraightlineValue::Bool(_),
        ) => {
            emit_dynamic_i64_int_map_set(ir, *id, value_reg, key_reg, tmp_index)?;
            (NativeMapKeyKind::I64, NativeMapValueKind::Bool)
        }
        (
            NativeMapKeyKind::Str | NativeMapKeyKind::I64,
            NativeStraightlineValue::I64(_),
            NativeMapValueKind::I64 | NativeMapValueKind::StrPtr,
            NativeStraightlineValue::String { .. }
            | NativeStraightlineValue::StringPtr(_)
            | NativeStraightlineValue::Text(_),
        ) => {
            emit_dynamic_i64_ptr_map_set(ir, *id, value_reg, key_reg, tmp_index)?;
            (NativeMapKeyKind::I64, NativeMapValueKind::StrPtr)
        }
        (
            NativeMapKeyKind::Str,
            map_key,
            NativeMapValueKind::I64 | NativeMapValueKind::StrPtr,
            NativeStraightlineValue::String { .. }
            | NativeStraightlineValue::StringPtr(_)
            | NativeStraightlineValue::Text(_),
        ) if !matches!(map_key, NativeStraightlineValue::I64(_)) => {
            emit_dynamic_string_ptr_map_set(ir, extra_globals, *id, pc, value_reg, map_key.clone(), tmp_index)?;
            (NativeMapKeyKind::Str, NativeMapValueKind::StrPtr)
        }
        (
            NativeMapKeyKind::Str | NativeMapKeyKind::I64,
            NativeStraightlineValue::I64(_),
            NativeMapValueKind::I64,
            NativeStraightlineValue::I64(_),
        ) => {
            emit_dynamic_i64_int_map_set(ir, *id, value_reg, key_reg, tmp_index)?;
            (NativeMapKeyKind::I64, NativeMapValueKind::I64)
        }
        _ => return None,
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicMap {
        id: *id,
        key: next_key,
        value: next_value,
    });
    Some(())
}

pub(super) fn emit_dynamic_map_get_method_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let [
        NativeStraightlineValue::DynamicMap { id, key, value },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    if method != "get" {
        return None;
    }
    match key {
        NativeMapKeyKind::I64 => {
            let key = dynamic_i64_map_get_key(method_args)?;
            match value {
                NativeMapValueKind::I64 => emit_dynamic_i64_int_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
                NativeMapValueKind::F64 => emit_dynamic_i64_f64_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
                NativeMapValueKind::Bool => emit_dynamic_i64_int_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
                NativeMapValueKind::StrPtr => emit_dynamic_i64_ptr_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
            }
        }
        NativeMapKeyKind::Str => {
            let key = dynamic_string_map_get_key(method_args)?;
            match value {
                NativeMapValueKind::I64 | NativeMapValueKind::Bool => {
                    emit_dynamic_string_int_map_get(ir, extra_globals, *id, instr.a(), key, tmp_index)?
                }
                NativeMapValueKind::F64 => {
                    emit_dynamic_string_f64_map_get(ir, extra_globals, *id, instr.a(), key, tmp_index)?
                }
                NativeMapValueKind::StrPtr => {
                    emit_dynamic_string_ptr_map_get(ir, extra_globals, *id, pc, instr.a(), key, tmp_index)?
                }
            }
        }
    }
    static_regs[instr.a() as usize] = match value {
        NativeMapValueKind::I64 => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeI64 { value: loaded, present })
        }
        NativeMapValueKind::F64 => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load double, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeF64 { value: loaded, present })
        }
        NativeMapValueKind::Bool => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeBool { value: loaded, present })
        }
        NativeMapValueKind::StrPtr => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load ptr, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeStrPtr { value: loaded, present })
        }
    };
    Some(())
}

pub(super) fn emit_dynamic_map_get_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeBuiltin::MapModuleMethod("get") = builtin else {
        return None;
    };
    let [NativeStraightlineValue::DynamicMap { id, key, value }, method_arg] = args else {
        return None;
    };
    match key {
        NativeMapKeyKind::I64 => {
            let key = dynamic_i64_map_key(method_arg)?;
            match value {
                NativeMapValueKind::I64 => emit_dynamic_i64_int_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
                NativeMapValueKind::F64 => emit_dynamic_i64_f64_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
                NativeMapValueKind::Bool => emit_dynamic_i64_int_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
                NativeMapValueKind::StrPtr => emit_dynamic_i64_ptr_map_get_key(ir, *id, instr.a(), &key, tmp_index)?,
            }
        }
        NativeMapKeyKind::Str => match value {
            NativeMapValueKind::I64 | NativeMapValueKind::Bool => {
                emit_dynamic_string_int_map_get(ir, extra_globals, *id, instr.a(), method_arg.clone(), tmp_index)?
            }
            NativeMapValueKind::F64 => {
                emit_dynamic_string_f64_map_get(ir, extra_globals, *id, instr.a(), method_arg.clone(), tmp_index)?
            }
            NativeMapValueKind::StrPtr => {
                emit_dynamic_string_ptr_map_get(ir, extra_globals, *id, pc, instr.a(), method_arg.clone(), tmp_index)?
            }
        },
    }
    static_regs[instr.a() as usize] = match value {
        NativeMapValueKind::I64 => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeI64 { value: loaded, present })
        }
        NativeMapValueKind::F64 => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load double, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeF64 { value: loaded, present })
        }
        NativeMapValueKind::Bool => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeBool { value: loaded, present })
        }
        NativeMapValueKind::StrPtr => {
            let loaded = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {loaded} = load ptr, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            Some(NativeStraightlineValue::MaybeStrPtr { value: loaded, present })
        }
    };
    Some(())
}

pub(super) fn emit_dynamic_map_has_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeBuiltin::MapModuleMethod("has") = builtin else {
        return None;
    };
    let [NativeStraightlineValue::DynamicMap { id, key, .. }, method_arg] = args else {
        return None;
    };
    match key {
        NativeMapKeyKind::I64 => {
            let key = dynamic_i64_map_key(method_arg)?;
            emit_dynamic_i64_map_has_key(ir, *id, instr.a(), &key, pc, tmp_index)?;
        }
        NativeMapKeyKind::Str => {
            emit_dynamic_string_map_has(ir, extra_globals, *id, instr.a(), method_arg.clone(), pc, tmp_index)?;
        }
    }
    let loaded = next_tmp(tmp_index);
    ir.push_str(&format!("  {loaded} = load i64, ptr %r{}.slot\n", instr.a()));
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(loaded));
    Some(())
}

pub(super) fn emit_dynamic_map_delete_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    if builtin != NativeBuiltin::MapDelete {
        return None;
    }
    let [NativeStraightlineValue::DynamicMap { id, key, value }, method_arg] = args else {
        return None;
    };
    match key {
        NativeMapKeyKind::I64 => {
            let key = dynamic_i64_map_key(method_arg)?;
            match value {
                NativeMapValueKind::I64 => {
                    emit_dynamic_i64_int_map_delete_key(ir, *id, pc, instr.a(), &key, pc, tmp_index)?
                }
                NativeMapValueKind::F64 => {
                    emit_dynamic_i64_f64_map_delete_key(ir, *id, pc, instr.a(), &key, pc, tmp_index)?
                }
                NativeMapValueKind::Bool => {
                    emit_dynamic_i64_int_map_delete_key(ir, *id, pc, instr.a(), &key, pc, tmp_index)?
                }
                NativeMapValueKind::StrPtr => {
                    emit_dynamic_i64_ptr_map_delete_key(ir, *id, pc, instr.a(), &key, pc, tmp_index)?
                }
            }
        }
        NativeMapKeyKind::Str => match value {
            NativeMapValueKind::I64 | NativeMapValueKind::Bool => emit_dynamic_string_int_map_delete(
                ir,
                extra_globals,
                *id,
                pc,
                instr.a(),
                method_arg.clone(),
                pc,
                tmp_index,
            )?,
            NativeMapValueKind::F64 => emit_dynamic_string_f64_map_delete(
                ir,
                extra_globals,
                *id,
                pc,
                instr.a(),
                method_arg.clone(),
                pc,
                tmp_index,
            )?,
            NativeMapValueKind::StrPtr => emit_dynamic_string_ptr_map_delete(
                ir,
                extra_globals,
                *id,
                pc,
                instr.a(),
                method_arg.clone(),
                pc,
                tmp_index,
            )?,
        },
    }
    let removed = match value {
        NativeMapValueKind::I64 => {
            let value = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            NativeStraightlineValue::MaybeI64 { value, present }
        }
        NativeMapValueKind::F64 => {
            let value = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load double, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            NativeStraightlineValue::MaybeF64 { value, present }
        }
        NativeMapValueKind::Bool => {
            let value = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            NativeStraightlineValue::MaybeBool { value, present }
        }
        NativeMapValueKind::StrPtr => {
            let value = next_tmp(tmp_index);
            let present = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load ptr, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            NativeStraightlineValue::MaybeStrPtr { value, present }
        }
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::ArgList {
        elements: vec![
            NativeStraightlineValue::DynamicMap {
                id: pc,
                key: *key,
                value: *value,
            },
            removed,
        ],
    });
    Some(())
}

fn dynamic_i64_map_get_key(method_args: &NativeStraightlineValue) -> Option<String> {
    match method_args {
        NativeStraightlineValue::ArgList { elements } => {
            let [key] = elements.as_slice() else {
                return None;
            };
            dynamic_i64_map_key(key)
        }
        NativeStraightlineValue::List { elements, .. } => {
            let [ConstRuntimeValue32Data::Int(key)] = elements.as_slice() else {
                return None;
            };
            Some(key.to_string())
        }
        _ => None,
    }
}

fn dynamic_i64_map_key(value: &NativeStraightlineValue) -> Option<String> {
    let NativeStraightlineValue::I64(key) = value else {
        return None;
    };
    Some(key.clone())
}

fn dynamic_string_map_get_key(method_args: &NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    match method_args {
        NativeStraightlineValue::ArgList { elements } => {
            let [key] = elements.as_slice() else {
                return None;
            };
            Some(key.clone())
        }
        NativeStraightlineValue::List { elements, .. } => {
            let [key] = elements.as_slice() else {
                return None;
            };
            native_const_runtime_value(key, String::new())
        }
        _ => None,
    }
}

pub(super) fn emit_dynamic_map_values_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeBuiltin::MapModuleMethod("values") = builtin else {
        return None;
    };
    let [NativeStraightlineValue::DynamicMap { id, key, value }] = args else {
        return None;
    };
    let element = match value {
        NativeMapValueKind::I64 => {
            match key {
                NativeMapKeyKind::Str => emit_dynamic_string_int_map_values(ir, *id, pc, tmp_index)?,
                NativeMapKeyKind::I64 => emit_dynamic_i64_int_map_values(ir, *id, pc, tmp_index)?,
            }
            NativeListElementKind::I64
        }
        NativeMapValueKind::F64 => {
            match key {
                NativeMapKeyKind::Str => emit_dynamic_string_f64_map_values(ir, *id, pc, tmp_index)?,
                NativeMapKeyKind::I64 => emit_dynamic_i64_f64_map_values(ir, *id, pc, tmp_index)?,
            }
            NativeListElementKind::F64
        }
        NativeMapValueKind::Bool => {
            match key {
                NativeMapKeyKind::Str => emit_dynamic_string_int_map_values(ir, *id, pc, tmp_index)?,
                NativeMapKeyKind::I64 => emit_dynamic_i64_int_map_values(ir, *id, pc, tmp_index)?,
            }
            NativeListElementKind::Bool
        }
        NativeMapValueKind::StrPtr => {
            match key {
                NativeMapKeyKind::Str => emit_dynamic_string_ptr_map_values(ir, *id, pc, pc, tmp_index)?,
                NativeMapKeyKind::I64 => emit_dynamic_i64_ptr_map_values(ir, *id, pc, tmp_index)?,
            }
            NativeListElementKind::StrPtr
        }
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList { id: pc, element });
    Some(())
}

pub(super) fn emit_dynamic_map_keys_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeBuiltin::MapModuleMethod("keys") = builtin else {
        return None;
    };
    let [NativeStraightlineValue::DynamicMap { id, key, .. }] = args else {
        return None;
    };
    let element = match key {
        NativeMapKeyKind::Str => {
            emit_dynamic_string_map_keys(ir, extra_globals, *id, pc, tmp_index)?;
            NativeListElementKind::StrPtr
        }
        NativeMapKeyKind::I64 => {
            emit_dynamic_i64_map_keys(ir, *id, pc, tmp_index)?;
            NativeListElementKind::I64
        }
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList { id: pc, element });
    Some(())
}
