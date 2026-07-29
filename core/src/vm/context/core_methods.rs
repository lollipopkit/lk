#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{anyhow, bail};
use arcstr::ArcStr;

mod bytes_dispatch;
mod list_dispatch;
mod slice_dispatch;
use self::bytes_dispatch::*;
use self::list_dispatch::*;
use self::slice_dispatch::*;

use crate::{
    val::{
        HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, ShortStr, SliceValue, Type, TypedList,
    },
    vm::{
        NativeArgs, NativeRuntime, call_runtime_value_runtime_list_args, call_runtime_value_runtime_named_map_list_args,
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum BuiltinReceiver {
    Map,
    Set,
    Str,
    List,
    Slice,
    Bytes,
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
            Some(HeapValue::Slice(_)) => BuiltinReceiver::Slice,
            Some(HeapValue::Bytes(_)) => BuiltinReceiver::Bytes,
            _ => BuiltinReceiver::Other,
        },
        _ => BuiltinReceiver::Other,
    }
}

/// The declared arity for `method` on `kind`, checked before dispatch.
///
/// Each dispatcher used to state its own arity in a `bail!` guard, which made
/// the declaration and the implementation two sources that drifted apart —
/// `bytes.slice`, `map.get` and `str.slice` each accepted a shape the checker
/// rejected, or the reverse, and only a hand-run comparison found them. The
/// declaration decides here; a guard that disagrees is now unreachable rather
/// than quietly authoritative.
fn check_declared_arity(kind: BuiltinReceiver, method: &str, count: usize) -> anyhow::Result<()> {
    let declared = match kind {
        BuiltinReceiver::Map => crate::typ::BuiltinReceiverKind::Map,
        BuiltinReceiver::Set => crate::typ::BuiltinReceiverKind::Set,
        BuiltinReceiver::Str => crate::typ::BuiltinReceiverKind::Str,
        BuiltinReceiver::Slice => crate::typ::BuiltinReceiverKind::Slice,
        BuiltinReceiver::Bytes => crate::typ::BuiltinReceiverKind::Bytes,
        BuiltinReceiver::List => crate::typ::BuiltinReceiverKind::List,
        BuiltinReceiver::Other => return Ok(()),
    };
    // A name the table does not declare is left to the implementation: a map's
    // entries are its fields, so `m.f(x)` need not be a method at all.
    let Some((required, most)) = crate::typ::builtin_method_arity(declared, method) else {
        return Ok(());
    };
    if count < required || count > most {
        let expected = if required == most {
            alloc::format!("{required}")
        } else {
            alloc::format!("{required} to {most}")
        };
        bail!("{method}() expects {expected} arguments, got {count}");
    }
    Ok(())
}

fn dispatch_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: MethodPositionalArgs,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<Option<RuntimeVal>> {
    let kind = builtin_receiver_kind(receiver, runtime.heap());
    check_declared_arity(kind, method, positional.len(runtime.heap())?)?;
    match kind {
        BuiltinReceiver::Map => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_map_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Set => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_set_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Str => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_string_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Slice => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_slice_builtin_method(receiver, method, positional, heap)
        }),
        BuiltinReceiver::Bytes => positional.with_slice(runtime.heap_mut(), |positional, heap| {
            dispatch_bytes_builtin_method(receiver, method, positional, heap)
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
///
/// Public because the standard library calls it: `iter.map(xs, f)` is defined
/// as `xs.map(f)`, and defining it that way is what makes the two spellings
/// impossible to drift apart. Everything a module form would otherwise
/// reimplement — the truthiness rule, the host-root pinning around callbacks,
/// which list representation comes back — is decided once, here.
pub fn core_call_method_windowed(
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
        BuiltinReceiver::Slice => dispatch_slice_builtin_method(receiver, method, args, runtime.heap_mut()),
        BuiltinReceiver::Bytes => dispatch_bytes_builtin_method(receiver, method, args, runtime.heap_mut()),
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
        // Every sequence, not only a list: a window and a `Bytes` have elements
        // too, and `map`/`filter`/`reduce` mean the same thing over them. What
        // the callback loop below needs is the elements, and nothing about it
        // cares where they came from.
        let sequence_kind = sequence_receiver_kind(&receiver, runtime.heap());
        if let Some(sequence_kind) = sequence_kind {
            let items: Vec<RuntimeVal> = sequence_items(&receiver, sequence_kind, runtime)?;
            let pos_args: Vec<RuntimeVal> = match &positional {
                MethodPositionalArgs::Empty => vec![],
                MethodPositionalArgs::List(handle) => match runtime.heap().get(*handle).cloned() {
                    // Cloned, then materialized through the allocating path: an
                    // argument can be a string past the inline limit, and
                    // `collect_owned` cannot produce one.
                    Some(HeapValue::List(list)) => list_runtime_items(list, runtime.heap_mut()),
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
                if let Some(result) = result? {
                    // `filter` keeps a subset of the elements, so the result is
                    // still bytes; `map` may produce anything, so it is not.
                    // That is the whole rule for which operations preserve a
                    // sequence's type.
                    if matches!(sequence_kind, SequenceKind::Bytes) && method_str == "filter" {
                        return rebuild_bytes(&result, runtime);
                    }
                    return Ok(result);
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
            let keys = TypedList::from_runtime_values(&keys, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(keys)))))
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
            let vals = TypedList::from_runtime_values(&vals, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(vals)))))
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
            let vals: Vec<RuntimeVal> = vals
                .into_iter()
                .map(|value| runtime_map_key_to_value(value, heap))
                .collect();
            let vals = TypedList::from_runtime_values(&vals, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(vals)))))
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
        Some(value) => runtime_set_from_value(value, runtime.heap_mut())?,
    };
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Set(set))))
}

