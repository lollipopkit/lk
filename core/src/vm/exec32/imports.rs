use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{CallableValue, HeapStore, HeapValue, RuntimeObject, RuntimeVal, TypedList, TypedMap};

use super::{RuntimeCallable32, runtime_value_to_callable32_shared};
use crate::vm::{Module32, RuntimeExport32};

pub fn import_runtime_export(export: &RuntimeExport32, dest_heap: &mut HeapStore) -> Result<RuntimeVal> {
    let state = export.state_lock()?;
    import_runtime_value(
        export.value(),
        &state.heap,
        dest_heap,
        export.shared_module(),
        export.shared_state(),
    )
}

fn import_runtime_value(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<RuntimeVal> {
    match value {
        RuntimeVal::Nil => Ok(RuntimeVal::Nil),
        RuntimeVal::Bool(value) => Ok(RuntimeVal::Bool(*value)),
        RuntimeVal::Int(value) => Ok(RuntimeVal::Int(*value)),
        RuntimeVal::Float(value) => Ok(RuntimeVal::Float(*value)),
        RuntimeVal::ShortStr(value) => Ok(RuntimeVal::ShortStr(*value)),
        RuntimeVal::Obj(handle) => {
            if matches!(
                source_heap.get(*handle),
                Some(HeapValue::Callable(CallableValue::Closure { .. }))
            ) {
                let callable = runtime_value_to_callable32_shared(
                    value,
                    source_heap,
                    Arc::clone(&source_module),
                    source_state.clone(),
                )
                .ok_or_else(|| anyhow!("closure import could not be materialized"))?;
                return Ok(RuntimeVal::Obj(
                    dest_heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))),
                ));
            }
            let value = source_heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            let value = import_heap_value(value, source_heap, dest_heap, source_module, source_state)?;
            Ok(RuntimeVal::Obj(dest_heap.alloc(value)))
        }
    }
}

fn import_heap_value(
    value: &HeapValue,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<HeapValue> {
    Ok(match value {
        HeapValue::String(value) => HeapValue::String(value.clone()),
        HeapValue::List(values) => HeapValue::List(import_typed_list(
            values,
            source_heap,
            dest_heap,
            source_module,
            source_state,
        )?),
        HeapValue::Map(values) => HeapValue::Map(import_typed_map(
            values,
            source_heap,
            dest_heap,
            source_module,
            source_state,
        )?),
        HeapValue::Object(object) => {
            let mut fields = std::collections::BTreeMap::new();
            for (key, value) in &object.fields {
                fields.insert(
                    Arc::clone(key),
                    import_runtime_value(
                        value,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )?,
                );
            }
            HeapValue::Object(RuntimeObject::new(object.type_name.clone(), fields))
        }
        HeapValue::Callable(CallableValue::RuntimeNative32 { name, arity, function }) => {
            HeapValue::Callable(CallableValue::RuntimeNative32 {
                name: name.clone(),
                arity: *arity,
                function: function.clone(),
            })
        }
        HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) => {
            let callable = RuntimeCallable32::with_shared_captures(
                Arc::clone(&source_module),
                *function_index,
                Arc::clone(captures),
                source_state,
            );
            HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))
        }
        HeapValue::Callable(CallableValue::Runtime32(function)) => {
            HeapValue::Callable(CallableValue::Runtime32(Arc::clone(function)))
        }
        HeapValue::Task(value) => HeapValue::Task(Arc::clone(value)),
        HeapValue::Channel(value) => HeapValue::Channel(Arc::clone(value)),
        HeapValue::Stream(value) => HeapValue::Stream(Arc::clone(value)),
        HeapValue::StreamCursor(value) => HeapValue::StreamCursor(Arc::clone(value)),
        HeapValue::UpvalCell(value) => HeapValue::UpvalCell(import_runtime_value(
            value,
            source_heap,
            dest_heap,
            source_module,
            source_state,
        )?),
        HeapValue::ErrorVal(error) => HeapValue::ErrorVal(crate::val::ErrorVal {
            message: error.message.clone(),
            trace: {
                let mut trace = Vec::with_capacity(error.trace.len());
                for value in &error.trace {
                    trace.push(import_runtime_value(
                        value,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )?);
                }
                trace
            },
        }),
    })
}

fn import_typed_list(
    values: &TypedList,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<TypedList> {
    Ok(match values {
        TypedList::Mixed(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(import_runtime_value(
                    value,
                    source_heap,
                    dest_heap,
                    Arc::clone(&source_module),
                    source_state.clone(),
                )?);
            }
            TypedList::Mixed(out)
        }
        TypedList::Int(values) => TypedList::Int(copy_slice(values)),
        TypedList::Float(values) => TypedList::Float(copy_slice(values)),
        TypedList::Bool(values) => TypedList::Bool(copy_slice(values)),
        TypedList::String(values) => TypedList::String(copy_slice(values)),
    })
}

fn import_typed_map(
    values: &TypedMap,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => {
            let mut out = std::collections::BTreeMap::new();
            for (key, value) in values {
                out.insert(
                    import_runtime_map_key(
                        key,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )?,
                    import_runtime_value(
                        value,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )?,
                );
            }
            TypedMap::Mixed(out)
        }
        TypedMap::StringMixed(values) => {
            let mut out = std::collections::BTreeMap::new();
            for (key, value) in values {
                out.insert(
                    Arc::clone(key),
                    import_runtime_value(
                        value,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )?,
                );
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(values) => TypedMap::StringInt(copy_string_map_values(values)),
        TypedMap::StringFloat(values) => TypedMap::StringFloat(copy_string_map_values(values)),
        TypedMap::StringBool(values) => TypedMap::StringBool(copy_string_map_values(values)),
    })
}

fn import_runtime_map_key(
    key: &crate::val::RuntimeMapKey,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<crate::val::RuntimeMapKey> {
    Ok(match key {
        crate::val::RuntimeMapKey::Nil => crate::val::RuntimeMapKey::Nil,
        crate::val::RuntimeMapKey::Bool(value) => crate::val::RuntimeMapKey::Bool(*value),
        crate::val::RuntimeMapKey::Int(value) => crate::val::RuntimeMapKey::Int(*value),
        crate::val::RuntimeMapKey::ShortStr(value) => crate::val::RuntimeMapKey::ShortStr(*value),
        crate::val::RuntimeMapKey::String(value) => crate::val::RuntimeMapKey::String(Arc::clone(value)),
        crate::val::RuntimeMapKey::Obj(handle) => {
            match import_runtime_value(
                &RuntimeVal::Obj(*handle),
                source_heap,
                dest_heap,
                source_module,
                source_state,
            )? {
                RuntimeVal::Obj(handle) => crate::val::RuntimeMapKey::Obj(handle),
                _ => unreachable!("object map key import must stay an object"),
            }
        }
    })
}

fn copy_slice<T: Clone>(values: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(values);
    out
}

fn copy_string_map_values<T: Copy>(
    values: &std::collections::BTreeMap<Arc<str>, T>,
) -> std::collections::BTreeMap<Arc<str>, T> {
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in values {
        out.insert(Arc::clone(key), *value);
    }
    out
}
