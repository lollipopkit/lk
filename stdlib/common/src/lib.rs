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

    let mut ints = Vec::with_capacity(values.len());
    let mut floats = Vec::with_capacity(values.len());
    let mut bools = Vec::with_capacity(values.len());
    let mut strings = Vec::with_capacity(values.len());
    for value in &values {
        match value {
            RuntimeVal::Int(value) if floats.is_empty() && bools.is_empty() && strings.is_empty() => {
                ints.push(*value);
            }
            RuntimeVal::Float(value) if ints.is_empty() && bools.is_empty() && strings.is_empty() => {
                floats.push(*value);
            }
            RuntimeVal::Bool(value) if ints.is_empty() && floats.is_empty() && strings.is_empty() => {
                bools.push(*value);
            }
            RuntimeVal::ShortStr(value) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                strings.push(Arc::<str>::from(value.as_str()));
            }
            RuntimeVal::Obj(handle) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                let Some(HeapValue::String(value)) = heap.get(*handle) else {
                    return TypedList::Mixed(values);
                };
                strings.push(value.clone());
            }
            _ => return TypedList::Mixed(values),
        }
    }

    if !ints.is_empty() {
        TypedList::Int(ints)
    } else if !floats.is_empty() {
        TypedList::Float(floats)
    } else if !bools.is_empty() {
        TypedList::Bool(bools)
    } else {
        TypedList::String(strings)
    }
}

pub fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = val::ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}
