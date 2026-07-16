#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{anyhow, bail};
use arcstr::ArcStr;

mod list_dispatch;
use self::list_dispatch::*;

use crate::{
    val::{HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, ShortStr, Type, TypedList},
    vm::{
        NativeArgs, NativeRuntime, call_runtime_value_runtime_list_args,
        call_runtime_value_runtime_named_map_list_args, call_runtime_value_runtime_with_receiver_list_args,
    },
};

const MAX_INLINE_METHOD_POSITIONAL_ARGS: usize = u8::MAX as usize + 1;

/// A string value detached from the heap borrow without copying its bytes: a
/// `ShortStr` is `Copy` (inline), a heap string keeps its `Arc` (refcount
/// clone). Method dispatch runs for every `x.method(…)` call, so the method
/// name, string receiver, and string arguments all use this instead of
/// materializing a fresh `Arc`/`ArcStr` per call.
#[derive(Clone)]
enum DetachedStr {
    Short(ShortStr),
    Heap(Arc<str>),
}

impl DetachedStr {
    fn as_str(&self) -> &str {
        match self {
            DetachedStr::Short(value) => value.as_str(),
            DetachedStr::Heap(value) => value,
        }
    }
}

fn method_name_detached(helper: &str, method: &RuntimeVal, heap: &HeapStore) -> anyhow::Result<DetachedStr> {
    match method {
        RuntimeVal::ShortStr(value) => Ok(DetachedStr::Short(*value)),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => Ok(DetachedStr::Heap(Arc::clone(value))),
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

pub(super) fn core_call_method_builtin(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        bail!("__lk_call_method expects 3 arguments: receiver, method name, positional args list");
    }
    let receiver = *args.get(0).expect("arity checked");
    let method = method_name_detached("__lk_call_method", args.get(1).expect("arity checked"), runtime.heap())?;
    let positional =
        runtime_positional_arg_list("__lk_call_method", args.get(2).expect("arity checked"), runtime.heap())?;
    call_method_positional_runtime(receiver, method, positional, runtime)
}

pub(super) fn core_call_method_named_builtin(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 4 {
        bail!(
            "__lk_call_method_named expects 4 arguments: receiver, method name, positional args list, named args map"
        );
    }
    let receiver = *args.get(0).expect("arity checked");
    let method = method_name_detached(
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

/// Which builtin-method dispatcher can possibly handle a receiver. The four
/// dispatchers are mutually exclusive on receiver type, so dispatch probes
/// exactly one instead of trying each in turn (every probe copies the
/// positional args out of the heap list).
enum BuiltinReceiver {
    Map,
    Set,
    Str,
    List,
    Other,
}

fn builtin_receiver_kind(receiver: &RuntimeVal, heap: &HeapStore) -> BuiltinReceiver {
    match receiver {
        RuntimeVal::ShortStr(_) => BuiltinReceiver::Str,
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::Map(_)) => BuiltinReceiver::Map,
            Some(HeapValue::Set(_)) => BuiltinReceiver::Set,
            Some(HeapValue::String(_)) => BuiltinReceiver::Str,
            Some(HeapValue::List(_)) => BuiltinReceiver::List,
            _ => BuiltinReceiver::Other,
        },
        _ => BuiltinReceiver::Other,
    }
}

fn dispatch_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: MethodPositionalArgs,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<Option<RuntimeVal>> {
    match builtin_receiver_kind(receiver, runtime.heap()) {
        BuiltinReceiver::Map => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_map_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Set => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_set_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Str => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_string_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::List => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_list_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Other => Ok(None),
    }
}

/// List higher-order methods that need the full runtime (they call back into
/// user code); kept in one place so every dispatch site agrees.
fn is_list_hof(method: &str) -> bool {
    matches!(method, "filter" | "map" | "reduce")
}

/// `CallMethodK` entry: dispatches a positional method call whose arguments
/// live in a register window (no boxed argument list). The hot builtin paths
/// consume the slice directly; only the rare tails (callable property, list
/// HOF, trait method) materialize a heap list, which the generic
/// `__lk_call_method` shape would have allocated anyway.
pub(crate) fn core_call_method_windowed(
    receiver: RuntimeVal,
    method_name: &str,
    args: &[RuntimeVal],
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<RuntimeVal> {
    if !is_list_hof(method_name)
        && let Some(prop) = runtime_access(&receiver, method_name, runtime.heap_mut())?
    {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("method call requires full runtime state for callable receiver");
            };
            let handle = materialize_positional_list(args, &mut state.heap);
            return call_runtime_value_runtime_list_args(prop, handle, state, module, ctx);
        }
        if args.is_empty() {
            return Ok(prop);
        }
    }
    if let Some(result) = dispatch_builtin_method_slice(&receiver, method_name, args, runtime)? {
        return Ok(result);
    }
    // Rare tails share the list-shaped generic path.
    let positional = match materialize_positional_list(args, runtime.heap_mut()) {
        Some(handle) => MethodPositionalArgs::List(handle),
        None => MethodPositionalArgs::Empty,
    };
    if is_list_hof(method_name) {
        let name = DetachedStr::Short(ShortStr::new(method_name).expect("method names are short"));
        return call_method_positional_runtime(receiver, name, positional, runtime);
    }
    call_trait_method_runtime(receiver, ArcStr::from(method_name), positional, runtime)
}

