use crate::{
    llvm::{
        scalar::facts::NativeScalarKind,
        straightline_value::{
            NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
            native_static_set_index,
        },
    },
    vm::{ConstHeapValueData, ConstRuntimeValueData, Instr, Opcode},
};

use super::{
    analysis::{native_string_int_map_key_supported, static_value_kind},
    slots::{set_native_kind, set_static_value},
};

pub(super) fn propagate_dynamic_map_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    propagate_dynamic_map_set_call(kinds, static_values, instr, target, start)
        .or_else(|| propagate_dynamic_map_values_call(kinds, static_values, instr, pc, target, start))
        .or_else(|| propagate_dynamic_map_keys_call(kinds, static_values, instr, pc, target, start))
        .or_else(|| propagate_dynamic_map_get_call(kinds, static_values, instr, target, start))
        .or_else(|| propagate_dynamic_map_has_call(kinds, static_values, instr, target, start))
        .or_else(|| propagate_dynamic_map_delete_call(kinds, static_values, instr, pc, target, start))
}

pub(super) fn propagate_dynamic_map_set_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    if !matches!(target, NativeStraightlineValue::Builtin(NativeBuiltin::MapSet)) || instr.c() != 3 {
        return None;
    }
    let map_reg = u8::try_from(start).ok()?;
    let key_reg = u8::try_from(start + 1).ok()?;
    let value_reg = u8::try_from(start + 2).ok()?;
    let Some(NativeStraightlineValue::DynamicMap {
        id,
        key: map_key,
        value: map_value,
    }) = static_kind(static_values, map_reg)
    else {
        return None;
    };
    if let (Some(key), Some(value)) = (
        static_kind(static_values, key_reg),
        static_kind(static_values, value_reg),
    ) {
        let map = NativeStraightlineValue::Map {
            symbol: format!("@lk_map_set_{id}"),
            value: "{}".to_string(),
            entries: Vec::new(),
        };
        if let Some(value) = native_static_set_index(map, key, value) {
            return Some(set_static_value(
                kinds,
                static_values,
                instr.a(),
                static_value_kind(&value),
                value,
            ));
        }
    }
    let key_kind = native_kind(kinds, key_reg)
        .or_else(|| static_kind(static_values, key_reg).and_then(|value| static_value_kind(&value)));
    let value_kind = native_kind(kinds, value_reg)
        .or_else(|| static_kind(static_values, value_reg).and_then(|value| static_value_kind(&value)));
    let next_key = match (map_key, key_kind) {
        (NativeMapKeyKind::Str, Some(NativeScalarKind::StrPtr)) => NativeMapKeyKind::Str,
        (NativeMapKeyKind::Str | NativeMapKeyKind::I64, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)) => {
            NativeMapKeyKind::I64
        }
        _ => return Some(false),
    };
    let next_value = match (map_value, value_kind) {
        (NativeMapValueKind::I64, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)) => NativeMapValueKind::I64,
        (NativeMapValueKind::I64 | NativeMapValueKind::F64, Some(NativeScalarKind::F64)) => NativeMapValueKind::F64,
        (NativeMapValueKind::I64 | NativeMapValueKind::Bool, Some(NativeScalarKind::Bool)) => NativeMapValueKind::Bool,
        (NativeMapValueKind::I64 | NativeMapValueKind::StrPtr, Some(NativeScalarKind::StrPtr)) => {
            NativeMapValueKind::StrPtr
        }
        (NativeMapValueKind::F64, _) => return Some(false),
        (NativeMapValueKind::Bool, _) => return Some(false),
        (NativeMapValueKind::StrPtr, _) => return Some(false),
        _ => return Some(false),
    };
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicMap {
            id,
            key: next_key,
            value: next_value,
        },
    ))
}

