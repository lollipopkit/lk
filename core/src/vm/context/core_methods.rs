use std::sync::Arc;

use anyhow::{anyhow, bail};
use arcstr::ArcStr;

use crate::{
    val::{HeapStore, HeapValue, RuntimeVal, ShortStr, Type, TypedList},
    vm::{
        NativeArgs32, NativeRuntime32, call_runtime_value32_runtime, call_runtime_value32_runtime_named,
        call_runtime_value32_runtime_with_receiver,
    },
};

fn method_name_arc(helper: &str, method: &RuntimeVal, heap: &HeapStore) -> anyhow::Result<ArcStr> {
    match method {
        RuntimeVal::ShortStr(value) => Ok(ArcStr::from(value.as_str())),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => Ok(ArcStr::from(value.as_ref())),
            Some(value) => Err(anyhow!(
                "{helper} expects method name as string, got {}",
                value.type_name()
            )),
            None => Err(anyhow!("heap object {} out of bounds", handle.index())),
        },
        other => Err(anyhow!(
            "{helper} expects method name as string, got {:?}",
            other.kind()
        )),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
pub(super) fn core_call_method_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        bail!("__lk_call_method expects 3 arguments: receiver, method name, positional args list");
    }
    let receiver = args.get(0).expect("arity checked").clone();
    let method = method_name_arc("__lk_call_method", args.get(1).expect("arity checked"), runtime.heap())?;
    let positional = runtime_positional_args(
        "__lk_call_method",
        args.get(2).expect("arity checked"),
        runtime.heap_mut(),
    )?;
    call_method_positional_runtime(receiver, method, &positional, runtime)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
pub(super) fn core_call_method_named_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 4 {
        bail!(
            "__lk_call_method_named expects 4 arguments: receiver, method name, positional args list, named args map"
        );
    }
    let receiver = args.get(0).expect("arity checked").clone();
    let method = method_name_arc(
        "__lk_call_method_named",
        args.get(1).expect("arity checked"),
        runtime.heap(),
    )?;
    let positional = runtime_positional_args(
        "__lk_call_method_named",
        args.get(2).expect("arity checked"),
        runtime.heap_mut(),
    )?;
    let named = runtime_named_args(
        "__lk_call_method_named",
        args.get(3).expect("arity checked"),
        runtime.heap_mut(),
    )?;
    call_method_named_runtime(receiver, method, &positional, &named, runtime)
}

fn call_method_positional_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: &[RuntimeVal],
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if let Some(prop) = runtime_access(&receiver, method.as_str(), runtime.heap_mut())? {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("__lk_call_method requires full runtime state for callable receiver");
            };
            return call_runtime_value32_runtime(prop, positional, state, module, ctx);
        }
        if positional.is_empty() {
            return Ok(prop);
        }
    }
    call_trait_method_runtime(receiver, method, positional, &[], runtime)
}

fn call_method_named_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: &[RuntimeVal],
    named: &[(Arc<str>, RuntimeVal)],
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if let Some(prop) = runtime_access(&receiver, method.as_str(), runtime.heap_mut())? {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("__lk_call_method_named requires full runtime state for callable receiver");
            };
            return call_runtime_value32_runtime_named(prop, positional, named, state, module, ctx);
        }
        if positional.is_empty() && named.is_empty() {
            return Ok(prop);
        }
    }
    call_trait_method_runtime(receiver, method, positional, named, runtime)
}

fn call_trait_method_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: &[RuntimeVal],
    named: &[(Arc<str>, RuntimeVal)],
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    let receiver_type = runtime_dispatch_type(&receiver, runtime.heap());
    let receiver_type_name = runtime_type_name(&receiver, runtime.heap());
    let Some((state, ctx, module)) = runtime.parts_mut() else {
        bail!("{} method '{}' requires full runtime state", receiver_type_name, method);
    };
    let Some(ctx) = ctx else {
        bail!("{} has no method '{}'", receiver_type_name, method);
    };
    let Some(method_val) = ctx
        .type_checker()
        .as_ref()
        .and_then(|tc| tc.registry().get_method(&receiver_type, method.as_str()).cloned())
    else {
        bail!("{} has no method '{}'", receiver_type_name, method);
    };
    if !named.is_empty() {
        bail!("Named arguments are not supported for trait methods");
    }

    match method_val {
        crate::typ::TraitMethodValue::Runtime(callee) => {
            call_runtime_value32_runtime_with_receiver(callee, &receiver, positional, state, module, Some(ctx))
        }
        crate::typ::TraitMethodValue::Legacy(_) => {
            bail!("legacy trait methods cannot be called from Executor32")
        }
    }
}