/// Takes `&mut HeapStore` because a list element can be a string past the
/// inline limit, which has to be materialized on the heap before it can become
/// a set key.
fn runtime_set_from_value(value: &RuntimeVal, heap: &mut HeapStore) -> anyhow::Result<RuntimeSet> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("Set(value) expects List or Set, got {:?}", value.kind());
    };
    match heap.get(*handle) {
        Some(HeapValue::List(list)) => {
            let list = list.clone();
            let mut set = RuntimeSet::new();
            for item in list_runtime_items(list, heap) {
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
/// The character at `index`, counting from the end when negative, `nil` when
/// out of range — the rule every sequence's `get` follows.
fn string_char_at(text: &str, index: i64, heap: &mut HeapStore) -> RuntimeVal {
    let total = crate::util::text::char_len(text) as i64;
    let resolved = if index < 0 { total + index } else { index };
    if resolved < 0 || resolved >= total {
        return RuntimeVal::Nil;
    }
    make_string_val(crate::util::text::substring(text, resolved as usize, 1), heap)
}

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
        "byte_at" => {
            if positional.len() != 1 {
                bail!("string.byte_at() expects 1 argument (index), got {}", positional.len());
            }
            let index = match &positional[0] {
                RuntimeVal::Int(value) => *value,
                other => bail!("string.byte_at() index must be an Int, got {:?}", other.kind()),
            };
            let bytes = s.as_bytes();
            // Nil past either end. This answered `-1` while `string.byte_at`
            // answered nil — the same operation with two answers — and `-1` is
            // not what the method declares either (`Int?`). It is a sentinel in
            // a language that says nil everywhere else it means absent:
            // `find`, `get`, `first`, `last`, `pop`, and the module form of
            // this very function.
            if index < 0 || index >= bytes.len() as i64 {
                return Ok(Some(RuntimeVal::Nil));
            }
            Ok(Some(RuntimeVal::Int(bytes[index as usize] as i64)))
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
            // A character index, and `nil` when absent. It used to answer a
            // *byte* offset and `-1`: the offset could not be handed back to
            // `substring` (which counts characters), and `-1` is itself a valid
            // index, so a missed search went wrong quietly instead of loudly.
            match crate::util::text::find_char_index(s, needle.as_str()) {
                Some(index) => Ok(Some(RuntimeVal::Int(index as i64))),
                None => Ok(Some(RuntimeVal::Nil)),
            }
        }
        // The read surface `List` / `Slice` / `Bytes` share. A `String` is a
        // sequence of characters — that is what `len()` counts and what `[i]`
        // indexes — and was the one sequence type without them.
        //
        // `slice(start, end)` in particular is why this matters beyond tidiness:
        // `substring(start, length)` looks identical at the call site and means
        // something else, so `xs.slice(1, 3)` and `s.substring(1, 3)` take
        // different windows from the same numbers.
        "slice" => {
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "string.slice() expects 1 or 2 arguments (start[, end]), got {}",
                    positional.len()
                );
            }
            let RuntimeVal::Int(start) = &positional[0] else {
                bail!("string.slice() start must be Int");
            };
            let start = (*start).max(0);
            let total = crate::util::text::char_len(s) as i64;
            // Omitting `end` means "to the end", as it does on every other
            // sequence.
            let end = match positional.get(1) {
                Some(RuntimeVal::Int(end)) => *end,
                Some(_) => bail!("string.slice() end must be Int"),
                None => total,
            };
            let length = (end - start).max(0);
            let text = crate::util::text::substring(s, start as usize, length as usize);
            Ok(Some(make_string_val(text, heap)))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!(
                    "string.index_of() expects 1 argument (needle), got {}",
                    positional.len()
                );
            }
            let needle = extract_string_detached(&positional[0], heap, "string.index_of() needle")?;
            match crate::util::text::find_char_index(s, needle.as_str()) {
                Some(index) => Ok(Some(RuntimeVal::Int(index as i64))),
                None => Ok(Some(RuntimeVal::Nil)),
            }
        }
        "get" => {
            if positional.len() != 1 {
                bail!("string.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(index) = &positional[0] else {
                bail!("string.get() index must be Int");
            };
            Ok(Some(string_char_at(s, *index, heap)))
        }
        "first" => {
            if !positional.is_empty() {
                bail!("string.first() expects no arguments, got {}", positional.len());
            }
            Ok(Some(string_char_at(s, 0, heap)))
        }
        "last" => {
            if !positional.is_empty() {
                bail!("string.last() expects no arguments, got {}", positional.len());
            }
            Ok(Some(string_char_at(s, -1, heap)))
        }
        "take" => {
            if positional.len() != 1 {
                bail!("string.take() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(count) = &positional[0] else {
                bail!("string.take() count must be Int");
            };
            let text = crate::util::text::substring(s, 0, (*count).max(0) as usize);
            Ok(Some(make_string_val(text, heap)))
        }
        "skip" => {
            if positional.len() != 1 {
                bail!("string.skip() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(count) = &positional[0] else {
                bail!("string.skip() count must be Int");
            };
            let total = crate::util::text::char_len(s);
            let start = (*count).max(0) as usize;
            let text = crate::util::text::substring(s, start, total.saturating_sub(start));
            Ok(Some(make_string_val(text, heap)))
        }
        // TODO(remove): `substring(start, length)` and `find` predate the
        // sequence read surface above. `slice(start, end)` and `index_of` say
        // the same things the way every other sequence says them; keep these
        // two until the corpus and docs have moved off them.
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
            // Character positions. Byte slicing panicked on a multi-byte
            // boundary — `"héllo".substring(2, 3)` took the process down.
            let text = crate::util::text::substring(s, *start as usize, *length as usize);
            Ok(Some(make_string_val(text, heap)))
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
        "bytes" => {
            if !positional.is_empty() {
                bail!("string.bytes() expects no arguments, got {}", positional.len());
            }
            // The way out. Positions in a string are characters, so anything
            // that genuinely needs bytes — a protocol frame, a buffer length —
            // asks for them, and gets a `Bytes` the `bytes` module operates on.
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(s.as_bytes()))),
            )))
        }
        "chars" => {
            if !positional.is_empty() {
                bail!("string.chars() expects no arguments, got {}", positional.len());
            }
            // `TypedList::String`, the same variant `string.chars` builds. As
            // `Mixed` the identical list printed differently — `[a,b]` here
            // against `["a","b"]` there — because rendering asks the variant.
            let chars: Vec<Arc<str>> = s.chars().map(|c| Arc::<str>::from(c.to_string())).collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::String(chars))),
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