pub(super) fn propagate_dynamic_map_values_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    if !matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("values"))
    ) || instr.c() != 1
    {
        return None;
    }
    let map_reg = u8::try_from(start).ok()?;
    let Some(NativeStraightlineValue::DynamicMap { value, .. }) = static_kind(static_values, map_reg) else {
        return None;
    };
    let element = match value {
        NativeMapValueKind::I64 => NativeListElementKind::I64,
        NativeMapValueKind::F64 => NativeListElementKind::F64,
        NativeMapValueKind::Bool => NativeListElementKind::Bool,
        NativeMapValueKind::StrPtr => NativeListElementKind::StrPtr,
    };
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicList { id: pc, element },
    ))
}

pub(super) fn propagate_dynamic_map_keys_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    if !matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("keys"))
    ) || instr.c() != 1
    {
        return None;
    }
    let map_reg = u8::try_from(start).ok()?;
    let Some(NativeStraightlineValue::DynamicMap { key, .. }) = static_kind(static_values, map_reg) else {
        return None;
    };
    let element = match key {
        NativeMapKeyKind::Str => NativeListElementKind::StrPtr,
        NativeMapKeyKind::I64 => NativeListElementKind::I64,
    };
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicList { id: pc, element },
    ))
}

pub(super) fn propagate_dynamic_map_has_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    if !matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("has"))
    ) || instr.c() != 2
    {
        return None;
    }
    let map_reg = u8::try_from(start).ok()?;
    let key_reg = u8::try_from(start + 1).ok()?;
    let Some(NativeStraightlineValue::DynamicMap { key: map_key, .. }) = static_kind(static_values, map_reg) else {
        return None;
    };
    let key_kind = native_kind(kinds, key_reg)
        .or_else(|| static_kind(static_values, key_reg).and_then(|value| static_value_kind(&value)));
    let ok = match map_key {
        NativeMapKeyKind::I64 => matches!(key_kind, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)),
        NativeMapKeyKind::Str => matches!(key_kind, Some(NativeScalarKind::StrPtr)),
    };
    if !ok {
        return Some(false);
    }
    Some(set_native_kind(kinds, static_values, instr.a(), NativeScalarKind::Bool))
}

pub(super) fn propagate_dynamic_map_get_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    if !matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("get"))
    ) || instr.c() != 2
    {
        return None;
    }
    let map_reg = u8::try_from(start).ok()?;
    let key_reg = u8::try_from(start + 1).ok()?;
    let Some(NativeStraightlineValue::DynamicMap {
        key: map_key, value, ..
    }) = static_kind(static_values, map_reg)
    else {
        return None;
    };
    let key_kind = native_kind(kinds, key_reg)
        .or_else(|| static_kind(static_values, key_reg).and_then(|value| static_value_kind(&value)));
    let ok = match map_key {
        NativeMapKeyKind::I64 => matches!(key_kind, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)),
        NativeMapKeyKind::Str => matches!(key_kind, Some(NativeScalarKind::StrPtr)),
    };
    if !ok {
        return Some(false);
    }
    let kind = match value {
        NativeMapValueKind::I64 => NativeScalarKind::MaybeI64,
        NativeMapValueKind::F64 => NativeScalarKind::F64,
        NativeMapValueKind::Bool => NativeScalarKind::Bool,
        NativeMapValueKind::StrPtr => NativeScalarKind::MaybeStrPtr,
    };
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_map_delete_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    if !matches!(target, NativeStraightlineValue::Builtin(NativeBuiltin::MapDelete)) || instr.c() != 2 {
        return None;
    }
    let map_reg = u8::try_from(start).ok()?;
    let key_reg = u8::try_from(start + 1).ok()?;
    let Some(NativeStraightlineValue::DynamicMap {
        key: map_key, value, ..
    }) = static_kind(static_values, map_reg)
    else {
        return None;
    };
    let key_kind = native_kind(kinds, key_reg)
        .or_else(|| static_kind(static_values, key_reg).and_then(|value| static_value_kind(&value)));
    let ok = match map_key {
        NativeMapKeyKind::I64 => matches!(key_kind, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)),
        NativeMapKeyKind::Str => matches!(key_kind, Some(NativeScalarKind::StrPtr)),
    };
    if !ok {
        return Some(false);
    }
    let removed = match value {
        NativeMapValueKind::I64 => NativeStraightlineValue::MaybeI64 {
            value: "0".to_string(),
            present: "0".to_string(),
        },
        NativeMapValueKind::F64 => NativeStraightlineValue::MaybeF64 {
            value: "0.0".to_string(),
            present: "0".to_string(),
        },
        NativeMapValueKind::Bool => NativeStraightlineValue::MaybeBool {
            value: "0".to_string(),
            present: "0".to_string(),
        },
        NativeMapValueKind::StrPtr => NativeStraightlineValue::MaybeStrPtr {
            value: String::new(),
            present: "0".to_string(),
        },
    };
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::ArgList {
            elements: vec![
                NativeStraightlineValue::DynamicMap {
                    id: pc,
                    key: map_key,
                    value,
                },
                removed,
            ],
        },
    ))
}