fn runtime_access(receiver: &RuntimeVal, field: &str, heap: &mut HeapStore) -> anyhow::Result<Option<RuntimeVal>> {
    match receiver {
        RuntimeVal::ShortStr(value) => Ok(runtime_string_access(value.as_str(), field)),
        RuntimeVal::Obj(handle) => {
            let value = heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                .clone();
            match value {
                HeapValue::String(value) => Ok(runtime_string_access(value.as_ref(), field)),
                HeapValue::List(values) => Ok(runtime_list_access(&values, field)),
                HeapValue::Map(values) => values.get_str_into_heap(field, heap),
                HeapValue::Object(object) => Ok(object.fields.get(field).cloned()),
                HeapValue::Task(task) if field == "value" => {
                    let Some(value) = &task.value else {
                        return Ok(Some(RuntimeVal::Nil));
                    };
                    let mut source_heap = value.heap.clone();
                    Ok(Some(crate::vm::copy_runtime_value(
                        &value.value,
                        &mut source_heap,
                        heap,
                    )?))
                }
                HeapValue::Channel(channel) => match field {
                    "capacity" => Ok(Some(RuntimeVal::Int(channel.capacity.unwrap_or(0) as i64))),
                    "type" => Ok(Some(runtime_string_value(format!("{:?}", channel.inner_type), heap))),
                    _ => Ok(None),
                },
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

fn runtime_string_access(value: &str, field: &str) -> Option<RuntimeVal> {
    match field {
        "len" => Some(RuntimeVal::Int(value.len() as i64)),
        _ => None,
    }
}

fn runtime_list_access(values: &TypedList, field: &str) -> Option<RuntimeVal> {
    match field {
        "len" => Some(RuntimeVal::Int(values.len() as i64)),
        _ => None,
    }
}

fn runtime_positional_args(helper: &str, value: &RuntimeVal, heap: &mut HeapStore) -> anyhow::Result<Vec<RuntimeVal>> {
    match value {
        RuntimeVal::Nil => Ok(Vec::new()),
        RuntimeVal::Obj(handle) => {
            let value = heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                .clone();
            let HeapValue::List(values) = value else {
                bail!(
                    "{helper} expects positional arguments as list, got {}",
                    value.type_name()
                );
            };
            Ok(values.materialize_mixed(heap))
        }
        other => bail!("{helper} expects positional arguments as list, got {:?}", other.kind()),
    }
}

fn runtime_named_args(
    helper: &str,
    value: &RuntimeVal,
    heap: &mut HeapStore,
) -> anyhow::Result<Vec<(Arc<str>, RuntimeVal)>> {
    match value {
        RuntimeVal::Nil => Ok(Vec::new()),
        RuntimeVal::Obj(handle) => {
            let value = heap
                .get(*handle)
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            let HeapValue::Map(values) = &value else {
                bail!("{helper} expects named arguments as map, got {}", value.type_name());
            };
            values
                .string_entries_into_heap(heap)
                .map_err(|error| anyhow!("{helper} named argument key must be a string: {error}"))
        }
        other => bail!("{helper} expects named arguments as map, got {:?}", other.kind()),
    }
}

fn runtime_string_value(value: String, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(short) = ShortStr::new(&value) {
        RuntimeVal::ShortStr(short)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

fn runtime_is_callable(value: &RuntimeVal, heap: &HeapStore) -> anyhow::Result<bool> {
    let RuntimeVal::Obj(handle) = value else {
        return Ok(false);
    };
    let Some(value) = heap.get(*handle) else {
        bail!("heap object {} out of bounds", handle.index());
    };
    Ok(matches!(value, HeapValue::Callable(_)))
}

fn runtime_dispatch_type(value: &RuntimeVal, heap: &HeapStore) -> Type {
    match value {
        RuntimeVal::Nil => Type::Nil,
        RuntimeVal::Bool(_) => Type::Bool,
        RuntimeVal::Int(_) => Type::Int,
        RuntimeVal::Float(_) => Type::Float,
        RuntimeVal::ShortStr(_) => Type::String,
        RuntimeVal::Obj(handle) => heap.get(*handle).map(heap_dispatch_type).unwrap_or(Type::Any),
    }
}

fn heap_dispatch_type(value: &HeapValue) -> Type {
    match value {
        HeapValue::String(_) => Type::String,
        HeapValue::List(_) => Type::List(Box::new(Type::Any)),
        HeapValue::Map(_) => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
        HeapValue::Callable(_) => Type::Function {
            params: Vec::new(),
            named_params: Vec::new(),
            return_type: Box::new(Type::Any),
        },
        HeapValue::Task(_) => Type::Task(Box::new(Type::Any)),
        HeapValue::Channel(channel) => Type::Channel(Box::new(channel.inner_type.clone())),
        HeapValue::Stream(stream) => Type::Generic {
            name: "Stream".to_string(),
            params: vec![stream.inner_type.clone()],
        },
        HeapValue::StreamCursor(_) => Type::Named("StreamCursor".to_string()),
        HeapValue::Object(object) => Type::Named(object.type_name.to_string()),
        HeapValue::UpvalCell(_) => Type::Any,
        HeapValue::ErrorVal(_) => Type::Named("Error".to_string()),
    }
}

fn runtime_type_name(value: &RuntimeVal, heap: &HeapStore) -> &'static str {
    match value {
        RuntimeVal::Nil => "Nil",
        RuntimeVal::Bool(_) => "Bool",
        RuntimeVal::Int(_) => "Int",
        RuntimeVal::Float(_) => "Float",
        RuntimeVal::ShortStr(_) => "String",
        RuntimeVal::Obj(handle) => heap.get(*handle).map(HeapValue::type_name).unwrap_or("Object"),
    }
}