/// The list reversed, in the representation it already has.
///
/// `reverse` used to materialize every element — allocating a heap string per
/// element past seven bytes — reverse the `RuntimeVal`s, and box the result as
/// `Mixed`. Reversing a `Vec<Arc<str>>` is a pointer shuffle; the old路 cost
/// about two hundred nanoseconds an element to do the same thing, and left the
/// list boxed so every later read took the slow path.
pub(super) fn typed_list_reversed(list: &TypedList) -> TypedList {
    fn flipped<T: Clone>(values: &[T]) -> Vec<T> {
        let mut out = values.to_vec();
        out.reverse();
        out
    }
    match list {
        TypedList::Mixed(values) => TypedList::Mixed(flipped(values)),
        TypedList::Int(values) => TypedList::Int(flipped(values)),
        TypedList::Float(values) => TypedList::Float(flipped(values)),
        TypedList::Bool(values) => TypedList::Bool(flipped(values)),
        TypedList::String(values) => TypedList::String(flipped(values)),
    }
}

/// The two lists joined, keeping the representation when they share one.
///
/// `None` when they do not — the caller falls back to materializing, which is
/// the only thing that can join an `Int` list to a `String` one.
pub(super) fn typed_lists_concatenated(left: &TypedList, right: &TypedList) -> Option<TypedList> {
    fn joined<T: Clone>(left: &[T], right: &[T]) -> Vec<T> {
        let mut out = Vec::with_capacity(left.len() + right.len());
        out.extend_from_slice(left);
        out.extend_from_slice(right);
        out
    }
    Some(match (left, right) {
        (TypedList::Mixed(left), TypedList::Mixed(right)) => TypedList::Mixed(joined(left, right)),
        (TypedList::Int(left), TypedList::Int(right)) => TypedList::Int(joined(left, right)),
        (TypedList::Float(left), TypedList::Float(right)) => TypedList::Float(joined(left, right)),
        (TypedList::Bool(left), TypedList::Bool(right)) => TypedList::Bool(joined(left, right)),
        (TypedList::String(left), TypedList::String(right)) => TypedList::String(joined(left, right)),
        _ => return None,
    })
}

