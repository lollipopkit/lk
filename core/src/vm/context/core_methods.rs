use std::sync::Arc;

use anyhow::{anyhow, bail};
use arcstr::ArcStr;

use crate::{
    val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, Type, TypedList, TypedMap},
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
    if let Some(result) = dispatch_map_builtin_method(&receiver, method.as_str(), positional, runtime.heap_mut())? {
        return Ok(result);
    }
    if let Some(result) = dispatch_string_builtin_method(&receiver, method.as_str(), positional, runtime.heap_mut())? {
        return Ok(result);
    }
    if let Some(result) = dispatch_list_builtin_method(&receiver, method.as_str(), positional, runtime.heap_mut())? {
        return Ok(result);
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
    if named.is_empty() {
        if let Some(result) = dispatch_map_builtin_method(&receiver, method.as_str(), positional, runtime.heap_mut())? {
            return Ok(result);
        }
        if let Some(result) =
            dispatch_string_builtin_method(&receiver, method.as_str(), positional, runtime.heap_mut())?
        {
            return Ok(result);
        }
        if let Some(result) = dispatch_list_builtin_method(&receiver, method.as_str(), positional, runtime.heap_mut())?
        {
            return Ok(result);
        }
    }
    call_trait_method_runtime(receiver, method, positional, named, runtime)
}

/// Dispatch built-in map instance methods: set, get, has, len.
/// Returns Some(value) if the method was handled, None if it should fall through.
fn dispatch_map_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = receiver else {
        return Ok(None);
    };
    let handle = *handle;
    if !matches!(heap.get(handle), Some(HeapValue::Map(_))) {
        return Ok(None);
    }
    match method {
        "set" => {
            if positional.len() != 2 {
                bail!("map.set() expects 2 arguments (key, value), got {}", positional.len());
            }
            let key = map_string_key(&positional[0], heap, "map.set() key")?;
            let value = positional[1].clone();
            if let Some(HeapValue::Map(map)) = heap.get_mut(handle) {
                map.set(key, value);
            }
            Ok(Some(RuntimeVal::Nil))
        }
        "get" => {
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "map.get() expects 1 or 2 arguments (key[, default]), got {}",
                    positional.len()
                );
            }
            let key = map_string_key(&positional[0], heap, "map.get() key")?;
            let default = positional.get(1).cloned().unwrap_or(RuntimeVal::Nil);
            let map = match heap.get(handle) {
                Some(HeapValue::Map(m)) => m.clone(),
                _ => return Ok(Some(default)),
            };
            let result = map.get_into_heap(&key, heap)?.unwrap_or(RuntimeVal::Nil);
            if matches!(result, RuntimeVal::Nil) {
                Ok(Some(default))
            } else {
                Ok(Some(result))
            }
        }
        "has" => {
            if positional.len() != 1 {
                bail!("map.has() expects 1 argument (key), got {}", positional.len());
            }
            let key = map_string_key(&positional[0], heap, "map.has() key")?;
            let found = matches!(heap.get(handle), Some(HeapValue::Map(m)) if m.get(&key).is_some());
            Ok(Some(RuntimeVal::Bool(found)))
        }
        "len" => {
            if !positional.is_empty() {
                bail!("map.len() expects no arguments, got {}", positional.len());
            }
            let len = match heap.get(handle) {
                Some(HeapValue::Map(m)) => m.len(),
                _ => 0,
            };
            Ok(Some(RuntimeVal::Int(len as i64)))
        }
        _ => Ok(None),
    }
}

fn map_string_key(value: &RuntimeVal, heap: &HeapStore, context: &str) -> anyhow::Result<RuntimeMapKey> {
    match value {
        RuntimeVal::ShortStr(s) => Ok(RuntimeMapKey::String(Arc::<str>::from(s.as_str()))),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(s)) => Ok(RuntimeMapKey::String(Arc::clone(s))),
            Some(v) => bail!("{context}: expected string key, got {}", v.type_name()),
            None => bail!("{context}: heap object out of bounds"),
        },
        other => bail!("{context}: expected string key, got {:?}", other.kind()),
    }
}

/// Extract a string value from a RuntimeVal as an Arc<str> (cloned, no borrow retained).
fn extract_string_arc(value: &RuntimeVal, heap: &HeapStore, context: &str) -> anyhow::Result<Arc<str>> {
    match value {
        RuntimeVal::ShortStr(s) => Ok(Arc::<str>::from(s.as_str())),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(s)) => Ok(Arc::clone(s)),
            Some(v) => bail!("{context}: expected string, got {}", v.type_name()),
            None => bail!("{context}: heap object out of bounds"),
        },
        other => bail!("{context}: expected string, got {:?}", other.kind()),
    }
}

/// Create a RuntimeVal string (ShortStr if it fits, otherwise heap-allocated).
fn make_string_val(s: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(short) = ShortStr::new(s) {
        RuntimeVal::ShortStr(short)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(s))))
    }
}