pub(super) fn propagate_dynamic_map_iter_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: NativeStraightlineValue,
    index_kind: Option<NativeScalarKind>,
    field: Option<NativeStraightlineValue>,
) -> Option<bool> {
    if let NativeStraightlineValue::DynamicMapIter { id, key, value } = target {
        if index_kind != Some(NativeScalarKind::I64) {
            return None;
        }
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            None,
            NativeStraightlineValue::DynamicMapEntry {
                id,
                index_reg: instr.c(),
                key,
                value,
            },
        ));
    }
    let NativeStraightlineValue::DynamicMapEntry { key, value, .. } = target else {
        return None;
    };
    let Some(NativeStraightlineValue::I64(field)) = field else {
        return None;
    };
    let kind = match field.as_str() {
        "0" => match key {
            NativeMapKeyKind::Str => NativeScalarKind::StrPtr,
            NativeMapKeyKind::I64 => NativeScalarKind::I64,
        },
        "1" => match value {
            NativeMapValueKind::I64 => NativeScalarKind::I64,
            NativeMapValueKind::F64 => NativeScalarKind::F64,
            NativeMapValueKind::Bool => NativeScalarKind::Bool,
            NativeMapValueKind::StrPtr => NativeScalarKind::StrPtr,
        },
        _ => return None,
    };
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_i64_map_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: &NativeStraightlineValue,
    index_kind: Option<NativeScalarKind>,
) -> Option<bool> {
    let NativeStraightlineValue::DynamicMap {
        key: NativeMapKeyKind::I64,
        value,
        ..
    } = target
    else {
        return None;
    };
    if !matches!(index_kind, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)) {
        return Some(false);
    }
    let kind = match value {
        NativeMapValueKind::I64 => NativeScalarKind::MaybeI64,
        NativeMapValueKind::F64 => NativeScalarKind::F64,
        NativeMapValueKind::Bool => NativeScalarKind::Bool,
        NativeMapValueKind::StrPtr => NativeScalarKind::MaybeStrPtr,
    };
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_string_map_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: &NativeStraightlineValue,
    key: Option<NativeStraightlineValue>,
) -> Option<bool> {
    let NativeStraightlineValue::DynamicMap {
        key: NativeMapKeyKind::Str,
        value,
        ..
    } = target
    else {
        return None;
    };
    if !matches!(
        value,
        NativeMapValueKind::I64 | NativeMapValueKind::F64 | NativeMapValueKind::Bool | NativeMapValueKind::StrPtr
    ) {
        return None;
    }
    if instr.b() != instr.c() && !key.as_ref().is_some_and(native_string_int_map_key_supported) {
        return Some(false);
    }
    let kind = match value {
        NativeMapValueKind::I64 => NativeScalarKind::MaybeI64,
        NativeMapValueKind::F64 => NativeScalarKind::F64,
        NativeMapValueKind::Bool => NativeScalarKind::Bool,
        NativeMapValueKind::StrPtr => NativeScalarKind::MaybeStrPtr,
    };
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_string_list_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    target: &NativeStraightlineValue,
    index_kind: Option<NativeScalarKind>,
) -> Option<bool> {
    if !matches!(
        target,
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
            ..
        }
    ) {
        return None;
    }
    if index_kind != Some(NativeScalarKind::I64) {
        return Some(false);
    }
    Some(set_native_kind(
        kinds,
        static_values,
        instr.a(),
        NativeScalarKind::StrPtr,
    ))
}