/// Boxes window arguments into a heap list for the generic method paths
/// (`None` for an empty window, matching `MethodPositionalArgs::Empty`).
fn materialize_positional_list(args: &[RuntimeVal], heap: &mut HeapStore) -> Option<HeapRef> {
    if args.is_empty() {
        return None;
    }
    Some(heap.alloc(HeapValue::List(TypedList::Mixed(args.to_vec()))))
}

/// [`dispatch_builtin_method`] over a direct argument slice (no
/// `MethodPositionalArgs` copy).
fn dispatch_builtin_method_slice(
    receiver: &RuntimeVal,
    method: &str,
    args: &[RuntimeVal],
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<Option<RuntimeVal>> {
    match builtin_receiver_kind(receiver, runtime.heap()) {
        BuiltinReceiver::Map => dispatch_map_builtin_method(receiver, method, args, runtime.heap_mut()),
        BuiltinReceiver::Set => dispatch_set_builtin_method(receiver, method, args, runtime.heap_mut()),
        BuiltinReceiver::Str => dispatch_string_builtin_method(receiver, method, args, runtime.heap_mut()),
        BuiltinReceiver::List => dispatch_list_builtin_method(receiver, method, args, runtime.heap_mut()),
        BuiltinReceiver::Other => Ok(None),
    }
}

fn call_method_positional_runtime(
    receiver: RuntimeVal,
    method: DetachedStr,
    positional: MethodPositionalArgs,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<RuntimeVal> {
    // Try dispatch for methods that need runtime state BEFORE heap closure
    let method_str = method.as_str();
    if is_list_hof(method_str) {
        // Check if receiver is a list
        let is_list = match &receiver {
            RuntimeVal::Obj(h) => matches!(runtime.heap().get(*h), Some(HeapValue::List(_))),
            _ => false,
        };
        if is_list {
            let list = clone_list(&receiver, runtime.heap_mut())?;
            let items: Vec<RuntimeVal> = list_runtime_items(list, runtime.heap_mut());
            let pos_args: Vec<RuntimeVal> = match &positional {
                MethodPositionalArgs::Empty => vec![],
                MethodPositionalArgs::List(handle) => match runtime.heap().get(*handle) {
                    Some(HeapValue::List(list)) => list.collect_owned(),
                    _ => vec![],
                },
            };
            if let Some((state, mut ctx, module)) = runtime.parts_mut() {
                // `items` may hold heap objects materialized off the receiver
                // (e.g. long strings) that nothing else references — pin them
                // for the duration of the callback loop or a GC inside the
                // callback frees them mid-iteration.
                let mark = state.host_roots_mark();
                state.host_roots_extend(items.iter());
                let result = match method_str {
                    "filter" => list_filter(&items, &pos_args, state, module, &mut ctx),
                    "map" => list_map(&items, &pos_args, state, module, &mut ctx),
                    "reduce" => list_reduce(&items, &pos_args, state, module, &mut ctx),
                    _ => Ok(None),
                };
                state.host_roots_truncate(mark);
                if let Some(r) = result? {
                    return Ok(r);
                }
            }
            return call_trait_method_runtime(receiver, ArcStr::from(method.as_str()), positional, runtime);
        }
    }
    if let Some(prop) = runtime_access(&receiver, method_str, runtime.heap_mut())? {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("__lk_call_method requires full runtime state for callable receiver");
            };
            return call_runtime_value_runtime_list_args(prop, positional.handle(), state, module, ctx);
        }
        if positional.is_empty(runtime.heap())? {
            return Ok(prop);
        }
    }
    if let Some(result) = dispatch_builtin_method(&receiver, method_str, positional, runtime)? {
        return Ok(result);
    }
    call_trait_method_runtime(receiver, ArcStr::from(method.as_str()), positional, runtime)
}

