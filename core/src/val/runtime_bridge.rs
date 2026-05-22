use std::collections::HashMap;

use anyhow::{Result, bail};

use super::{CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap, Val};

pub fn val_to_runtime_val(value: &Val, heap: &mut HeapStore) -> Result<RuntimeVal> {
    match value {
        Val::Nil => Ok(RuntimeVal::Nil),
        Val::Bool(value) => Ok(RuntimeVal::Bool(*value)),
        Val::Int(value) => Ok(RuntimeVal::Int(*value)),
        Val::Float(value) => Ok(RuntimeVal::Float(*value)),
        Val::ShortStr(value) => Ok(RuntimeVal::ShortStr(*value)),
        value if value.as_list().is_some() => {
            let values = value.as_list().expect("checked list");
            let values = values
                .iter()
                .map(|value| val_to_runtime_val(value, heap))
                .collect::<Result<Vec<_>>>()?;
            Ok(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))),
            ))
        }
        value if value.as_map().is_some() => {
            let values = value.as_map().expect("checked map");
            let mut entries = std::collections::BTreeMap::new();
            for (key, value) in values.iter() {
                entries.insert(
                    RuntimeMapKey::String(key.as_str().into()),
                    val_to_runtime_val(value, heap)?,
                );
            }
            Ok(RuntimeVal::Obj(
                heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
            ))
        }
        Val::Obj(value) => match value.as_ref() {
            HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) => Ok(RuntimeVal::Obj(heap.alloc(
                HeapValue::Callable(CallableValue::RuntimeNative32 {
                    arity: *arity,
                    function: function.clone(),
                }),
            ))),
            HeapValue::Callable(CallableValue::Aot(_)) => {
                bail!("cannot convert native legacy callable to RuntimeVal without a native table")
            }
            value => Ok(RuntimeVal::Obj(heap.alloc(value.clone()))),
        },
    }
}

pub fn runtime_val_to_val(value: &RuntimeVal, heap: &HeapStore) -> Result<Val> {
    match value {
        RuntimeVal::Nil => Ok(Val::Nil),
        RuntimeVal::Bool(value) => Ok(Val::Bool(*value)),
        RuntimeVal::Int(value) => Ok(Val::Int(*value)),
        RuntimeVal::Float(value) => Ok(Val::Float(*value)),
        RuntimeVal::ShortStr(value) => Ok(Val::from(value.as_str())),
        RuntimeVal::Obj(handle) => {
            let value = heap
                .get(*handle)
                .ok_or_else(|| anyhow::anyhow!("heap object {} out of bounds", handle.index()))?;
            heap_value_to_val(value, heap)
        }
    }
}

fn heap_value_to_val(value: &HeapValue, heap: &HeapStore) -> Result<Val> {
    match value {
        HeapValue::String(value) => Ok(Val::from(value.as_ref())),
        HeapValue::List(values) => typed_list_to_val(values, heap),
        HeapValue::Map(values) => typed_map_to_val(values, heap),
        HeapValue::Object(object) => {
            let mut fields = HashMap::with_capacity(object.fields.len());
            for (key, value) in &object.fields {
                fields.insert(key.to_string(), runtime_val_to_val(value, heap)?);
            }
            Ok(Val::object(object.type_name.as_ref(), fields))
        }
        HeapValue::Callable(CallableValue::Aot(function)) => Ok(Val::aot_function(*function)),
        HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) => {
            Ok(Val::runtime_native32(function.clone(), *arity))
        }
        HeapValue::Callable(CallableValue::Runtime32(value)) => Ok(Val::runtime_callable32(value.clone())),
        HeapValue::Callable(_) => bail!("cannot convert RuntimeVal callable to legacy Val"),
        HeapValue::Task(value) => Ok(Val::task(value.clone())),
        HeapValue::Channel(value) => Ok(Val::channel(value.clone())),
        HeapValue::Stream(value) => Ok(Val::stream(value.clone())),
        HeapValue::StreamCursor(value) => Ok(Val::stream_cursor(value.clone())),
        HeapValue::UpvalCell(value) => runtime_val_to_val(value, heap),
        HeapValue::ErrorVal(error) => {
            let mut fields = HashMap::with_capacity(2);
            fields.insert("message".to_string(), Val::from(error.message.as_ref()));
            fields.insert(
                "trace".to_string(),
                Val::from(
                    error
                        .trace
                        .iter()
                        .map(|value| runtime_val_to_val(value, heap))
                        .collect::<Result<Vec<_>>>()?,
                ),
            );
            Ok(Val::object("Error", fields))
        }
    }
}