pub(super) fn propagate_dynamic_string_list_method_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    args: &[NativeStraightlineValue],
) -> Option<bool> {
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
    let arg_len = match method_args {
        NativeStraightlineValue::ArgList { elements } => elements.len(),
        NativeStraightlineValue::List { elements, .. } => elements.len(),
        _ => return Some(false),
    };
    let ok = match method.as_str() {
        "take" | "skip" => arg_len == 1,
        "concat" | "chain" => {
            if arg_len != 1 {
                false
            } else {
                matches!(
                    method_args,
                    NativeStraightlineValue::ArgList { elements }
                        if matches!(
                            elements.first(),
                            Some(NativeStraightlineValue::DynamicList {
                                element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
                                ..
                            })
                        )
                )
            }
        }
        "unique" => arg_len == 0,
        _ => return None,
    };
    if !ok {
        return Some(false);
    }
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicList {
            id: *id,
            element: NativeListElementKind::StrPtr,
        },
    ))
}

pub(super) fn propagate_dynamic_i64_list_method_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    args: &[NativeStraightlineValue],
) -> Option<bool> {
    let [
        NativeStraightlineValue::DynamicList { id, element },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    let arg_len = match method_args {
        NativeStraightlineValue::ArgList { elements } => elements.len(),
        NativeStraightlineValue::List { elements, .. } => elements.len(),
        NativeStraightlineValue::DynamicList { .. } => 1,
        _ => return Some(false),
    };
    if !matches!(element, NativeListElementKind::I64 | NativeListElementKind::Bool) {
        return None;
    }
    let ok = match method.as_str() {
        "take" | "skip" => arg_len == 1,
        "concat" | "chain" => {
            arg_len == 1
                && (matches!(
                    method_args,
                    NativeStraightlineValue::DynamicList {
                        element: rhs_element,
                        ..
                    } if rhs_element == element
                ) || (*element == NativeListElementKind::I64
                    && matches!(
                        method_args,
                        NativeStraightlineValue::List { elements, .. }
                            if elements.iter().all(|value| matches!(value, ConstRuntimeValueData::Int(_)))
                    ))
                    || (*element == NativeListElementKind::Bool
                        && matches!(
                            method_args,
                            NativeStraightlineValue::List { elements, .. }
                                if elements.iter().all(|value| matches!(value, ConstRuntimeValueData::Bool(_)))
                        ))
                    || matches!(
                        method_args,
                        NativeStraightlineValue::ArgList { elements }
                            if arglist_first_i64_storage_list_like(elements, *element)
                    ))
        }
        _ => return None,
    };
    if !ok {
        return Some(false);
    }
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicList {
            id: *id,
            element: *element,
        },
    ))
}

