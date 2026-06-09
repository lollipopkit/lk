use crate::{
    llvm::{
        const_display::native_const_list_display,
        straightline_value::{
            NativeBuiltin, NativeStraightlineValue, native_static_contains, native_static_index, native_static_len,
            native_static_map_delete, native_static_set_index,
        },
    },
    vm::{ConstRuntimeValueData, RuntimeMapKeyData},
};

pub(super) fn emit_native_map_builtin(
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match builtin {
        NativeBuiltin::MapModuleMethod(method) => emit_native_map_module_method(method, args, ssa_index),
        NativeBuiltin::MapDelete => emit_native_map_delete(args, ssa_index),
        NativeBuiltin::MapSet => emit_native_map_set(args),
        NativeBuiltin::MapMutate => None,
        _ => None,
    }
}

fn emit_native_map_module_method(
    method: &str,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match method {
        "len" => emit_native_map_len(args),
        "keys" => emit_native_map_keys(args, ssa_index),
        "values" => emit_native_map_values(args, ssa_index),
        "has" => emit_native_map_has(args),
        "get" => emit_native_map_get(args, ssa_index),
        _ => None,
    }
}

fn emit_native_map_len(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    native_static_len(target.clone())
}

fn emit_native_map_keys(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    let elements = match target {
        NativeStraightlineValue::Map { entries, .. } => entries
            .iter()
            .map(|(key, _)| native_map_key_arg(key))
            .collect::<Option<Vec<_>>>()?,
        NativeStraightlineValue::DisplayMap { entries, .. } => entries
            .iter()
            .map(|(key, _)| native_map_key_arg(key))
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    native_static_map_list_value(elements, "keys", ssa_index)
}

fn emit_native_map_values(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    match target {
        NativeStraightlineValue::Map { entries, .. } => native_static_map_list_value(
            entries.iter().map(|(_, value)| value.clone()).collect(),
            "values",
            ssa_index,
        ),
        NativeStraightlineValue::DisplayMap { entries, .. } => Some(NativeStraightlineValue::ArgList {
            elements: entries.iter().map(|(_, value)| value.clone()).collect(),
        }),
        _ => None,
    }
}

fn emit_native_map_has(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target, key] = args else {
        return None;
    };
    native_static_contains(key.clone(), target.clone())
}

fn emit_native_map_get(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, key] = args else {
        return None;
    };
    let symbol = format!("@lk_map_get_{}", *ssa_index);
    *ssa_index += 1;
    native_static_index(target.clone(), key.clone(), symbol)
}

pub(in crate::llvm::output) fn emit_native_map_delete(
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [target, key] = args else {
        return None;
    };
    let symbol = format!("@lk_map_delete_{}", *ssa_index);
    *ssa_index += 1;
    native_static_map_delete(target.clone(), key.clone(), symbol)
}

pub(in crate::llvm) fn emit_native_map_set(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target, key, value] = args else {
        return None;
    };
    native_static_set_index(target.clone(), key.clone(), value.clone())
}

fn native_static_map_list_value(
    elements: Vec<ConstRuntimeValueData>,
    method: &str,
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let symbol = format!("@lk_map_{method}_{}", *ssa_index);
    *ssa_index += 1;
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

fn native_map_key_arg(key: &RuntimeMapKeyData) -> Option<ConstRuntimeValueData> {
    match key {
        RuntimeMapKeyData::Nil => Some(ConstRuntimeValueData::Nil),
        RuntimeMapKeyData::Bool(value) => Some(ConstRuntimeValueData::Bool(*value)),
        RuntimeMapKeyData::Int(value) => Some(ConstRuntimeValueData::Int(*value)),
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => {
            Some(ConstRuntimeValueData::ShortStr(value.clone()))
        }
        RuntimeMapKeyData::Obj(_) => None,
    }
}