fn call_method_named_runtime(
    receiver: RuntimeVal,
    method: DetachedStr,
    positional: MethodPositionalArgs,
    named: Option<HeapRef>,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<RuntimeVal> {
    if let Some(prop) = runtime_access(&receiver, method.as_str(), runtime.heap_mut())? {
        if runtime_is_callable(&prop, runtime.heap())? {
            let Some((state, ctx, module)) = runtime.parts_mut() else {
                bail!("__lk_call_method_named requires full runtime state for callable receiver");
            };
            return call_runtime_value_runtime_named_map_list_args(
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
    if named.is_none()
        && let Some(result) = dispatch_builtin_method(&receiver, method.as_str(), positional, runtime)?
    {
        return Ok(result);
    }
    bail!("Named arguments are not supported for trait methods")
}

/// Dispatch built-in map instance methods.
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
            let key = runtime_map_key_from_value(&positional[0], heap, "map.set() key")?;
            let value = positional[1];
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
            let key = runtime_map_key_from_value(&positional[0], heap, "map.get() key")?;
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
            let key = runtime_map_key_from_value(&positional[0], heap, "map.has() key")?;
            let found = matches!(heap.get(handle), Some(HeapValue::Map(m)) if m.get(&key).is_some());
            Ok(Some(RuntimeVal::Bool(found)))
        }
        "delete" => {
            if positional.len() != 1 {
                bail!("map.delete() expects 1 argument (key), got {}", positional.len());
            }
            let key = runtime_map_key_from_value(&positional[0], heap, "map.delete() key")?;
            let removed = match heap.get_mut(handle) {
                Some(HeapValue::Map(map)) => map.remove(&key).unwrap_or(RuntimeVal::Nil),
                _ => RuntimeVal::Nil,
            };
            Ok(Some(removed))
        }
        "clear" => {
            if !positional.is_empty() {
                bail!("map.clear() expects no arguments, got {}", positional.len());
            }
            if let Some(HeapValue::Map(map)) = heap.get_mut(handle) {
                map.clear();
            }
            Ok(Some(RuntimeVal::Nil))
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
        "is_empty" => {
            if !positional.is_empty() {
                bail!("map.is_empty() expects no arguments, got {}", positional.len());
            }
            let is_empty = match heap.get(handle) {
                Some(HeapValue::Map(m)) => m.is_empty(),
                _ => true,
            };
            Ok(Some(RuntimeVal::Bool(is_empty)))
        }
        "keys" => {
            if !positional.is_empty() {
                bail!("map.keys() expects no arguments, got {}", positional.len());
            }
            let handle = match receiver {
                RuntimeVal::Obj(h) => *h,
                _ => return Ok(None),
            };
            let keys = match heap.get(handle) {
                Some(HeapValue::Map(m)) => {
                    let mut ks: Vec<RuntimeVal> = Vec::with_capacity(m.len());
                    for (k, _) in m.entries_iter() {
                        ks.push(runtime_map_key_to_value(k, heap));
                    }
                    ks
                }
                _ => return Ok(None),
            };
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(keys))),
            )))
        }
        "values" => {
            if !positional.is_empty() {
                bail!("map.values() expects no arguments, got {}", positional.len());
            }
            let handle = match receiver {
                RuntimeVal::Obj(h) => *h,
                _ => return Ok(None),
            };
            let vals = match heap.get(handle) {
                Some(HeapValue::Map(m)) => {
                    let mut vs: Vec<RuntimeVal> = Vec::with_capacity(m.len());
                    for (_, v) in m.entries_iter() {
                        vs.push(v);
                    }
                    vs
                }
                _ => return Ok(None),
            };
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(vals))),
            )))
        }
        _ => Ok(None),
    }
}