pub(super) fn propagate_dynamic_f64_list_method_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    args: &[NativeStraightlineValue],
) -> Option<bool> {
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
    let arg_len = match method_args {
        NativeStraightlineValue::ArgList { elements } => elements.len(),
        NativeStraightlineValue::List { elements, .. } => elements.len(),
        NativeStraightlineValue::DynamicList { .. } => 1,
        _ => return Some(false),
    };
    let ok = match method.as_str() {
        "take" | "skip" => arg_len == 1,
        "concat" | "chain" => {
            arg_len == 1
                && (matches!(
                    method_args,
                    NativeStraightlineValue::DynamicList {
                        element: NativeListElementKind::F64,
                        ..
                    }
                ) || matches!(
                    method_args,
                    NativeStraightlineValue::List { elements, .. }
                        if elements.iter().all(|value| matches!(
                            value,
                            ConstRuntimeValueData::Float(_) | ConstRuntimeValueData::Int(_)
                        ))
                ) || matches!(
                    method_args,
                    NativeStraightlineValue::ArgList { elements }
                        if arglist_first_f64_list_like(elements)
                ))
        }
        _ => return None,
    };
    if !ok {
        return Some(false);
    }
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicList {
            id: *id,
            element: NativeListElementKind::F64,
        },
    ))
}

pub(super) fn propagate_dynamic_f64_list_builtin_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    let kind = match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains) => NativeScalarKind::Bool,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListIndexOf) => NativeScalarKind::I64,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPop) => NativeScalarKind::F64,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListReverse | NativeBuiltin::ListSort) => NativeScalarKind::I64,
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListPush
            | NativeBuiltin::ListSlice
            | NativeBuiltin::ListInsert
            | NativeBuiltin::ListRemoveAt
            | NativeBuiltin::ListSet,
        ) => NativeScalarKind::I64,
        _ => return None,
    };
    let expected_arity = match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf) => 2,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPush | NativeBuiltin::ListRemoveAt) => 2,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListInsert | NativeBuiltin::ListSet) => 3,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListSlice) if instr.c() == 2 || instr.c() == 3 => instr.c(),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListPop | NativeBuiltin::ListReverse | NativeBuiltin::ListSort,
        ) => 1,
        _ => return None,
    };
    if instr.c() != expected_arity {
        return Some(false);
    }
    let list_reg = u8::try_from(start).ok()?;
    if !matches!(
        static_kind(static_values, list_reg),
        Some(NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::F64,
            ..
        })
    ) {
        return None;
    }
    match target {
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf | NativeBuiltin::ListPush,
        ) => {
            let value_reg = u8::try_from(start + 1).ok()?;
            if !f64_list_f64_arg(kinds, static_values, value_reg) {
                return Some(false);
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListSlice) => {
            let start_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, start_reg) {
                return Some(false);
            }
            if instr.c() == 3 {
                let end_reg = u8::try_from(start + 2).ok()?;
                if !ptr_list_i64_arg(kinds, static_values, end_reg) {
                    return Some(false);
                }
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListInsert | NativeBuiltin::ListSet) => {
            let index_reg = u8::try_from(start + 1).ok()?;
            let value_reg = u8::try_from(start + 2).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, index_reg) || !f64_list_f64_arg(kinds, static_values, value_reg)
            {
                return Some(false);
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListRemoveAt) => {
            let index_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, index_reg) {
                return Some(false);
            }
        }
        _ => {}
    }
    if matches!(
        target,
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListReverse
                | NativeBuiltin::ListSort
                | NativeBuiltin::ListPush
                | NativeBuiltin::ListSlice
                | NativeBuiltin::ListInsert
        )
    ) {
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            Some(kind),
            NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::F64,
            },
        ));
    }
    if matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListRemoveAt | NativeBuiltin::ListSet)
    ) {
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            None,
            NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::F64,
                    },
                    NativeStraightlineValue::F64("0.0".to_string()),
                ],
            },
        ));
    }
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_i64_list_builtin_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    let NativeStraightlineValue::Builtin(builtin) = target else {
        return None;
    };
    let list_reg = u8::try_from(start).ok()?;
    let Some(NativeStraightlineValue::DynamicList { id, element }) = static_kind(static_values, list_reg) else {
        return None;
    };
    if !dynamic_i64_storage_list_builtin_supported(code, heap_values, id, element) {
        return None;
    }
    let kind = match builtin {
        NativeBuiltin::ListContains => NativeScalarKind::Bool,
        NativeBuiltin::ListIndexOf => NativeScalarKind::I64,
        NativeBuiltin::ListPop if element == NativeListElementKind::Bool => NativeScalarKind::Bool,
        NativeBuiltin::ListPop if element == NativeListElementKind::I64 => NativeScalarKind::I64,
        NativeBuiltin::ListReverse => NativeScalarKind::I64,
        NativeBuiltin::ListPush
        | NativeBuiltin::ListSlice
        | NativeBuiltin::ListInsert
        | NativeBuiltin::ListRemoveAt
        | NativeBuiltin::ListSet => NativeScalarKind::I64,
        _ => return None,
    };
    let expected_arity = match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf => 2,
        NativeBuiltin::ListPush | NativeBuiltin::ListRemoveAt => 2,
        NativeBuiltin::ListInsert | NativeBuiltin::ListSet => 3,
        NativeBuiltin::ListSlice if instr.c() == 2 || instr.c() == 3 => instr.c(),
        NativeBuiltin::ListPop | NativeBuiltin::ListReverse => 1,
        _ => return None,
    };
    if instr.c() != expected_arity {
        return Some(false);
    }
    match builtin {
        NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf | NativeBuiltin::ListPush => {
            let value_reg = u8::try_from(start + 1).ok()?;
            if !i64_storage_list_value_arg(kinds, static_values, value_reg, element) {
                return Some(false);
            }
        }
        NativeBuiltin::ListSlice => {
            let start_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, start_reg) {
                return Some(false);
            }
            if instr.c() == 3 {
                let end_reg = u8::try_from(start + 2).ok()?;
                if !ptr_list_i64_arg(kinds, static_values, end_reg) {
                    return Some(false);
                }
            }
        }
        NativeBuiltin::ListInsert | NativeBuiltin::ListSet => {
            let index_reg = u8::try_from(start + 1).ok()?;
            let value_reg = u8::try_from(start + 2).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, index_reg)
                || !i64_storage_list_value_arg(kinds, static_values, value_reg, element)
            {
                return Some(false);
            }
        }
        NativeBuiltin::ListRemoveAt => {
            let index_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, index_reg) {
                return Some(false);
            }
        }
        _ => {}
    }
    if matches!(
        builtin,
        NativeBuiltin::ListReverse
            | NativeBuiltin::ListSort
            | NativeBuiltin::ListPush
            | NativeBuiltin::ListSlice
            | NativeBuiltin::ListInsert
    ) {
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            Some(kind),
            NativeStraightlineValue::DynamicList { id: pc, element },
        ));
    }
    if matches!(builtin, NativeBuiltin::ListRemoveAt | NativeBuiltin::ListSet) {
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            None,
            NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList { id: pc, element },
                    i64_storage_list_old_value(element),
                ],
            },
        ));
    }
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
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