fn typed_list_to_val(values: &TypedList, heap: &HeapStore) -> Result<Val> {
    let values = match values {
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| runtime_val_to_val(value, heap))
            .collect::<Result<Vec<_>>>()?,
        TypedList::Int(values) => values.iter().copied().map(Val::Int).collect(),
        TypedList::Float(values) => values.iter().copied().map(Val::Float).collect(),
        TypedList::Bool(values) => values.iter().copied().map(Val::Bool).collect(),
        TypedList::String(values) => values.iter().map(|value| Val::from(value.as_ref())).collect(),
    };
    Ok(Val::from(values))
}

fn typed_map_to_val(values: &TypedMap, heap: &HeapStore) -> Result<Val> {
    let mut out = HashMap::with_capacity(values.len());
    for (key, value) in values.entries() {
        let Some(key) = runtime_key_to_string(&key) else {
            bail!("cannot convert non-string RuntimeMapKey to legacy Val map");
        };
        out.insert(key, runtime_val_to_val(&value, heap)?);
    }
    Ok(Val::from(out))
}

fn runtime_key_to_string(key: &RuntimeMapKey) -> Option<String> {
    match key {
        RuntimeMapKey::ShortStr(value) => Some(value.as_str().to_string()),
        RuntimeMapKey::String(value) => Some(value.to_string()),
        RuntimeMapKey::Nil | RuntimeMapKey::Bool(_) | RuntimeMapKey::Int(_) | RuntimeMapKey::Obj(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use super::*;
    use crate::val::{RuntimeObject, ShortStr};

    #[test]
    fn runtime_val_to_val_converts_typed_containers_and_objects() {
        let mut heap = HeapStore::new();
        let map = heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([(
            Arc::<str>::from("answer"),
            42,
        )]))));
        let object = heap.alloc(HeapValue::Object(RuntimeObject {
            type_name: Arc::<str>::from("Box"),
            fields: BTreeMap::from([(Arc::<str>::from("value"), RuntimeVal::Int(42))]),
        }));

        let value = runtime_val_to_val(&RuntimeVal::Obj(object), &heap).expect("convert object");
        let Some(object) = value.as_object() else {
            panic!("expected object");
        };
        assert_eq!(&object.type_name.to_string(), "Box");
        assert_eq!(
            object.fields.get("value").map(Val::object_field_to_val),
            Some(Val::Int(42))
        );

        let value = runtime_val_to_val(&RuntimeVal::Obj(map), &heap).expect("convert map");
        assert!(matches!(value.as_map(), Some(values) if values.get("answer") == Some(&Val::Int(42))));
    }

    #[test]
    fn runtime_val_to_val_rejects_non_string_map_keys() {
        let mut heap = HeapStore::new();
        let map = heap.alloc(HeapValue::Map(TypedMap::Mixed(BTreeMap::from([(
            RuntimeMapKey::Int(1),
            RuntimeVal::ShortStr(ShortStr::new("x").expect("short")),
        )]))));

        let err = runtime_val_to_val(&RuntimeVal::Obj(map), &heap).expect_err("non-string key");

        assert!(err.to_string().contains("non-string RuntimeMapKey"));
    }

    #[test]
    fn val_to_runtime_val_converts_legacy_data_values() {
        let mut heap = HeapStore::new();
        let value = Val::from(HashMap::from([("items", Val::from(vec![Val::Int(40), Val::Int(2)]))]));

        let runtime = val_to_runtime_val(&value, &mut heap).expect("convert to runtime");
        let round_trip = runtime_val_to_val(&runtime, &heap).expect("convert to legacy");

        assert_eq!(round_trip, value);
    }
}
