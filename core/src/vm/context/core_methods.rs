use std::sync::Arc;

use anyhow::{anyhow, bail};
use arcstr::ArcStr;

use crate::{
    val::{HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, Type, TypedList},
    vm::{
        NativeArgs32, NativeRuntime32, call_runtime_value32_runtime_list_args,
        call_runtime_value32_runtime_named_map_list_args, call_runtime_value32_runtime_with_receiver_list_args,
    },
};

const MAX_INLINE_METHOD_POSITIONAL_ARGS32: usize = u8::MAX as usize + 1;

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

pub(super) fn core_call_method_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        bail!("__lk_call_method expects 3 arguments: receiver, method name, positional args list");
    }
    let receiver = args.get(0).expect("arity checked").clone();
    let method = method_name_arc("__lk_call_method", args.get(1).expect("arity checked"), runtime.heap())?;
    let positional =
        runtime_positional_arg_list("__lk_call_method", args.get(2).expect("arity checked"), runtime.heap())?;
    call_method_positional_runtime(receiver, method, positional, runtime)
}

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
    let positional = runtime_positional_arg_list(
        "__lk_call_method_named",
        args.get(2).expect("arity checked"),
        runtime.heap(),
    )?;
    let named = runtime_named_arg_map(
        "__lk_call_method_named",
        args.get(3).expect("arity checked"),
        runtime.heap(),
    )?;
    call_method_named_runtime(receiver, method, positional, named, runtime)
}

fn call_method_positional_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: MethodPositionalArgs,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if let Some(prop) = runtime_access(&receiver, method.as_str(), runtime.heap_mut())? {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("__lk_call_method requires full runtime state for callable receiver");
            };
            return call_runtime_value32_runtime_list_args(prop, positional.handle(), state, module, ctx);
        }
        if positional.is_empty(runtime.heap())? {
            return Ok(prop);
        }
    }
    if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
        dispatch_map_builtin_method(&receiver, method.as_str(), positional, heap)
    })? {
        return Ok(result);
    }
    if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
        dispatch_string_builtin_method(&receiver, method.as_str(), positional, heap)
    })? {
        return Ok(result);
    }
    if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
        dispatch_list_builtin_method(&receiver, method.as_str(), positional, heap)
    })? {
        return Ok(result);
    }
    call_trait_method_runtime(receiver, method, positional, runtime)
}

fn call_method_named_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: MethodPositionalArgs,
    named: Option<HeapRef>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if let Some(prop) = runtime_access(&receiver, method.as_str(), runtime.heap_mut())? {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("__lk_call_method_named requires full runtime state for callable receiver");
            };
            return call_runtime_value32_runtime_named_map_list_args(
                prop,
                positional.handle(),
                named,
                state,
                module,
                ctx,
            );
        }
        if positional.is_empty(runtime.heap())? && named.is_none() {
            return Ok(prop);
        }
    }
    if named.is_none() {
        if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_map_builtin_method(&receiver, method.as_str(), positional, heap)
        })? {
            return Ok(result);
        }
        if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_string_builtin_method(&receiver, method.as_str(), positional, heap)
        })? {
            return Ok(result);
        }
        if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_list_builtin_method(&receiver, method.as_str(), positional, heap)
        })? {
            return Ok(result);
        }
    }
    bail!("Named arguments are not supported for trait methods")
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
            let result = match heap.get(handle) {
                Some(HeapValue::Map(map)) => map.get(&key).unwrap_or(RuntimeVal::Nil),
                _ => return Ok(Some(default)),
            };
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
            let parts = match heap.get(handle) {
                Some(HeapValue::List(list)) => list_join_parts(list, heap)?,
                _ => return Ok(None),
            };
            let joined = parts.join(sep.as_ref());
            Ok(Some(make_string_val(&joined, heap)))
        }
        _ => Ok(None),
    }
}

fn list_join_parts(list: &TypedList, heap: &HeapStore) -> anyhow::Result<Vec<String>> {
    match list {
        TypedList::String(vals) => Ok(vals.iter().map(|s| s.to_string()).collect()),
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
            .collect(),
        _ => bail!("list.join(): list must contain only strings"),
    }
}

fn call_trait_method_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: MethodPositionalArgs,
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
    call_runtime_value32_runtime_with_receiver_list_args(
        method_val,
        &receiver,
        positional.handle(),
        state,
        module,
        Some(ctx),
    )
}