fn i64_storage_list_value_arg(
    kinds: &[Option<NativeScalarKind>],
    static_values: &[Option<NativeStraightlineValue>],
    reg: u8,
    element: NativeListElementKind,
) -> bool {
    match element {
        NativeListElementKind::Bool => bool_list_bool_arg(kinds, static_values, reg),
        NativeListElementKind::I64 => ptr_list_i64_arg(kinds, static_values, reg),
        _ => false,
    }
}

fn i64_storage_list_old_value(element: NativeListElementKind) -> NativeStraightlineValue {
    match element {
        NativeListElementKind::Bool => NativeStraightlineValue::Bool("0".to_string()),
        _ => NativeStraightlineValue::I64("0".to_string()),
    }
}

fn bool_list_bool_arg(
    kinds: &[Option<NativeScalarKind>],
    static_values: &[Option<NativeStraightlineValue>],
    reg: u8,
) -> bool {
    native_kind(kinds, reg) == Some(NativeScalarKind::Bool)
        || matches!(static_kind(static_values, reg), Some(NativeStraightlineValue::Bool(_)))
}

pub(super) fn propagate_dynamic_ptr_list_builtin_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    let kind = match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains) => NativeScalarKind::Bool,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListIndexOf) => NativeScalarKind::I64,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPop) => NativeScalarKind::StrPtr,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListReverse | NativeBuiltin::ListSort) => {
            NativeScalarKind::StrPtr
        }
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListPush
            | NativeBuiltin::ListSlice
            | NativeBuiltin::ListInsert
            | NativeBuiltin::ListRemoveAt
            | NativeBuiltin::ListSet,
        ) => NativeScalarKind::StrPtr,
        _ => return None,
    };
    let expected_arity = match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf) => 2,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPush | NativeBuiltin::ListRemoveAt) => 2,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListInsert | NativeBuiltin::ListSet) => 3,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListSlice) if instr.c() == 2 || instr.c() == 3 => instr.c(),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListPop | NativeBuiltin::ListReverse | NativeBuiltin::ListSort,
        ) => 1,
        _ => return None,
    };
    if instr.c() != expected_arity {
        return Some(false);
    }
    let list_reg = u8::try_from(start).ok()?;
    if !matches!(
        static_kind(static_values, list_reg),
        Some(NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::StrPtr | NativeListElementKind::Text,
            ..
        })
    ) {
        return None;
    }
    match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf) => {
            let needle_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_string_arg(kinds, static_values, needle_reg) {
                return Some(false);
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPush) => {
            let value_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_string_arg(kinds, static_values, value_reg) {
                return Some(false);
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListSlice) => {
            let start_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, start_reg) {
                return Some(false);
            }
            if instr.c() == 3 {
                let end_reg = u8::try_from(start + 2).ok()?;
                if !ptr_list_i64_arg(kinds, static_values, end_reg) {
                    return Some(false);
                }
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListInsert | NativeBuiltin::ListSet) => {
            let index_reg = u8::try_from(start + 1).ok()?;
            let value_reg = u8::try_from(start + 2).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, index_reg)
                || !ptr_list_string_arg(kinds, static_values, value_reg)
            {
                return Some(false);
            }
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListRemoveAt) => {
            let index_reg = u8::try_from(start + 1).ok()?;
            if !ptr_list_i64_arg(kinds, static_values, index_reg) {
                return Some(false);
            }
        }
        _ => {}
    }
    if matches!(
        target,
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListReverse
                | NativeBuiltin::ListSort
                | NativeBuiltin::ListPush
                | NativeBuiltin::ListSlice
                | NativeBuiltin::ListInsert
        )
    ) {
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            Some(kind),
            NativeStraightlineValue::DynamicList {
                id: pc,
                element: NativeListElementKind::StrPtr,
            },
        ));
    }
    if matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListRemoveAt | NativeBuiltin::ListSet)
    ) {
        return Some(set_static_value(
            kinds,
            static_values,
            instr.a(),
            None,
            NativeStraightlineValue::ArgList {
                elements: vec![
                    NativeStraightlineValue::DynamicList {
                        id: pc,
                        element: NativeListElementKind::StrPtr,
                    },
                    NativeStraightlineValue::StringPtr(String::new()),
                ],
            },
        ));
    }
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