/// The list sorted ascending, in the representation it already has.
///
/// A typed list sorts its own scalars — an `i64` sort is a comparison, where
/// the materialized path built a `RuntimeVal` per element first and then
/// compared through `compare_runtime_values`. The order is the same one:
/// `compare_runtime_values` on two `Int`s *is* `i64`'s.
pub(super) fn typed_list_sorted(list: &TypedList, heap: &HeapStore) -> TypedList {
    match list {
        TypedList::Int(values) => {
            let mut out = values.to_vec();
            out.sort_unstable();
            TypedList::Int(out)
        }
        TypedList::Float(values) => {
            let mut out = values.to_vec();
            out.sort_by(|left, right| left.partial_cmp(right).unwrap_or(core::cmp::Ordering::Equal));
            TypedList::Float(out)
        }
        TypedList::Bool(values) => {
            let mut out = values.to_vec();
            out.sort_unstable();
            TypedList::Bool(out)
        }
        TypedList::String(values) => {
            let mut out = values.to_vec();
            out.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
            TypedList::String(out)
        }
        // Mixed elements can be anything, including heap values whose order
        // needs the comparison the executor defines.
        TypedList::Mixed(values) => {
            let mut out = values.to_vec();
            out.sort_by(|left, right| compare_runtime_values(left, right, heap));
            TypedList::Mixed(out)
        }
    }
}

/// One element of a list, allocating only for that element.
///
/// The single-element reads — `first`, `last`, `get`, `pop` — used to call
/// `list_runtime_items`, which materializes *every* element and allocates a
/// heap string for each one past seven bytes. Two thousand `pop`s on a
/// twenty-thousand-element string list therefore did forty million
/// allocations to return two thousand values.
///
/// Out of range is nil, as everywhere else.
pub(super) fn typed_list_element(list_handle: HeapRef, index: usize, heap: &mut HeapStore) -> RuntimeVal {
    enum Element {
        Ready(RuntimeVal),
        Text(Arc<str>),
    }
    let element = match heap.get(list_handle) {
        Some(HeapValue::List(list)) => match list {
            TypedList::Mixed(values) => values.get(index).copied().map(Element::Ready),
            TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int).map(Element::Ready),
            TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float).map(Element::Ready),
            TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool).map(Element::Ready),
            // The one case that can allocate — and only for this element.
            TypedList::String(values) => values.get(index).cloned().map(Element::Text),
        },
        _ => None,
    };
    match element {
        Some(Element::Ready(value)) => value,
        Some(Element::Text(text)) => make_string_val(text.as_ref(), heap),
        None => RuntimeVal::Nil,
    }
}