fn dispatch_set_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = receiver else {
        return Ok(None);
    };
    let handle = *handle;
    if !matches!(heap.get(handle), Some(HeapValue::Set(_))) {
        return Ok(None);
    }
    match method {
        "len" => {
            if !positional.is_empty() {
                bail!("set.len() expects no arguments, got {}", positional.len());
            }
            let len = match heap.get(handle) {
                Some(HeapValue::Set(values)) => values.len(),
                _ => 0,
            };
            Ok(Some(RuntimeVal::Int(len as i64)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("set.is_empty() expects no arguments, got {}", positional.len());
            }
            let is_empty = match heap.get(handle) {
                Some(HeapValue::Set(values)) => values.is_empty(),
                _ => true,
            };
            Ok(Some(RuntimeVal::Bool(is_empty)))
        }
        "has" | "contains" => {
            if positional.len() != 1 {
                bail!("set.{method}() expects 1 argument (value), got {}", positional.len());
            }
            let key = runtime_map_key_from_value(&positional[0], heap, "set.has() value")?;
            let found = matches!(heap.get(handle), Some(HeapValue::Set(values)) if values.contains(&key));
            Ok(Some(RuntimeVal::Bool(found)))
        }
        "add" => {
            if positional.len() != 1 {
                bail!("set.add() expects 1 argument (value), got {}", positional.len());
            }
            let key = runtime_map_key_from_value(&positional[0], heap, "set.add() value")?;
            let inserted = match heap.get_mut(handle) {
                Some(HeapValue::Set(values)) => values.insert(key),
                _ => false,
            };
            Ok(Some(RuntimeVal::Bool(inserted)))
        }
        "delete" | "remove" => {
            if positional.len() != 1 {
                bail!("set.{method}() expects 1 argument (value), got {}", positional.len());
            }
            let key = runtime_map_key_from_value(&positional[0], heap, "set.delete() value")?;
            let removed = match heap.get_mut(handle) {
                Some(HeapValue::Set(values)) => values.remove(&key),
                _ => false,
            };
            Ok(Some(RuntimeVal::Bool(removed)))
        }
        "clear" => {
            if !positional.is_empty() {
                bail!("set.clear() expects no arguments, got {}", positional.len());
            }
            if let Some(HeapValue::Set(values)) = heap.get_mut(handle) {
                values.clear();
            }
            Ok(Some(RuntimeVal::Nil))
        }
        "values" => {
            if !positional.is_empty() {
                bail!("set.values() expects no arguments, got {}", positional.len());
            }
            let vals = match heap.get(handle) {
                Some(HeapValue::Set(values)) => values.entries().cloned().collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            let vals = vals
                .into_iter()
                .map(|value| runtime_map_key_to_value(value, heap))
                .collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(vals))),
            )))
        }
        _ => Ok(None),
    }
}

pub(super) fn core_set_builtin(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() > 1 {
        bail!("Set() expects 0 or 1 argument, got {}", args.len());
    }
    let set = match args.get(0) {
        None => RuntimeSet::new(),
        Some(value) => runtime_set_from_value(value, runtime.heap())?,
    };
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Set(set))))
}

fn runtime_set_from_value(value: &RuntimeVal, heap: &HeapStore) -> anyhow::Result<RuntimeSet> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("Set(value) expects List or Set, got {:?}", value.kind());
    };
    match heap.get(*handle) {
        Some(HeapValue::List(list)) => {
            let mut set = RuntimeSet::new();
            for item in list.collect_owned() {
                set.insert(runtime_map_key_from_value(&item, heap, "Set() item")?);
            }
            Ok(set)
        }
        Some(HeapValue::Set(values)) => {
            let mut set = RuntimeSet::new();
            for item in values.entries() {
                set.insert(item.clone());
            }
            Ok(set)
        }
        Some(value) => bail!("Set(value) expects List or Set, got {}", value.type_name()),
        None => bail!("Set(value) heap object out of bounds"),
    }
}