fn ptr_list_string_arg(
    kinds: &[Option<NativeScalarKind>],
    static_values: &[Option<NativeStraightlineValue>],
    reg: u8,
) -> bool {
    matches!(
        native_kind(kinds, reg),
        Some(NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr)
    ) || matches!(
        static_kind(static_values, reg),
        Some(
            NativeStraightlineValue::String { .. }
                | NativeStraightlineValue::StringPtr(_)
                | NativeStraightlineValue::Text(_)
        )
    )
}

fn ptr_list_i64_arg(
    kinds: &[Option<NativeScalarKind>],
    static_values: &[Option<NativeStraightlineValue>],
    reg: u8,
) -> bool {
    matches!(
        native_kind(kinds, reg),
        Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
    ) || matches!(static_kind(static_values, reg), Some(NativeStraightlineValue::I64(_)))
}

fn f64_list_f64_arg(
    kinds: &[Option<NativeScalarKind>],
    static_values: &[Option<NativeStraightlineValue>],
    reg: u8,
) -> bool {
    matches!(
        native_kind(kinds, reg),
        Some(NativeScalarKind::F64 | NativeScalarKind::I64 | NativeScalarKind::MaybeI64)
    ) || matches!(
        static_kind(static_values, reg),
        Some(NativeStraightlineValue::F64(_) | NativeStraightlineValue::I64(_))
    )
}