/// Where `needle` first appears in `list`, or `None`.
///
/// Searches the `TypedList` **in place**. `contains`/`index_of`/`unique` used to
/// clone the list and materialize every element into a `RuntimeVal` first —
/// which allocates a heap string for every element past seven bytes — to answer
/// a question that reads each element once and often stops at the first. A
/// twenty-thousand-element string list cost twenty thousand allocations per
/// call, whatever the answer was.
///
/// The typed variants never touch the heap at all: an `Int` list compares
/// integers, a `String` list compares text against text.
pub(super) fn typed_list_position(list: &TypedList, needle: &RuntimeVal, heap: &HeapStore) -> Option<usize> {
    match list {
        TypedList::Int(values) => match needle {
            RuntimeVal::Int(needle) => values.iter().position(|value| value == needle),
            // A float needle can still equal an integer element (`1.0 == 1`),
            // which is the language's rule for `==`.
            RuntimeVal::Float(needle) => values.iter().position(|value| *value as f64 == *needle),
            _ => None,
        },
        TypedList::Float(values) => match needle {
            RuntimeVal::Float(needle) => values.iter().position(|value| value.to_bits() == needle.to_bits()),
            RuntimeVal::Int(needle) => values.iter().position(|value| *value == *needle as f64),
            _ => None,
        },
        TypedList::Bool(values) => match needle {
            RuntimeVal::Bool(needle) => values.iter().position(|value| value == needle),
            _ => None,
        },
        TypedList::String(values) => {
            let needle = runtime_value_text(needle, heap)?;
            values.iter().position(|value| value.as_ref() == needle)
        }
        TypedList::Mixed(values) => values
            .iter()
            .position(|value| runtime_values_equal(value, needle, heap)),
    }
}

/// The list with later duplicates dropped, order preserved, representation kept.
///
/// The typed variants dedup through a hash set — the previous implementation
/// compared each element against every element already kept, which is O(n²):
/// twenty thousand elements with five thousand distinct ones took a hundred
/// million comparisons. It also materialized every element first, and returned
/// a `Mixed` list whatever it was given, so an `Int` list came back boxed and
/// every later read of it took the slow path.
///
/// `Mixed` keeps the quadratic scan, and has to: its elements are arbitrary
/// values whose equality needs the heap, and there is no key to hash them by.
pub(super) fn typed_list_unique(list: &TypedList, heap: &HeapStore) -> TypedList {
    match list {
        TypedList::Int(values) => {
            let mut seen = crate::util::fast_map::fast_hash_set_new();
            TypedList::Int(values.iter().copied().filter(|value| seen.insert(*value)).collect())
        }
        TypedList::Float(values) => {
            let mut seen = crate::util::fast_map::fast_hash_set_new();
            let mut nan_ordinal = 0u64;
            // Keyed by what `==` says, not by bits. `0.0` and `-0.0` are equal,
            // so they share a key; no `NaN` equals any `NaN`, so each gets a
            // fresh one. Bits said the opposite on both counts — the only two
            // places `unique()` still disagreed with `==`.
            //
            // Still one hash lookup per element: canonicalising the key is what
            // keeps this from becoming the O(n²) scan that value equality would
            // otherwise force.
            TypedList::Float(
                values
                    .iter()
                    .copied()
                    .filter(|value| {
                        let key = if value.is_nan() {
                            nan_ordinal += 1;
                            (u64::MAX, nan_ordinal)
                        } else if *value == 0.0 {
                            (0f64.to_bits(), 0)
                        } else {
                            (value.to_bits(), 0)
                        };
                        seen.insert(key)
                    })
                    .collect(),
            )
        }
        TypedList::Bool(values) => {
            let mut seen = crate::util::fast_map::fast_hash_set_new();
            TypedList::Bool(values.iter().copied().filter(|value| seen.insert(*value)).collect())
        }
        TypedList::String(values) => {
            let mut seen = crate::util::fast_map::fast_hash_set_new();
            TypedList::String(
                values
                    .iter()
                    .filter(|value| seen.insert(value.as_ref().to_string()))
                    .cloned()
                    .collect(),
            )
        }
        TypedList::Mixed(values) => {
            let mut unique: Vec<RuntimeVal> = Vec::new();
            for value in values {
                if !unique.iter().any(|seen| runtime_values_equal(seen, value, heap)) {
                    unique.push(*value);
                }
            }
            TypedList::Mixed(unique)
        }
    }
}

