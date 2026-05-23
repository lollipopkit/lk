use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{CallableValue, HeapStore, HeapValue, RuntimeObject, RuntimeVal, TypedList, TypedMap};

use super::{RuntimeCallable32, runtime_value_to_callable32_shared};
use crate::vm::{Module32, RuntimeExport32};

pub fn import_runtime_export(export: &RuntimeExport32, dest_heap: &mut HeapStore) -> Result<RuntimeVal> {
    let state = export
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeExport32 state lock poisoned"))?;
    import_runtime_value(
        &export.value,
        &state.heap,
        dest_heap,
        Arc::clone(&export.module),
        export.state.clone(),
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
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                .clone();
            let value = import_heap_value(value, source_heap, dest_heap, source_module, source_state)?;
            Ok(RuntimeVal::Obj(dest_heap.alloc(value)))
        }
    }
}

fn import_heap_value(
    value: HeapValue,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<HeapValue> {
    Ok(match value {
        HeapValue::String(value) => HeapValue::String(value),
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
            let fields = object
                .fields
                .iter()
                .map(|(key, value)| {
                    Ok((
                        key.clone(),
                        import_runtime_value(
                            value,
                            source_heap,
                            dest_heap,
                            Arc::clone(&source_module),
                            source_state.clone(),
                        )?,
                    ))
                })
                .collect::<Result<_>>()?;
            HeapValue::Object(RuntimeObject {
                type_name: object.type_name,
                fields,
            })
        }
        HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) => {
            HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function })
        }
        HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) => {
            let callable =
                RuntimeCallable32::with_state(Arc::clone(&source_module), function_index, captures, source_state);
            HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))
        }
        HeapValue::Callable(CallableValue::Runtime32(function)) => {
            HeapValue::Callable(CallableValue::Runtime32(function))
        }
        HeapValue::Task(value) => HeapValue::Task(value),
        HeapValue::Channel(value) => HeapValue::Channel(value),
        HeapValue::Stream(value) => HeapValue::Stream(value),
        HeapValue::StreamCursor(value) => HeapValue::StreamCursor(value),
        HeapValue::UpvalCell(value) => HeapValue::UpvalCell(import_runtime_value(
            &value,
            source_heap,
            dest_heap,
            source_module,
            source_state,
        )?),
        HeapValue::ErrorVal(error) => HeapValue::ErrorVal(crate::val::ErrorVal {
            message: error.message,
            trace: error
                .trace
                .iter()
                .map(|value| {
                    import_runtime_value(
                        value,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )
                })
                .collect::<Result<_>>()?,
        }),
    })
}

fn import_typed_list(
    values: TypedList,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<TypedList> {
    Ok(match values {
        TypedList::Mixed(values) => TypedList::Mixed(
            values
                .iter()
                .map(|value| {
                    import_runtime_value(
                        value,
                        source_heap,
                        dest_heap,
                        Arc::clone(&source_module),
                        source_state.clone(),
                    )
                })
                .collect::<Result<_>>()?,
        ),
        TypedList::Int(values) => TypedList::Int(values),
        TypedList::Float(values) => TypedList::Float(values),
        TypedList::Bool(values) => TypedList::Bool(values),
        TypedList::String(values) => TypedList::String(values),
    })
}

fn import_typed_map(
    values: TypedMap,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    source_module: Arc<Module32>,
    source_state: std::sync::Arc<std::sync::Mutex<crate::vm::RuntimeModuleState32>>,
) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => TypedMap::Mixed(
            values
                .iter()
                .map(|(key, value)| {
                    Ok((
                        key.clone(),
                        import_runtime_value(
                            value,
                            source_heap,
                            dest_heap,
                            Arc::clone(&source_module),
                            source_state.clone(),
                        )?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
        TypedMap::StringMixed(values) => TypedMap::StringMixed(
            values
                .iter()
                .map(|(key, value)| {
                    Ok((
                        key.clone(),
                        import_runtime_value(
                            value,
                            source_heap,
                            dest_heap,
                            Arc::clone(&source_module),
                            source_state.clone(),
                        )?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
        TypedMap::StringInt(values) => TypedMap::StringInt(values),
        TypedMap::StringFloat(values) => TypedMap::StringFloat(values),
        TypedMap::StringBool(values) => TypedMap::StringBool(values),
    })
}
