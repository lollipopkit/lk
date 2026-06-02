use crate::{
    llvm::{
        scalar::facts::NativeScalarKind,
        straightline_value::{
            NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
            native_static_set_index,
        },
    },
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32},
};

use super::{
    analysis::{native_string_int_map_key_supported, static_value_kind},
    slots::{set_native_kind, set_static_value},
};

pub(super) fn propagate_dynamic_map_set_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
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
    if next_key == NativeMapKeyKind::I64 && map_value != NativeMapValueKind::I64 {
        return Some(false);
    }
    let next_value = match (map_value, value_kind) {
        (NativeMapValueKind::I64, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)) => NativeMapValueKind::I64,
        (NativeMapValueKind::I64 | NativeMapValueKind::F64, Some(NativeScalarKind::F64))
            if next_key == NativeMapKeyKind::Str =>
        {
            NativeMapValueKind::F64
        }
        (NativeMapValueKind::F64, _) => return Some(false),
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
    instr: Instr32,
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
    instr: Instr32,
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
    let Some(NativeStraightlineValue::DynamicMap {
        key: NativeMapKeyKind::Str,
        ..
    }) = static_kind(static_values, map_reg)
    else {
        return None;
    };
    Some(set_static_value(
        kinds,
        static_values,
        instr.a(),
        None,
        NativeStraightlineValue::DynamicList {
            id: pc,
            element: NativeListElementKind::StrPtr,
        },
    ))
}

pub(super) fn propagate_dynamic_map_iter_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
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
        },
        _ => return None,
    };
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_i64_map_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    target: &NativeStraightlineValue,
    index_kind: Option<NativeScalarKind>,
) -> Option<bool> {
    if !matches!(
        target,
        NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::I64,
            value: NativeMapValueKind::I64,
            ..
        }
    ) {
        return None;
    }
    if !matches!(index_kind, Some(NativeScalarKind::I64 | NativeScalarKind::MaybeI64)) {
        return Some(false);
    }
    Some(set_native_kind(
        kinds,
        static_values,
        instr.a(),
        NativeScalarKind::MaybeI64,
    ))
}

pub(super) fn propagate_dynamic_string_map_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
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
    if !matches!(value, NativeMapValueKind::I64 | NativeMapValueKind::F64) {
        return None;
    }
    let Some(key) = key else {
        return Some(false);
    };
    if !native_string_int_map_key_supported(&key) {
        return Some(false);
    }
    let kind = match value {
        NativeMapValueKind::I64 => NativeScalarKind::MaybeI64,
        NativeMapValueKind::F64 => NativeScalarKind::F64,
    };
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_string_list_get_index(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
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
    instr: Instr32,
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
    instr: Instr32,
    args: &[NativeStraightlineValue],
) -> Option<bool> {
    let [
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
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
                        element: NativeListElementKind::I64,
                        ..
                    }
                ) || matches!(
                    method_args,
                    NativeStraightlineValue::List { elements, .. }
                        if elements.iter().all(|value| matches!(value, ConstRuntimeValue32Data::Int(_)))
                ) || matches!(
                    method_args,
                    NativeStraightlineValue::ArgList { elements }
                        if arglist_first_i64_list_like(elements)
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
            element: NativeListElementKind::I64,
        },
    ))
}

pub(super) fn propagate_dynamic_f64_list_method_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
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
                            ConstRuntimeValue32Data::Float(_) | ConstRuntimeValue32Data::Int(_)
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
    instr: Instr32,
    pc: usize,
    target: &NativeStraightlineValue,
    start: usize,
) -> Option<bool> {
    let kind = match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains) => NativeScalarKind::Bool,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListIndexOf) => NativeScalarKind::I64,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPop) => NativeScalarKind::F64,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListReverse | NativeBuiltin::ListSort) => NativeScalarKind::I64,
        _ => return None,
    };
    let expects_needle = matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains | NativeBuiltin::ListIndexOf)
    );
    if (expects_needle && instr.c() != 2) || (!expects_needle && instr.c() != 1) {
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
    if expects_needle {
        let needle_reg = u8::try_from(start + 1).ok()?;
        if native_kind(kinds, needle_reg) != Some(NativeScalarKind::F64)
            && !matches!(
                static_kind(static_values, needle_reg),
                Some(NativeStraightlineValue::F64(_))
            )
        {
            return Some(false);
        }
    }
    if matches!(
        target,
        NativeStraightlineValue::Builtin(NativeBuiltin::ListReverse | NativeBuiltin::ListSort)
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
    Some(set_native_kind(kinds, static_values, instr.a(), kind))
}

pub(super) fn propagate_dynamic_ptr_list_builtin_call(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
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

fn arglist_first_i64_list_like(elements: &[NativeStraightlineValue]) -> bool {
    match elements.first() {
        Some(NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        }) => true,
        Some(NativeStraightlineValue::List { elements, .. }) => elements
            .iter()
            .all(|value| matches!(value, ConstRuntimeValue32Data::Int(_))),
        _ => false,
    }
}

fn arglist_first_f64_list_like(elements: &[NativeStraightlineValue]) -> bool {
    match elements.first() {
        Some(NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::F64,
            ..
        }) => true,
        Some(NativeStraightlineValue::List { elements, .. }) => elements.iter().all(|value| {
            matches!(
                value,
                ConstRuntimeValue32Data::Float(_) | ConstRuntimeValue32Data::Int(_)
            )
        }),
        _ => false,
    }
}

fn native_kind(kinds: &[Option<NativeScalarKind>], reg: u8) -> Option<NativeScalarKind> {
    kinds.get(reg as usize).copied().flatten()
}

fn static_kind(values: &[Option<NativeStraightlineValue>], reg: u8) -> Option<NativeStraightlineValue> {
    values.get(reg as usize).cloned().flatten()
}

pub(super) fn dynamic_heap_container_value(value: &ConstHeapValue32Data, id: usize) -> Option<NativeStraightlineValue> {
    if let ConstHeapValue32Data::List(values) = value
        && (values.is_empty() || values.iter().all(|v| matches!(v, ConstRuntimeValue32Data::Int(_))))
    {
        return Some(NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        });
    }
    if matches!(value, ConstHeapValue32Data::Map(values) if values.is_empty()) {
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