/// The text a value holds, without allocating — `None` when it is not a string.
fn runtime_value_text<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Option<&'a str> {
    match value {
        RuntimeVal::ShortStr(value) => Some(value.as_str()),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => Some(value.as_ref()),
            _ => None,
        },
        _ => None,
    }
}

/// Value equality for the search methods (`contains`, `index_of`, `unique`).
///
/// Two heap values are compared by *what they are*, not by which handle they
/// arrived on. Comparing handles is what this used to do, and the boundary it
/// drew was `ShortStr`'s seven-byte inline limit:
///
/// ```text
/// ["ab", "cd"].contains("ab")                 → true
/// ["abcdefghij", …].contains("abcdefghij")    → false
/// ```
///
/// A string too long to live inline is a heap object, and two equal strings
/// built separately are two handles. So a list could not be searched for any
/// string of eight characters or more — nor for a list, a map or a set, ever.
/// Same shape as the `TypedList::String` read bug: the seven-byte boundary is
/// invisible in the source and decides the answer.
fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> bool {
    match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left == right,
        // By value, the same as `==` (`Executor::runtime_values_equal`). Bits
        // were the rule here, which made `0.0 != -0.0` and `NaN == NaN` for
        // every caller of this function — `unique`, `index_of`, `position`,
        // `contains` on a mixed list — and only for them.
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => (*left as f64) == *right,
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => *left == (*right as f64),
        (RuntimeVal::ShortStr(left), RuntimeVal::ShortStr(right)) => left.as_str() == right.as_str(),
        // A short string and a heap string can hold the same text: the same
        // literal reaches one form or the other depending only on its length.
        (RuntimeVal::ShortStr(left), RuntimeVal::Obj(right)) => {
            matches!(heap.get(*right), Some(HeapValue::String(right)) if left.as_str() == right.as_ref())
        }
        (RuntimeVal::Obj(left), RuntimeVal::ShortStr(right)) => {
            matches!(heap.get(*left), Some(HeapValue::String(left)) if left.as_ref() == right.as_str())
        }
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) if left == right => true,
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) => match (heap.get(*left), heap.get(*right)) {
            (Some(left), Some(right)) => heap_values_equal(left, right, heap),
            _ => false,
        },
        _ => false,
    }
}

fn heap_values_equal(left: &HeapValue, right: &HeapValue, heap: &HeapStore) -> bool {
    match (left, right) {
        (HeapValue::String(left), HeapValue::String(right)) => left == right,
        (HeapValue::Bytes(left), HeapValue::Bytes(right)) => left == right,
        (HeapValue::List(left), HeapValue::List(right)) => typed_lists_equal(left, right, heap),
        (HeapValue::Set(left), HeapValue::Set(right)) => {
            left.len() == right.len() && left.entries().all(|key| right.contains(key))
        }
        (HeapValue::Map(left), HeapValue::Map(right)) => {
            let left = left.entries_iter();
            let right = right.entries_iter();
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.iter().any(|(other_key, other_value)| {
                        key == other_key && runtime_values_equal(value, other_value, heap)
                    })
                })
        }
        // A window and the list it windows hold the same elements, and the
        // executor's `==` already says so; this is the same question asked from
        // a method.
        (HeapValue::Slice(left), HeapValue::Slice(right)) => {
            left.len == right.len && (0..left.len).all(|index| slice_element_equal_at(left, index, right, index, heap))
        }
        (HeapValue::Slice(left), HeapValue::List(right)) => slice_equals_list(left, right, heap),
        (HeapValue::List(left), HeapValue::Slice(right)) => slice_equals_list(right, left, heap),
        _ => false,
    }
}

fn typed_lists_equal(left: &TypedList, right: &TypedList, heap: &HeapStore) -> bool {
    left.len() == right.len()
        && (0..left.len()).all(|index| {
            runtime_values_equal(
                &typed_list_value_at(left, index),
                &typed_list_value_at(right, index),
                heap,
            )
        })
}