fn runtime_map_key_from_value(value: &RuntimeVal, heap: &HeapStore, context: &str) -> anyhow::Result<RuntimeMapKey> {
    match value {
        RuntimeVal::Nil => Ok(RuntimeMapKey::Nil),
        RuntimeVal::Bool(value) => Ok(RuntimeMapKey::Bool(*value)),
        RuntimeVal::Int(value) => Ok(RuntimeMapKey::Int(*value)),
        RuntimeVal::Float(_) => bail!("{context}: Float cannot be used as a key"),
        RuntimeVal::ShortStr(s) => Ok(RuntimeMapKey::ShortStr(*s)),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(s)) => Ok(RuntimeMapKey::String(Arc::clone(s))),
            Some(_) => Ok(RuntimeMapKey::Obj(*handle)),
            None => bail!("{context}: heap object out of bounds"),
        },
    }
}

fn runtime_map_key_to_value(value: RuntimeMapKey, heap: &mut HeapStore) -> RuntimeVal {
    match value {
        RuntimeMapKey::Nil => RuntimeVal::Nil,
        RuntimeMapKey::Bool(value) => RuntimeVal::Bool(value),
        RuntimeMapKey::Int(value) => RuntimeVal::Int(value),
        RuntimeMapKey::ShortStr(value) => RuntimeVal::ShortStr(value),
        RuntimeMapKey::String(value) => make_string_val(&value, heap),
        RuntimeMapKey::Obj(value) => RuntimeVal::Obj(value),
    }
}

/// Extract a string value from a RuntimeVal as an Arc<str> (cloned, no borrow retained).
fn extract_string_detached(value: &RuntimeVal, heap: &HeapStore, context: &str) -> anyhow::Result<DetachedStr> {
    match value {
        RuntimeVal::ShortStr(s) => Ok(DetachedStr::Short(*s)),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(s)) => Ok(DetachedStr::Heap(Arc::clone(s))),
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
    // Detach the string value from the heap borrow (no byte copy) so the
    // method bodies can use heap mutably.
    let detached = match receiver {
        RuntimeVal::ShortStr(s) => DetachedStr::Short(*s),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(arc)) => DetachedStr::Heap(Arc::clone(arc)),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let s = detached.as_str();
    match method {
        "split" => {
            if positional.len() != 1 {
                bail!(
                    "string.split() expects 1 argument (delimiter), got {}",
                    positional.len()
                );
            }
            let delim = extract_string_detached(&positional[0], heap, "string.split() delimiter")?;
            let mut parts = Vec::new();
            for part in s.split(delim.as_str()) {
                parts.push(Arc::<str>::from(part));
            }
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
            let prefix = extract_string_detached(&positional[0], heap, "string.starts_with() prefix")?;
            Ok(Some(RuntimeVal::Bool(s.starts_with(prefix.as_str()))))
        }
        "ends_with" => {
            if positional.len() != 1 {
                bail!(
                    "string.ends_with() expects 1 argument (suffix), got {}",
                    positional.len()
                );
            }
            let suffix = extract_string_detached(&positional[0], heap, "string.ends_with() suffix")?;
            Ok(Some(RuntimeVal::Bool(s.ends_with(suffix.as_str()))))
        }
        "contains" => {
            if positional.len() != 1 {
                bail!(
                    "string.contains() expects 1 argument (needle), got {}",
                    positional.len()
                );
            }
            let needle = extract_string_detached(&positional[0], heap, "string.contains() needle")?;
            Ok(Some(RuntimeVal::Bool(s.contains(needle.as_str()))))
        }
        "trim" => {
            if !positional.is_empty() {
                bail!("string.trim() expects no arguments, got {}", positional.len());
            }
            Ok(Some(make_string_val(s.trim(), heap)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("string.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(s.is_empty())))
        }
        "lower" => {
            if !positional.is_empty() {
                bail!("string.lower() expects no arguments, got {}", positional.len());
            }
            Ok(Some(make_string_val(&s.to_lowercase(), heap)))
        }
        "upper" => {
            if !positional.is_empty() {
                bail!("string.upper() expects no arguments, got {}", positional.len());
            }
            Ok(Some(make_string_val(&s.to_uppercase(), heap)))
        }
        "find" => {
            if positional.len() != 1 {
                bail!("string.find() expects 1 argument (needle), got {}", positional.len());
            }
            let needle = extract_string_detached(&positional[0], heap, "string.find() needle")?;
            match s.find(needle.as_str()) {
                Some(pos) => Ok(Some(RuntimeVal::Int(pos as i64))),
                None => Ok(Some(RuntimeVal::Int(-1))),
            }
        }
        "substring" => {
            if positional.len() != 2 {
                bail!(
                    "string.substring() expects 2 arguments (start, length), got {}",
                    positional.len()
                );
            }
            let RuntimeVal::Int(start) = &positional[0] else {
                bail!("string.substring() start must be Int");
            };
            let RuntimeVal::Int(length) = &positional[1] else {
                bail!("string.substring() length must be Int");
            };
            let start_val = *start as usize;
            let length_val = *length as usize;

            let end = (start_val.saturating_add(length_val)).min(s.len());

            if end <= start_val {
                Ok(Some(make_string_val("", heap)))
            } else {
                Ok(Some(make_string_val(&s[start_val..end], heap)))
            }
        }
        "reverse" => {
            if !positional.is_empty() {
                bail!("string.reverse() expects no arguments, got {}", positional.len());
            }
            let reversed: String = s.chars().rev().collect();
            Ok(Some(make_string_val(&reversed, heap)))
        }
        "repeat" => {
            if positional.len() != 1 {
                bail!("string.repeat() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("string.repeat() count must be Int");
            };
            if *n <= 0 {
                return Ok(Some(make_string_val("", heap)));
            }
            let repeated: String = s.repeat(*n as usize);
            Ok(Some(make_string_val(&repeated, heap)))
        }
        "chars" => {
            if !positional.is_empty() {
                bail!("string.chars() expects no arguments, got {}", positional.len());
            }
            let chars: Vec<RuntimeVal> = s
                .chars()
                .map(|c| {
                    let mut buf = [0u8; 4];
                    let encoded = c.encode_utf8(&mut buf);
                    let s = String::from(encoded);
                    RuntimeVal::ShortStr(ShortStr::new(&s).unwrap_or_else(|| ShortStr::new("?").unwrap()))
                })
                .collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(chars))),
            )))
        }
        "replace" => {
            if positional.len() != 2 {
                bail!(
                    "string.replace() expects 2 arguments (from, to), got {}",
                    positional.len()
                );
            }
            let from = extract_string_detached(&positional[0], heap, "string.replace() from")?;
            let to = extract_string_detached(&positional[1], heap, "string.replace() to")?;
            Ok(Some(make_string_val(&s.replace(from.as_str(), to.as_str()), heap)))
        }
        _ => Ok(None),
    }
}