fn runtime_access(receiver: &RuntimeVal, field: &str, heap: &mut HeapStore) -> anyhow::Result<Option<RuntimeVal>> {
    match receiver {
        RuntimeVal::ShortStr(value) => Ok(runtime_string_access(value.as_str(), field)),
        RuntimeVal::Obj(handle) => {
            enum RuntimeAccess32 {
                Ready(Option<RuntimeVal>),
                CopyPayload(crate::rt::RuntimePayload),
                String(String),
            }
            let access = match heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => RuntimeAccess32::Ready(runtime_string_access(value.as_ref(), field)),
                HeapValue::List(values) => RuntimeAccess32::Ready(runtime_list_access(values, field)),
                HeapValue::Map(values) => RuntimeAccess32::Ready(values.get_str(field)),
                HeapValue::Object(object) => RuntimeAccess32::Ready(object.get_field(field)),
                HeapValue::Task(task) if field == "value" => match &task.value {
                    Some(value) => RuntimeAccess32::CopyPayload(value.clone()),
                    None => RuntimeAccess32::Ready(Some(RuntimeVal::Nil)),
                },
                HeapValue::Channel(channel) => match field {
                    "capacity" => RuntimeAccess32::Ready(Some(RuntimeVal::Int(channel.capacity.unwrap_or(0) as i64))),
                    "type" => RuntimeAccess32::String(format!("{:?}", channel.inner_type)),
                    _ => RuntimeAccess32::Ready(None),
                },
                _ => RuntimeAccess32::Ready(None),
            };
            match access {
                RuntimeAccess32::Ready(value) => Ok(value),
                RuntimeAccess32::CopyPayload(value) => {
                    Ok(Some(crate::vm::copy_runtime_value(&value.value, &value.heap, heap)?))
                }
                RuntimeAccess32::String(value) => Ok(Some(runtime_string_value(value, heap))),
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

#[derive(Clone, Copy)]
enum MethodPositionalArgs {
    Empty,
    List(HeapRef),
}

impl MethodPositionalArgs {
    fn handle(self) -> Option<HeapRef> {
        match self {
            Self::Empty => None,
            Self::List(handle) => Some(handle),
        }
    }

    fn is_empty(self, heap: &HeapStore) -> anyhow::Result<bool> {
        Ok(self.len(heap)? == 0)
    }

    fn len(self, heap: &HeapStore) -> anyhow::Result<usize> {
        match self {
            Self::Empty => Ok(0),
            Self::List(handle) => match heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::List(list) => Ok(list.len()),
                other => bail!("method positional arguments must be a list, got {}", other.type_name()),
            },
        }
    }

    fn with_slice<R>(
        self,
        heap: &mut HeapStore,
        f: impl FnOnce(&[RuntimeVal], &mut HeapStore) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        match self {
            Self::Empty => f(&[], heap),
            Self::List(handle) => {
                let len = self.len(heap)?;
                if len > MAX_INLINE_METHOD_POSITIONAL_ARGS32 {
                    bail!(
                        "method positional argument count {} exceeds inline call buffer {}",
                        len,
                        MAX_INLINE_METHOD_POSITIONAL_ARGS32
                    );
                }
                let mut values: [RuntimeVal; MAX_INLINE_METHOD_POSITIONAL_ARGS32] =
                    std::array::from_fn(|_| RuntimeVal::Nil);
                copy_method_positional_list(handle, heap, &mut values[..len])?;
                f(&values[..len], heap)
            }
        }
    }
}

fn runtime_positional_arg_list(
    helper: &str,
    value: &RuntimeVal,
    heap: &HeapStore,
) -> anyhow::Result<MethodPositionalArgs> {
    let handle = match value {
        RuntimeVal::Nil => return Ok(MethodPositionalArgs::Empty),
        RuntimeVal::Obj(h) => *h,
        other => bail!("{helper} expects positional arguments as list, got {:?}", other.kind()),
    };

    let heap_val = heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::List(_) = heap_val else {
        bail!(
            "{helper} expects positional arguments as list, got {}",
            heap_val.type_name()
        );
    };
    Ok(MethodPositionalArgs::List(handle))
}

fn copy_method_positional_list(handle: HeapRef, heap: &mut HeapStore, frame: &mut [RuntimeVal]) -> anyhow::Result<()> {
    let long_string_values = match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => {
            for (slot, value) in frame.iter_mut().zip(values) {
                *slot = value.clone();
            }
            return Ok(());
        }
        HeapValue::List(TypedList::Int(values)) => {
            for (slot, &value) in frame.iter_mut().zip(values) {
                *slot = RuntimeVal::Int(value);
            }
            return Ok(());
        }
        HeapValue::List(TypedList::Float(values)) => {
            for (slot, &value) in frame.iter_mut().zip(values) {
                *slot = RuntimeVal::Float(value);
            }
            return Ok(());
        }
        HeapValue::List(TypedList::Bool(values)) => {
            for (slot, &value) in frame.iter_mut().zip(values) {
                *slot = RuntimeVal::Bool(value);
            }
            return Ok(());
        }
        HeapValue::List(TypedList::String(values)) => {
            let mut long_values = Vec::new();
            for (index, value) in values.iter().enumerate() {
                match ShortStr::new(value.as_ref()) {
                    Some(short) => frame[index] = RuntimeVal::ShortStr(short),
                    None => long_values.push((index, Arc::clone(value))),
                }
            }
            long_values
        }
        other => bail!("method positional arguments must be a list, got {}", other.type_name()),
    };
    for (index, value) in long_string_values {
        frame[index] = RuntimeVal::Obj(heap.alloc(HeapValue::String(value)));
    }
    Ok(())
}

fn runtime_named_arg_map(helper: &str, value: &RuntimeVal, heap: &HeapStore) -> anyhow::Result<Option<HeapRef>> {
    let handle = match value {
        RuntimeVal::Nil => return Ok(None),
        RuntimeVal::Obj(h) => *h,
        other => bail!("{helper} expects named arguments as map, got {:?}", other.kind()),
    };

    let heap_val = heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::Map(_) = heap_val else {
        bail!("{helper} expects named arguments as map, got {}", heap_val.type_name());
    };
    Ok(Some(handle))
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
