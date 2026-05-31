use crate::vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, RuntimeMapKeyData};

use super::NativeStraightlineValue;

pub(super) fn native_map_entries_are_string_keyed(entries: &[(RuntimeMapKeyData, ConstRuntimeValue32Data)]) -> bool {
    !entries.is_empty() && entries.iter().all(|(key, _)| native_map_key_str(key).is_some())
}

pub(super) fn native_map_entry_keys_match(
    lhs: &RuntimeMapKeyData,
    rhs: &RuntimeMapKeyData,
    compare_string_keys: bool,
) -> bool {
    if compare_string_keys {
        native_map_keys_match(lhs, rhs)
    } else {
        lhs == rhs
    }
}

pub(super) fn native_map_keys_match(lhs: &RuntimeMapKeyData, rhs: &RuntimeMapKeyData) -> bool {
    lhs == rhs
        || native_map_key_str(lhs)
            .zip(native_map_key_str(rhs))
            .is_some_and(|(lhs, rhs)| lhs == rhs)
}

pub(super) fn native_map_key_str(key: &RuntimeMapKeyData) -> Option<&str> {
    match key {
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => Some(value),
        _ => None,
    }
}

pub(super) fn native_static_value_eq(lhs: &NativeStraightlineValue, rhs: &NativeStraightlineValue) -> bool {
    match (lhs, rhs) {
        (NativeStraightlineValue::Nil, NativeStraightlineValue::Nil) => true,
        (NativeStraightlineValue::Bool(lhs), NativeStraightlineValue::Bool(rhs)) => lhs == rhs,
        (NativeStraightlineValue::I64(lhs), NativeStraightlineValue::I64(rhs)) => lhs == rhs,
        (NativeStraightlineValue::F64(lhs), NativeStraightlineValue::F64(rhs)) => lhs == rhs,
        (NativeStraightlineValue::String { value: lhs, .. }, NativeStraightlineValue::String { value: rhs, .. }) => {
            lhs == rhs
        }
        (NativeStraightlineValue::List { elements: lhs, .. }, NativeStraightlineValue::List { elements: rhs, .. }) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs.iter())
                    .all(|(lhs, rhs)| native_const_runtime_eq(lhs, rhs))
        }
        (NativeStraightlineValue::Map { entries: lhs, .. }, NativeStraightlineValue::Map { entries: rhs, .. }) => {
            let compare_string_keys =
                native_map_entries_are_string_keyed(lhs) && native_map_entries_are_string_keyed(rhs);
            lhs.len() == rhs.len()
                && lhs.iter().all(|(lhs_key, lhs_value)| {
                    rhs.iter()
                        .find(|(rhs_key, _)| native_map_entry_keys_match(rhs_key, lhs_key, compare_string_keys))
                        .is_some_and(|(_, rhs_value)| native_const_runtime_eq(lhs_value, rhs_value))
                })
        }
        (
            NativeStraightlineValue::Object {
                type_name: lhs_name,
                fields: lhs,
                ..
            },
            NativeStraightlineValue::Object {
                type_name: rhs_name,
                fields: rhs,
                ..
            },
        ) => {
            lhs_name == rhs_name
                && lhs.len() == rhs.len()
                && lhs.iter().all(|(lhs_key, lhs_value)| {
                    rhs.iter()
                        .find(|(rhs_key, _)| rhs_key == lhs_key)
                        .is_some_and(|(_, rhs_value)| native_const_runtime_eq(lhs_value, rhs_value))
                })
        }
        _ => false,
    }
}

pub(super) fn native_const_runtime_eq(lhs: &ConstRuntimeValue32Data, rhs: &ConstRuntimeValue32Data) -> bool {
    match (lhs, rhs) {
        (ConstRuntimeValue32Data::Nil, ConstRuntimeValue32Data::Nil) => true,
        (ConstRuntimeValue32Data::Bool(lhs), ConstRuntimeValue32Data::Bool(rhs)) => lhs == rhs,
        (ConstRuntimeValue32Data::Int(lhs), ConstRuntimeValue32Data::Int(rhs)) => lhs == rhs,
        (ConstRuntimeValue32Data::Float(lhs), ConstRuntimeValue32Data::Float(rhs)) => lhs == rhs,
        (ConstRuntimeValue32Data::ShortStr(lhs), ConstRuntimeValue32Data::ShortStr(rhs)) => lhs == rhs,
        (ConstRuntimeValue32Data::Heap(lhs), ConstRuntimeValue32Data::Heap(rhs)) => {
            native_const_heap_eq(lhs.as_ref(), rhs.as_ref())
        }
        _ => false,
    }
}

fn native_const_heap_eq(lhs: &ConstHeapValue32Data, rhs: &ConstHeapValue32Data) -> bool {
    match (lhs, rhs) {
        (ConstHeapValue32Data::LongString(lhs), ConstHeapValue32Data::LongString(rhs)) => lhs == rhs,
        (ConstHeapValue32Data::List(lhs), ConstHeapValue32Data::List(rhs)) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs.iter())
                    .all(|(lhs, rhs)| native_const_runtime_eq(lhs, rhs))
        }
        (ConstHeapValue32Data::Map(lhs), ConstHeapValue32Data::Map(rhs)) => {
            lhs.len() == rhs.len()
                && lhs.iter().all(|(lhs_key, lhs_value)| {
                    rhs.iter()
                        .find(|(rhs_key, _)| rhs_key == lhs_key)
                        .is_some_and(|(_, rhs_value)| native_const_runtime_eq(lhs_value, rhs_value))
                })
        }
        (ConstHeapValue32Data::UpvalCell(lhs), ConstHeapValue32Data::UpvalCell(rhs)) => {
            native_const_runtime_eq(lhs.as_ref(), rhs.as_ref())
        }
        _ => false,
    }
}
