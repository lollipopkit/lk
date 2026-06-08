pub mod resource;
pub mod runtime_native;

use lk_core::{
    val,
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
};
use std::sync::Arc;

pub fn typed_list_from_values(values: Vec<RuntimeVal>, heap: &HeapStore) -> TypedList {
    if values.is_empty() {
        return TypedList::Mixed(values);
    }

    let mut ints: Option<Vec<i64>> = None;
    let mut floats: Option<Vec<f64>> = None;
    let mut bools: Option<Vec<bool>> = None;
    let mut strings: Option<Vec<Arc<str>>> = None;
    for value in &values {
        match value {
            RuntimeVal::Int(value) if floats.is_none() && bools.is_none() && strings.is_none() => {
                ints.get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(*value);
            }
            RuntimeVal::Float(value) if ints.is_none() && bools.is_none() && strings.is_none() => {
                floats
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(*value);
            }
            RuntimeVal::Bool(value) if ints.is_none() && floats.is_none() && strings.is_none() => {
                bools
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(*value);
            }
            RuntimeVal::ShortStr(value) if ints.is_none() && floats.is_none() && bools.is_none() => {
                strings
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(Arc::<str>::from(value.as_str()));
            }
            RuntimeVal::Obj(handle) if ints.is_none() && floats.is_none() && bools.is_none() => {
                let Some(HeapValue::String(value)) = heap.get(*handle) else {
                    return TypedList::Mixed(values);
                };
                strings
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(value.clone());
            }
            _ => return TypedList::Mixed(values),
        }
    }

    if let Some(ints) = ints {
        TypedList::Int(ints)
    } else if let Some(floats) = floats {
        TypedList::Float(floats)
    } else if let Some(bools) = bools {
        TypedList::Bool(bools)
    } else if let Some(strings) = strings {
        TypedList::String(strings)
    } else {
        TypedList::Mixed(values)
    }
}

pub fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = val::ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}