fn arglist_first_i64_storage_list_like(elements: &[NativeStraightlineValue], element: NativeListElementKind) -> bool {
    match elements.first() {
        Some(NativeStraightlineValue::DynamicList {
            element: rhs_element, ..
        }) => *rhs_element == element,
        Some(NativeStraightlineValue::List { elements, .. }) => match element {
            NativeListElementKind::I64 => elements
                .iter()
                .all(|value| matches!(value, ConstRuntimeValueData::Int(_))),
            NativeListElementKind::Bool => elements
                .iter()
                .all(|value| matches!(value, ConstRuntimeValueData::Bool(_))),
            _ => false,
        },
        _ => false,
    }
}

fn arglist_first_f64_list_like(elements: &[NativeStraightlineValue]) -> bool {
    match elements.first() {
        Some(NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::F64,
            ..
        }) => true,
        Some(NativeStraightlineValue::List { elements, .. }) => elements
            .iter()
            .all(|value| matches!(value, ConstRuntimeValueData::Float(_) | ConstRuntimeValueData::Int(_))),
        _ => false,
    }
}

fn native_kind(kinds: &[Option<NativeScalarKind>], reg: u8) -> Option<NativeScalarKind> {
    kinds.get(reg as usize).copied().flatten()
}

fn static_kind(values: &[Option<NativeStraightlineValue>], reg: u8) -> Option<NativeStraightlineValue> {
    values.get(reg as usize).cloned().flatten()
}

pub(super) fn dynamic_heap_container_value(value: &ConstHeapValueData, id: usize) -> Option<NativeStraightlineValue> {
    if let ConstHeapValueData::List(values) = value
        && (values.is_empty() || values.iter().all(|v| matches!(v, ConstRuntimeValueData::Int(_))))
    {
        return Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        });
    }
    if matches!(value, ConstHeapValueData::Map(values) if values.is_empty()) {
        return Some(NativeStraightlineValue::DynamicMap {
            id,
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
        });
    }
    None
}

pub(super) fn dynamic_map_to_iter_value(target: &NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::DynamicMap { id, key, value } = target else {
        return None;
    };
    Some(NativeStraightlineValue::DynamicMapIter {
        id: *id,
        key: *key,
        value: *value,
    })
}

pub(super) fn dynamic_map_get_method_kind(args: &[NativeStraightlineValue]) -> Option<NativeScalarKind> {
    let [
        NativeStraightlineValue::DynamicMap { value, .. },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    else {
        return None;
    };
    if method != "get" {
        return None;
    }
    let arg_len = match method_args {
        NativeStraightlineValue::ArgList { elements } => elements.len(),
        NativeStraightlineValue::List { elements, .. } => elements.len(),
        _ => return None,
    };
    if arg_len != 1 {
        return None;
    }
    match value {
        NativeMapValueKind::I64 => Some(NativeScalarKind::MaybeI64),
        NativeMapValueKind::F64 => Some(NativeScalarKind::F64),
        NativeMapValueKind::Bool => Some(NativeScalarKind::Bool),
        NativeMapValueKind::StrPtr => Some(NativeScalarKind::MaybeStrPtr),
    }
}