fn list_index_arg(value: &RuntimeVal, context: &str) -> anyhow::Result<usize> {
    let RuntimeVal::Int(index) = value else {
        bail!("{context} must be Int");
    };
    if *index < 0 {
        bail!("{context} must be non-negative");
    }
    Ok(*index as usize)
}

fn list_runtime_items(list: TypedList, heap: &mut HeapStore) -> Vec<RuntimeVal> {
    match list {
        TypedList::Mixed(values) => values,
        TypedList::Int(values) => values.into_iter().map(RuntimeVal::Int).collect(),
        TypedList::Float(values) => values.into_iter().map(RuntimeVal::Float).collect(),
        TypedList::Bool(values) => values.into_iter().map(RuntimeVal::Bool).collect(),
        TypedList::String(values) => values
            .into_iter()
            .map(|value| make_string_val(value.as_ref(), heap))
            .collect(),
    }
}

fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal) -> bool {
    match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left == right,
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => left.to_bits() == right.to_bits(),
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => (*left as f64).to_bits() == right.to_bits(),
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => left.to_bits() == (*right as f64).to_bits(),
        (RuntimeVal::ShortStr(left), RuntimeVal::ShortStr(right)) => left.as_str() == right.as_str(),
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) => left == right,
        _ => false,
    }
}

