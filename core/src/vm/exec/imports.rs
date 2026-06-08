use crate::util::fast_map::{FastHashMap, fast_hash_map_new};
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{CallableValue, HeapStore, HeapValue, RuntimeObject, RuntimeVal, TypedList, TypedMap};

use super::{RuntimeCallable, runtime_value_to_callable_shared};
use crate::vm::{Module, RuntimeExport};

pub fn import_runtime_export(export: &RuntimeExport, dest_heap: &mut HeapStore) -> Result<RuntimeVal> {
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
    source_module: Arc<Module>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState>>,
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
                let callable = runtime_value_to_callable_shared(
                    value,
                    source_heap,
                    Arc::clone(&source_module),
                    source_state.clone(),
                )
                .ok_or_else(|| anyhow!("closure use could not be materialized"))?;
                return Ok(RuntimeVal::Obj(
                    dest_heap.alloc(HeapValue::Callable(CallableValue::Runtime(Arc::new(callable)))),
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
    source_module: Arc<Module>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState>>,
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
            let mut fields = fast_hash_map_new();
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
        HeapValue::Callable(CallableValue::RuntimeNative { name, arity, function }) => {
            HeapValue::Callable(CallableValue::RuntimeNative {
                name: name.clone(),
                arity: *arity,
                function: function.clone(),
            })
        }
        HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) => {
            let callable = RuntimeCallable::with_shared_captures(
                Arc::clone(&source_module),
                *function_index,
                Arc::clone(captures),
                source_state,
            );
            HeapValue::Callable(CallableValue::Runtime(Arc::new(callable)))
        }
        HeapValue::Callable(CallableValue::Runtime(function)) => {
            HeapValue::Callable(CallableValue::Runtime(Arc::clone(function)))
        }
        HeapValue::Task(value) => HeapValue::Task(Arc::clone(value)),
        HeapValue::Channel(value) => HeapValue::Channel(Arc::clone(value)),
        HeapValue::Stream(value) => HeapValue::Stream(Arc::clone(value)),
        HeapValue::StreamCursor(value) => HeapValue::StreamCursor(Arc::clone(value)),
        HeapValue::Slice(value) => HeapValue::Slice(Arc::new(crate::val::SliceValue {
            source: import_runtime_value(
                &value.source,
                source_heap,
                dest_heap,
                source_module,
                source_state.clone(),
            )?,
            kind: value.kind,
            start: value.start,
            len: value.len,
        })),
        HeapValue::Resource(value) => HeapValue::Resource(Arc::clone(value)),
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
    source_module: Arc<Module>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState>>,
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
    source_module: Arc<Module>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState>>,
) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => {
            let mut out = fast_hash_map_new();
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
            let mut out = fast_hash_map_new();
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
    source_module: Arc<Module>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState>>,
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
                _ => unreachable!("object map key use must stay an object"),
            }
        }
    })
}

fn copy_slice<T: Clone>(values: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(values);
    out
}

fn copy_string_map_values<T: Copy>(values: &FastHashMap<Arc<str>, T>) -> FastHashMap<Arc<str>, T> {
    let mut out = fast_hash_map_new();
    for (key, value) in values {
        out.insert(Arc::clone(key), *value);
    }
    out
}