/// Dispatch built-in string instance methods: split, starts_with, ends_with, contains, trim.
/// Returns Some(value) if handled, None to fall through.
fn dispatch_string_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    // Extract the string value as an owned Arc<str> so we can use heap mutably after.
    let s: Arc<str> = match receiver {
        RuntimeVal::ShortStr(s) => Arc::<str>::from(s.as_str()),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(arc)) => Arc::clone(arc),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    match method {
        "split" => {
            if positional.len() != 1 {
                bail!(
                    "string.split() expects 1 argument (delimiter), got {}",
                    positional.len()
                );
            }
            let delim = extract_string_arc(&positional[0], heap, "string.split() delimiter")?;
            let parts: Vec<Arc<str>> = s.split(delim.as_ref()).map(Arc::<str>::from).collect();
            let handle = heap.alloc(HeapValue::List(TypedList::String(parts)));
            Ok(Some(RuntimeVal::Obj(handle)))
        }
        "starts_with" => {
            if positional.len() != 1 {
                bail!(
                    "string.starts_with() expects 1 argument (prefix), got {}",
                    positional.len()
                );
            }
            let prefix = extract_string_arc(&positional[0], heap, "string.starts_with() prefix")?;
            Ok(Some(RuntimeVal::Bool(s.starts_with(prefix.as_ref()))))
        }
        "ends_with" => {
            if positional.len() != 1 {
                bail!(
                    "string.ends_with() expects 1 argument (suffix), got {}",
                    positional.len()
                );
            }
            let suffix = extract_string_arc(&positional[0], heap, "string.ends_with() suffix")?;
            Ok(Some(RuntimeVal::Bool(s.ends_with(suffix.as_ref()))))
        }
        "contains" => {
            if positional.len() != 1 {
                bail!(
                    "string.contains() expects 1 argument (needle), got {}",
                    positional.len()
                );
            }
            let needle = extract_string_arc(&positional[0], heap, "string.contains() needle")?;
            Ok(Some(RuntimeVal::Bool(s.contains(needle.as_ref()))))
        }
        "trim" => {
            if !positional.is_empty() {
                bail!("string.trim() expects no arguments, got {}", positional.len());
            }
            Ok(Some(make_string_val(s.trim(), heap)))
        }
        _ => Ok(None),
    }
}

/// Dispatch built-in list instance methods: join.
/// Returns Some(value) if handled, None to fall through.
fn dispatch_list_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = receiver else {
        return Ok(None);
    };
    let handle = *handle;
    if !matches!(heap.get(handle), Some(HeapValue::List(_))) {
        return Ok(None);
    }
    match method {
        "join" => {
            if positional.len() != 1 {
                bail!("list.join() expects 1 argument (separator), got {}", positional.len());
            }
            let sep = extract_string_arc(&positional[0], heap, "list.join() separator")?;
            // Clone the list to avoid borrow conflict when calling make_string_val.
            let list = match heap.get(handle) {
                Some(HeapValue::List(l)) => l.clone(),
                _ => return Ok(None),
            };
            let parts: Vec<String> = match &list {
                TypedList::String(vals) => vals.iter().map(|s| s.to_string()).collect(),
                TypedList::Mixed(vals) => vals
                    .iter()
                    .map(|v| match v {
                        RuntimeVal::ShortStr(s) => Ok(s.as_str().to_string()),
                        RuntimeVal::Obj(h) => match heap.get(*h) {
                            Some(HeapValue::String(s)) => Ok(s.to_string()),
                            Some(other) => bail!("list.join(): element is not a string ({})", other.type_name()),
                            None => bail!("list.join(): heap object out of bounds"),
                        },
                        other => bail!("list.join(): element is not a string ({:?})", other.kind()),
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
                _ => bail!("list.join(): list must contain only strings"),
            };
            let joined = parts.join(sep.as_ref());
            Ok(Some(make_string_val(&joined, heap)))
        }
        _ => Ok(None),
    }
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
    let handle = match value {
        RuntimeVal::Nil => return Ok(Vec::new()),
        RuntimeVal::Obj(h) => *h,
        other => bail!("{helper} expects positional arguments as list, got {:?}", other.kind()),
    };

    // Phase 1: immutable borrow — handles all inline typed cases without cloning HeapValue.
    {
        let heap_val = heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::List(list) = heap_val else {
            bail!(
                "{helper} expects positional arguments as list, got {}",
                heap_val.type_name()
            );
        };
        let result = match list {
            TypedList::Mixed(values) => Some(values.clone()),
            TypedList::Int(values) => Some(values.iter().copied().map(RuntimeVal::Int).collect()),
            TypedList::Float(values) => Some(values.iter().copied().map(RuntimeVal::Float).collect()),
            TypedList::Bool(values) => Some(values.iter().copied().map(RuntimeVal::Bool).collect()),
            TypedList::String(_) | TypedList::OwnedRuntime(_) => None,
        };
        if let Some(v) = result {
            return Ok(v);
        }
    }

    // Phase 2: String (may allocate long strings into heap) or OwnedRuntime (bridge).
    let list = match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        .clone()
    {
        HeapValue::List(l) => l,
        _ => unreachable!("already verified as List in phase 1"),
    };
    Ok(list.materialize_mixed(heap))
}

fn runtime_named_args(
    helper: &str,
    value: &RuntimeVal,
    heap: &mut HeapStore,
) -> anyhow::Result<Vec<(Arc<str>, RuntimeVal)>> {
    let handle = match value {
        RuntimeVal::Nil => return Ok(Vec::new()),
        RuntimeVal::Obj(h) => *h,
        other => bail!("{helper} expects named arguments as map, got {:?}", other.kind()),
    };

    // Phase 1: immutable borrow — handles all non-OwnedRuntime variants without cloning TypedMap.
    {
        let heap_val = heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::Map(map) = heap_val else {
            bail!("{helper} expects named arguments as map, got {}", heap_val.type_name());
        };
        if !matches!(map, TypedMap::OwnedRuntime(_)) {
            return map
                .string_entries_no_heap()
                .map_err(|e| anyhow!("{helper} named argument key must be a string: {e}"));
        }
    }

    // Phase 2: OwnedRuntime only — needs &mut heap to copy values across heap boundaries.
    let value = heap
        .get(handle)
        .cloned()
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::Map(map) = &value else {
        unreachable!("already verified as Map in phase 1")
    };
    map.string_entries_into_heap(heap)
        .map_err(|e| anyhow!("{helper} named argument key must be a string: {e}"))
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