fn compare_runtime_values(left: &RuntimeVal, right: &RuntimeVal) -> core::cmp::Ordering {
    match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => core::cmp::Ordering::Equal,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left.cmp(right),
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left.cmp(right),
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => {
            left.partial_cmp(right).unwrap_or(core::cmp::Ordering::Equal)
        }
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => {
            (*left as f64).partial_cmp(right).unwrap_or(core::cmp::Ordering::Equal)
        }
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => {
            left.partial_cmp(&(*right as f64)).unwrap_or(core::cmp::Ordering::Equal)
        }
        (RuntimeVal::ShortStr(left), RuntimeVal::ShortStr(right)) => left.as_str().cmp(right.as_str()),
        _ => runtime_val_kind_rank(left).cmp(&runtime_val_kind_rank(right)),
    }
}

fn runtime_val_kind_rank(value: &RuntimeVal) -> u8 {
    match value {
        RuntimeVal::Nil => 0,
        RuntimeVal::Bool(_) => 1,
        RuntimeVal::Int(_) => 2,
        RuntimeVal::Float(_) => 3,
        RuntimeVal::ShortStr(_) => 4,
        RuntimeVal::Obj(_) => 5,
    }
}

fn list_join_parts(list: &TypedList, heap: &HeapStore) -> anyhow::Result<Vec<String>> {
    match list {
        TypedList::String(vals) => {
            let mut out = Vec::with_capacity(vals.len());
            for value in vals {
                out.push(value.to_string());
            }
            Ok(out)
        }
        TypedList::Mixed(vals) => {
            let mut out = Vec::with_capacity(vals.len());
            for value in vals {
                let string = match value {
                    RuntimeVal::ShortStr(s) => s.as_str().to_string(),
                    RuntimeVal::Obj(h) => match heap.get(*h) {
                        Some(HeapValue::String(s)) => s.to_string(),
                        Some(other) => bail!("list.join(): element is not a string ({})", other.type_name()),
                        None => bail!("list.join(): heap object out of bounds"),
                    },
                    other => bail!("list.join(): element is not a string ({:?})", other.kind()),
                };
                out.push(string);
            }
            Ok(out)
        }
        _ => bail!("list.join(): list must contain only strings"),
    }
}

fn list_filter(
    items: &[RuntimeVal],
    args: &[RuntimeVal],
    state: &mut crate::vm::RuntimeModuleState,
    module: Option<&crate::vm::Module>,
    ctx: &mut Option<&mut crate::vm::VmContext>,
) -> anyhow::Result<Option<RuntimeVal>> {
    if args.len() != 1 {
        bail!("list.filter() expects 1 argument (predicate), got {}", args.len());
    }
    let pred = args[0];
    let mut filtered = Vec::with_capacity(items.len());
    for item in items {
        let result = crate::vm::call_runtime_value_runtime(pred, &[*item], state, module, ctx.as_deref_mut())?;
        let keep = match &result {
            RuntimeVal::Bool(b) => *b,
            RuntimeVal::Nil => false,
            _ => true,
        };
        if keep {
            filtered.push(*item);
        }
    }
    let result = TypedList::Mixed(filtered);
    Ok(Some(RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(result)))))
}

fn list_map(
    items: &[RuntimeVal],
    args: &[RuntimeVal],
    state: &mut crate::vm::RuntimeModuleState,
    module: Option<&crate::vm::Module>,
    ctx: &mut Option<&mut crate::vm::VmContext>,
) -> anyhow::Result<Option<RuntimeVal>> {
    if args.len() != 1 {
        bail!("list.map() expects 1 argument (transform), got {}", args.len());
    }
    let transform = args[0];
    let mut mapped = Vec::with_capacity(items.len());
    for item in items {
        let result = crate::vm::call_runtime_value_runtime(transform, &[*item], state, module, ctx.as_deref_mut())?;
        // Results accumulated here are invisible to the collector while the
        // next callback runs — pin each one (the caller restores the mark).
        state.host_root_push(result);
        mapped.push(result);
    }
    let result = TypedList::Mixed(mapped);
    Ok(Some(RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(result)))))
}

