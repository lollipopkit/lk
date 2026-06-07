use crate::{
    llvm::const_display::{native_const_list_display, native_const_map_display},
    vm::{ConstHeapValueData, ConstRuntimeValueData},
};

use super::{
    NativeStraightlineValue, native_display_map_entries_are_string_keyed, native_map_entry_keys_match, native_map_key,
    native_map_keys_match, native_runtime_const_value,
};

pub(in crate::llvm) fn native_static_map_from_pairs(
    pairs: &[(NativeStraightlineValue, NativeStraightlineValue)],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let entries = pairs
        .iter()
        .map(|(key, value)| Some((native_map_key(key.clone())?, native_runtime_const_value(value)?)))
        .collect::<Option<Vec<_>>>();
    if let Some(entries) = entries {
        return Some(NativeStraightlineValue::Map {
            value: native_const_map_display(&entries)?,
            symbol,
            entries,
        });
    }

    let entries = pairs
        .iter()
        .map(|(key, value)| Some((native_map_key(key.clone())?, value.clone())))
        .collect::<Option<Vec<_>>>()?;
    Some(NativeStraightlineValue::DisplayMap { symbol, entries })
}

pub(in crate::llvm) fn native_static_map_rest(
    target: NativeStraightlineValue,
    removed_keys: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let removed_keys = removed_keys
        .iter()
        .cloned()
        .map(native_map_key)
        .collect::<Option<Vec<_>>>()?;
    match target {
        NativeStraightlineValue::Map { entries, .. } => {
            let entries = entries
                .into_iter()
                .filter(|(key, _)| !removed_keys.iter().any(|removed| native_map_keys_match(key, removed)))
                .collect::<Vec<_>>();
            Some(NativeStraightlineValue::Map {
                value: native_const_map_display(&entries)?,
                symbol,
                entries,
            })
        }
        NativeStraightlineValue::DisplayMap { entries, .. } => {
            let entries = entries
                .into_iter()
                .filter(|(key, _)| !removed_keys.iter().any(|removed| native_map_keys_match(key, removed)))
                .collect::<Vec<_>>();
            Some(NativeStraightlineValue::DisplayMap { symbol, entries })
        }
        _ => None,
    }
}

pub(in crate::llvm) fn native_static_map_delete(
    target: NativeStraightlineValue,
    key: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    match target {
        NativeStraightlineValue::Map { mut entries, .. } => {
            let key = native_map_key(key)?;
            let compare_string_keys = super::native_map_entries_are_string_keyed(&entries);
            let mut removed = ConstRuntimeValueData::Nil;
            entries.retain(|(entry_key, value)| {
                if native_map_entry_keys_match(entry_key, &key, compare_string_keys) {
                    removed = value.clone();
                    false
                } else {
                    true
                }
            });
            let updated = ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::Map(entries)));
            let elements = vec![updated, removed];
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        NativeStraightlineValue::DisplayMap { mut entries, .. } => {
            let key = native_map_key(key)?;
            let compare_string_keys = native_display_map_entries_are_string_keyed(&entries);
            let mut removed = NativeStraightlineValue::Nil;
            entries.retain(|(entry_key, value)| {
                if native_map_entry_keys_match(entry_key, &key, compare_string_keys) {
                    removed = value.clone();
                    false
                } else {
                    true
                }
            });
            Some(NativeStraightlineValue::ArgList {
                elements: vec![NativeStraightlineValue::DisplayMap { symbol, entries }, removed],
            })
        }
        _ => None,
    }
}