/// One element as a value, without allocating for a long string.
///
/// A `TypedList::String` element is an `Arc<str>` with no `RuntimeVal` short of
/// a heap allocation, so comparison reads those through `heap` instead — see
/// `typed_list_string_at`.
fn typed_list_value_at(list: &TypedList, index: usize) -> RuntimeVal {
    match list {
        TypedList::Mixed(values) => values.get(index).copied().unwrap_or(RuntimeVal::Nil),
        TypedList::Int(values) => values.get(index).copied().map_or(RuntimeVal::Nil, RuntimeVal::Int),
        TypedList::Float(values) => values.get(index).copied().map_or(RuntimeVal::Nil, RuntimeVal::Float),
        TypedList::Bool(values) => values.get(index).copied().map_or(RuntimeVal::Nil, RuntimeVal::Bool),
        // Long ones cannot become a `RuntimeVal` without allocating; the
        // string-aware comparisons below handle them.
        TypedList::String(values) => values
            .get(index)
            .and_then(|text| ShortStr::new(text).map(RuntimeVal::ShortStr))
            .unwrap_or(RuntimeVal::Nil),
    }
}

/// The text of a `TypedList::String` element, for the comparisons that
/// `typed_list_value_at` cannot express.
fn typed_list_string_at(list: &TypedList, index: usize) -> Option<&str> {
    match list {
        TypedList::String(values) => values.get(index).map(|text| text.as_ref()),
        _ => None,
    }
}

fn slice_source_list<'a>(slice: &SliceValue, heap: &'a HeapStore) -> Option<&'a TypedList> {
    let RuntimeVal::Obj(handle) = slice.source else {
        return None;
    };
    match heap.get(handle) {
        Some(HeapValue::List(list)) => Some(list),
        _ => None,
    }
}

fn slice_element_equal_at(
    left: &SliceValue,
    left_index: usize,
    right: &SliceValue,
    right_index: usize,
    heap: &HeapStore,
) -> bool {
    let (Some(left_list), Some(right_list)) = (slice_source_list(left, heap), slice_source_list(right, heap)) else {
        return false;
    };
    elements_equal(
        left_list,
        left.start + left_index,
        right_list,
        right.start + right_index,
        heap,
    )
}

fn slice_equals_list(slice: &SliceValue, list: &TypedList, heap: &HeapStore) -> bool {
    if slice.len != list.len() {
        return false;
    }
    let Some(source) = slice_source_list(slice, heap) else {
        return false;
    };
    (0..slice.len).all(|index| elements_equal(source, slice.start + index, list, index, heap))
}

/// Two list elements, by position, string-aware.
fn elements_equal(
    left: &TypedList,
    left_index: usize,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> bool {
    match (
        typed_list_string_at(left, left_index),
        typed_list_string_at(right, right_index),
    ) {
        (Some(left), Some(right)) => left == right,
        (Some(text), None) | (None, Some(text)) => {
            let other = if typed_list_string_at(left, left_index).is_some() {
                typed_list_value_at(right, right_index)
            } else {
                typed_list_value_at(left, left_index)
            };
            runtime_value_is_text(&other, text, heap)
        }
        (None, None) => runtime_values_equal(
            &typed_list_value_at(left, left_index),
            &typed_list_value_at(right, right_index),
            heap,
        ),
    }
}

fn runtime_value_is_text(value: &RuntimeVal, text: &str, heap: &HeapStore) -> bool {
    match value {
        RuntimeVal::ShortStr(value) => value.as_str() == text,
        RuntimeVal::Obj(handle) => {
            matches!(heap.get(*handle), Some(HeapValue::String(value)) if value.as_ref() == text)
        }
        _ => false,
    }
}

/// The order `sort` puts values in.
///
/// Takes the heap because a string longer than `ShortStr`'s seven inline bytes
/// lives there, and two of them used to fall through to the by-kind ranking
/// below — both `Obj`, same rank, therefore *equal*. So sorting long strings
/// did nothing at all while sorting short ones worked:
///
/// ```text
/// ["zzz", "aaa", "mmm"].sort()                 → ["aaa", "mmm", "zzz"]
/// ["zzzzzzzzzz", "aaaaaaaaaa", …].sort()       → unchanged
/// ```
///
/// Same seven-byte boundary as the equality and search bugs, in the ordering.
fn compare_runtime_values(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> core::cmp::Ordering {
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
        _ => match (runtime_value_text(left, heap), runtime_value_text(right, heap)) {
            // Two strings, wherever each of them lives.
            (Some(left), Some(right)) => left.cmp(right),
            _ => runtime_val_kind_rank(left).cmp(&runtime_val_kind_rank(right)),
        },
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
    let result = TypedList::from_runtime_values(&filtered, state.heap());
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
    let result = TypedList::from_runtime_values(&mapped, state.heap());
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

/// A filtered `Bytes` back as `Bytes`.
///
/// The callback loop works in `RuntimeVal`s, so it hands back a list; every
/// element of it came out of a `Bytes` and is therefore a byte again.
fn rebuild_bytes(filtered: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = filtered else {
        return Ok(*filtered);
    };
    let Some(HeapValue::List(list)) = runtime.heap().get(*handle) else {
        return Ok(*filtered);
    };
    let mut bytes = Vec::with_capacity(list.len());
    for value in list_runtime_items(list.clone(), runtime.heap_mut()) {
        let RuntimeVal::Int(value) = value else {
            bail!("bytes.filter() kept a non-byte value");
        };
        bytes.push(u8::try_from(value).map_err(|_| anyhow!("bytes.filter() kept {value}, which is not a byte"))?);
    }
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::Bytes(Arc::<[u8]>::from(bytes))),
    ))
}