fn list_reduce(
    items: &[RuntimeVal],
    args: &[RuntimeVal],
    state: &mut crate::vm::RuntimeModuleState,
    module: Option<&crate::vm::Module>,
    ctx: &mut Option<&mut crate::vm::VmContext>,
) -> anyhow::Result<Option<RuntimeVal>> {
    if args.len() != 2 {
        bail!(
            "list.reduce() expects 2 arguments (initial, accumulator), got {}",
            args.len()
        );
    }
    let acc_fn = args[1];
    let mut acc = args[0];
    for item in items {
        // Pin the running accumulator only for the callback that consumes it
        // (per-iteration mark/truncate keeps `host_roots` O(1) instead of
        // growing by one entry per element).
        let iteration_mark = state.host_roots_mark();
        state.host_root_push(acc);
        let result = crate::vm::call_runtime_value_runtime(acc_fn, &[acc, *item], state, module, ctx.as_deref_mut());
        state.host_roots_truncate(iteration_mark);
        acc = result?;
    }
    Ok(Some(acc))
}

fn clone_list(receiver: &RuntimeVal, heap: &mut HeapStore) -> anyhow::Result<TypedList> {
    let handle = match receiver {
        RuntimeVal::Obj(h) => *h,
        _ => bail!("expected list receiver"),
    };
    match heap.get(handle) {
        Some(HeapValue::List(list)) => Ok(list.clone()),
        _ => bail!("expected list receiver"),
    }
}

fn call_trait_method_runtime(
    receiver: RuntimeVal,
    method: ArcStr,
    positional: MethodPositionalArgs,
    runtime: &mut NativeRuntime<'_>,
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
    call_runtime_value_runtime_with_receiver_list_args(
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
            enum RuntimeAccess {
                Ready(Option<RuntimeVal>),
                CopyPayload(crate::rt::RuntimePayload),
                String(String),
            }
            let access = match heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => RuntimeAccess::Ready(runtime_string_access(value.as_ref(), field)),
                HeapValue::Bytes(value) => match field {
                    "len" => RuntimeAccess::Ready(Some(RuntimeVal::Int(value.len() as i64))),
                    _ => RuntimeAccess::Ready(None),
                },
                HeapValue::List(values) => RuntimeAccess::Ready(runtime_list_access(values, field)),
                HeapValue::Map(values) => RuntimeAccess::Ready(values.get_str(field)),
                HeapValue::Slice(slice) => match field {
                    "len" => RuntimeAccess::Ready(Some(RuntimeVal::Int(slice.len as i64))),
                    _ => RuntimeAccess::Ready(None),
                },
                HeapValue::Object(object) => RuntimeAccess::Ready(object.get_field(field)),
                HeapValue::Task(task) if field == "value" => match &task.value {
                    Some(value) => RuntimeAccess::CopyPayload(value.clone()),
                    None => RuntimeAccess::Ready(Some(RuntimeVal::Nil)),
                },
                HeapValue::Channel(channel) => match field {
                    "capacity" => RuntimeAccess::Ready(Some(RuntimeVal::Int(channel.capacity.unwrap_or(0)))),
                    "type" => RuntimeAccess::String(format!("{:?}", channel.inner_type)),
                    _ => RuntimeAccess::Ready(None),
                },
                _ => RuntimeAccess::Ready(None),
            };
            match access {
                RuntimeAccess::Ready(value) => Ok(value),
                RuntimeAccess::CopyPayload(value) => {
                    Ok(Some(crate::vm::copy_runtime_value(&value.value, &value.heap, heap)?))
                }
                RuntimeAccess::String(value) => Ok(Some(runtime_string_value(value, heap))),
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
                if len > MAX_INLINE_METHOD_POSITIONAL_ARGS {
                    bail!(
                        "method positional argument count {} exceeds inline call buffer {}",
                        len,
                        MAX_INLINE_METHOD_POSITIONAL_ARGS
                    );
                }
                let mut values: [RuntimeVal; MAX_INLINE_METHOD_POSITIONAL_ARGS] =
                    core::array::from_fn(|_| RuntimeVal::Nil);
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
                *slot = *value;
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
        HeapValue::Bytes(_) => Type::Named("Bytes".to_string()),
        HeapValue::List(_) => Type::List(Box::new(Type::Any)),
        HeapValue::Map(_) => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
        HeapValue::Set(_) => Type::Set(Box::new(Type::Any)),
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
        HeapValue::Slice(_) => Type::Named("Slice".to_string()),
        HeapValue::Resource(resource) => Type::Named(resource.kind.to_string()),
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