/// Which sequence a receiver is, for the higher-order methods.
///
/// `None` means "not a sequence", and the caller falls through to trait
/// dispatch — the same answer it gave for everything but a list before windows
/// and `Bytes` had elements the language could reach.
#[derive(Clone, Copy)]
enum SequenceKind {
    List,
    Slice,
    Bytes,
}

fn sequence_receiver_kind(receiver: &RuntimeVal, heap: &HeapStore) -> Option<SequenceKind> {
    let RuntimeVal::Obj(handle) = receiver else {
        return None;
    };
    match heap.get(*handle) {
        Some(HeapValue::List(_)) => Some(SequenceKind::List),
        Some(HeapValue::Slice(_)) => Some(SequenceKind::Slice),
        Some(HeapValue::Bytes(_)) => Some(SequenceKind::Bytes),
        _ => None,
    }
}

/// A sequence's elements, materialized for the callback loop.
///
/// Materializing is what a callback loop needs either way — it hands each
/// element to user code — so a window pays here what it saved everywhere else,
/// and only here.
fn sequence_items(
    receiver: &RuntimeVal,
    kind: SequenceKind,
    runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<Vec<RuntimeVal>> {
    match kind {
        SequenceKind::List => {
            let list = clone_list(receiver, runtime.heap_mut())?;
            Ok(list_runtime_items(list, runtime.heap_mut()))
        }
        SequenceKind::Slice | SequenceKind::Bytes => {
            // Both answer `to_list`, which is exactly this question, and
            // answering it twice is how the two would drift apart.
            let materialized = dispatch_builtin_method_slice(receiver, "to_list", &[], runtime)?
                .ok_or_else(|| anyhow!("sequence receiver has no to_list"))?;
            let list = clone_list(&materialized, runtime.heap_mut())?;
            Ok(list_runtime_items(list, runtime.heap_mut()))
        }
    }
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
    // Taken before `parts_mut` borrows the heap mutably: a struct instance
    // dispatches in the scope of the module that declared it, which is the
    // half of its identity the bare type name does not carry.
    let receiver_scope = super::receiver_type_scope(&receiver, runtime.heap());
    let Some((state, ctx, module)) = runtime.parts_mut() else {
        bail!("{} method '{}' requires full runtime state", receiver_type_name, method);
    };
    let Some(ctx) = ctx else {
        bail!("{} has no method '{}'", receiver_type_name, method);
    };
    // Dispatch on the *declared* type name (`Sq`), not the diagnostic one
    // (`runtime_type_name` reports the heap kind, i.e. "Object", for any
    // struct instance).
    let declared_type = receiver_type.display();
    let Some(impl_ref) = ctx
        .trait_method(&receiver_scope, &declared_type, method.as_str())
        .cloned()
    else {
        bail!("{} has no method '{}'", receiver_type_name, method);
    };
    crate::vm::call_trait_method(
        &impl_ref,
        crate::vm::TraitMethodRef {
            type_name: &declared_type,
            method: method.as_str(),
        },
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
        // Characters, like `s.len()` and `s[i]`. This answered bytes, so
        // `s.len` and `s.len()` disagreed on the same string.
        "len" => Some(RuntimeVal::Int(crate::util::text::char_len(value) as i64)),
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
        HeapValue::Object(object) => Type::Named(object.type_name().to_string()),
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
