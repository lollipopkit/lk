#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};
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
            "{helper} expects method name as string, got {}",
            other.type_name_in(heap)
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
    // The receiver's own method wins over a same-named key or field.
    //
    // This used to run *after* the key lookup, which made the documented rule
    // ("方法优先", docs/semantics.md) true for exactly one method: `len`, and
    // only because the compiler emits a dedicated opcode for it.
    // `{"keys": 5, "z": 1}.keys()` answered `5`, `{"is_empty": 5}.is_empty()`
    // answered `5` — the key had shadowed the method, and which of the two you
    // got depended on whether the method happened to have its own opcode.
    //
    // Method-first is the rule because the other order makes a builtin method
    // vanish from *some* maps with no diagnostic, while a shadowed key still
    // has an unambiguous spelling (`m["len"]`).
    if let Some(result) = dispatch_builtin_method_slice(&receiver, method_name, args, runtime)? {
        return Ok(result);
    }
    // No builtin of that name: the key (or struct field) may hold the callable,
    // or be a plain value read with `()` — `m.f(1)` where `f` is a stored
    // function is the shape this exists for.
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
            // The receiver, so writes chain the way `push`/`insert` do. A
            // mutating method answers the container unless it has something
            // better to say — `delete` hands back what it removed, `add`
            // reports whether the value was new.
            Ok(Some(*receiver))
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
            // `m.has(k)` and `k in m` are one question, so they answer the
            // same way: a value that cannot be a key is not a key the map
            // holds. `in` says `false` and this said "map.has() key: Float
            // cannot be a map key or set member" — two answers, decided by
            // which spelling the program used.
            //
            // `delete` below keeps refusing, and the difference is the same one
            // `map_contains` draws: asking is a predicate, removing names a key.
            let found = match runtime_map_key_from_value(&positional[0], heap, "map.has() key") {
                Ok(key) => matches!(heap.get(handle), Some(HeapValue::Map(m)) if m.get(&key).is_some()),
                Err(_) => false,
            };
            Ok(Some(RuntimeVal::Bool(found)))
        }
        "delete" => {
            if positional.len() != 1 {
                bail!("map.delete() expects 1 argument (key), got {}", positional.len());
            }
            // Removing a key the map cannot hold removes nothing — and cannot
            // corrupt the map's key type, which is why this joins the
            // predicates rather than the key *builders* (`set`, indexing).
            // `m - k` already answered this way.
            let Ok(key) = runtime_map_key_from_value(&positional[0], heap, "map.delete() key") else {
                return Ok(Some(RuntimeVal::Nil));
            };
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
            Ok(Some(*receiver))
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
        "contains" => {
            if positional.len() != 1 {
                bail!("set.{method}() expects 1 argument (value), got {}", positional.len());
            }
            // `s.contains(v)` and `v in s` are one question, and answer alike:
            // a value that cannot be a member is not one. `add` below still
            // refuses, because it builds the key rather than asking after it.
            let found = match runtime_map_key_from_value(&positional[0], heap, "set.contains() value") {
                Ok(key) => matches!(heap.get(handle), Some(HeapValue::Set(values)) if values.contains(&key)),
                Err(_) => false,
            };
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
        // `remove` was a second name for this and is gone; nothing used it.
        "delete" => {
            if positional.len() != 1 {
                bail!("set.{method}() expects 1 argument (value), got {}", positional.len());
            }
            // Removing a value the set cannot hold removes nothing, for
            // `map.delete`'s reason. `add` still refuses.
            let Ok(key) = runtime_map_key_from_value(&positional[0], heap, "set.delete() value") else {
                return Ok(Some(RuntimeVal::Bool(false)));
            };
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
            Ok(Some(*receiver))
        }
        // The set operations. A `Set` that can only add, delete, test a member
        // and hand back a list is a deduplicating bag; these are what make it a
        // set, and none of them existed.
        //
        // **The insertion sequence is the contract.** A set's iteration order
        // is its hash order (see `DYN_SET` and the mirror discipline), so two
        // sets with the same members can still iterate differently if they were
        // filled in different sequences. Each operation below therefore fills
        // the answer in one stated order — the receiver's own order first, then
        // the argument's — and the native mirror replays exactly that. Building
        // the same answer "some other way" is how the two ends come to print a
        // set differently.
        "union" | "intersection" | "difference" | "symmetric_difference" => {
            if positional.len() != 1 {
                bail!("set.{method}() expects 1 argument (other), got {}", positional.len());
            }
            let mine = set_entries(handle, heap);
            let theirs = set_entries_of_value(&positional[0], heap, method)?;
            let other: crate::util::fast_map::FastHashSet<RuntimeMapKey> = theirs.iter().cloned().collect();
            let mut out = crate::util::fast_map::fast_hash_set_new();
            match method {
                "union" => {
                    out.extend(mine.iter().cloned());
                    out.extend(theirs.iter().cloned());
                }
                "intersection" => out.extend(mine.iter().filter(|key| other.contains(*key)).cloned()),
                "difference" => out.extend(mine.iter().filter(|key| !other.contains(*key)).cloned()),
                _ => {
                    let owned: crate::util::fast_map::FastHashSet<RuntimeMapKey> = mine.iter().cloned().collect();
                    out.extend(mine.iter().filter(|key| !other.contains(*key)).cloned());
                    out.extend(theirs.iter().filter(|key| !owned.contains(*key)).cloned());
                }
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Set(RuntimeSet::from_entries(out))),
            )))
        }
        // The three predicates. `is_disjoint` is not `!intersection().is_empty()`
        // spelled out — it stops at the first shared member and allocates
        // nothing.
        "is_subset" | "is_superset" | "is_disjoint" => {
            if positional.len() != 1 {
                bail!("set.{method}() expects 1 argument (other), got {}", positional.len());
            }
            let mine = set_entries(handle, heap);
            let theirs = set_entries_of_value(&positional[0], heap, method)?;
            let answer = match method {
                "is_subset" => {
                    let other: crate::util::fast_map::FastHashSet<RuntimeMapKey> = theirs.iter().cloned().collect();
                    mine.iter().all(|key| other.contains(key))
                }
                "is_superset" => {
                    let owned: crate::util::fast_map::FastHashSet<RuntimeMapKey> = mine.iter().cloned().collect();
                    theirs.iter().all(|key| owned.contains(key))
                }
                _ => {
                    let other: crate::util::fast_map::FastHashSet<RuntimeMapKey> = theirs.iter().cloned().collect();
                    !mine.iter().any(|key| other.contains(key))
                }
            };
            Ok(Some(RuntimeVal::Bool(answer)))
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
        bail!("Set(value) expects List or Set, got {}", value.type_name_in(heap));
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

/// The key a value is used under, with the caller's name on the front — see
/// [`RuntimeMapKey::from_value`], which is the one conversion.
fn runtime_map_key_from_value(value: &RuntimeVal, heap: &HeapStore, context: &str) -> anyhow::Result<RuntimeMapKey> {
    RuntimeMapKey::from_value(value, heap).map_err(|error| anyhow!("{context}: {error}"))
}

fn runtime_map_key_to_value(value: RuntimeMapKey, heap: &mut HeapStore) -> RuntimeVal {
    match value {
        RuntimeMapKey::Nil => RuntimeVal::Nil,
        RuntimeMapKey::Bool(value) => RuntimeVal::Bool(value),
        RuntimeMapKey::Int(value) => RuntimeVal::Int(value),
        RuntimeMapKey::ShortStr(value) => RuntimeVal::ShortStr(value),
        RuntimeMapKey::String(value) => make_string_val(&value, heap),
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
        other => bail!("{context}: expected string, got {}", other.type_name_in(heap)),
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
                other => bail!(
                    "string.byte_at() index must be an Int, got {}",
                    other.kind().scalar_type_name()
                ),
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
            // Total, like `in` on the same string and like every other
            // container's membership: a needle that is not a string is not a
            // substring. `1 in "abc"` has always said `false` here, and this
            // said "string.contains() needle: expected string, got Int" — one
            // question, two answers, chosen by which spelling was written.
            let Ok(needle) = extract_string_detached(&positional[0], heap, "string.contains() needle") else {
                return Ok(Some(RuntimeVal::Bool(false)));
            };
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
            let total = crate::util::text::char_len(s);
            let start = slice_position(&positional[0], total, "string.slice() start")?;
            // Omitting `end` means "to the end", as it does on every other
            // sequence.
            let end = match positional.get(1) {
                Some(RuntimeVal::Nil) | None => total,
                Some(value) => slice_position(value, total, "string.slice() end")?,
            };
            let text = crate::util::text::substring(s, start, end.saturating_sub(start));
            Ok(Some(make_string_val(text, heap)))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!(
                    "string.index_of() expects 1 argument (needle), got {}",
                    positional.len()
                );
            }
            // Absent, for `contains`'s reason.
            let Ok(needle) = extract_string_detached(&positional[0], heap, "string.index_of() needle") else {
                return Ok(Some(RuntimeVal::Nil));
            };
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
            // Refused, not clamped — the same rule `list.take()` follows. A
            // count is not a position: a negative *position* means "from the
            // end" here, and that decision is what made `.max(0)` look
            // reasonable, but `take(-1)` is a mistake in any reading and the
            // List carrier has said so all along.
            if *count < 0 {
                bail!("string.take() count must be non-negative, got {count}");
            }
            let text = crate::util::text::substring(s, 0, *count as usize);
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
            if *count < 0 {
                bail!("string.skip() count must be non-negative, got {count}");
            }
            let start = *count as usize;
            let text = crate::util::text::substring(s, start, total.saturating_sub(start));
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
            // Zero repeats is the empty string; a *negative* count is a
            // mistake, and every other count-taking method says so.
            if *n < 0 {
                bail!("string.repeat() count must be non-negative, got {n}");
            }
            if *n == 0 {
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
            // The optional third argument is what the module spelling has had
            // all along: `all: false` replaces the first occurrence only. The
            // method could not say it, so the two spellings were not the same
            // operation — and the module could not simply forward here.
            if !(2..=3).contains(&positional.len()) {
                bail!(
                    "string.replace() expects 2 or 3 arguments (from, to[, all]), got {}",
                    positional.len()
                );
            }
            let from = extract_string_detached(&positional[0], heap, "string.replace() from")?;
            let to = extract_string_detached(&positional[1], heap, "string.replace() to")?;
            let all = match positional.get(2) {
                None | Some(RuntimeVal::Nil) => true,
                Some(RuntimeVal::Bool(all)) => *all,
                Some(_) => bail!("string.replace() `all` must be Bool"),
            };
            let replaced = if all {
                s.replace(from.as_str(), to.as_str())
            } else {
                s.replacen(from.as_str(), to.as_str(), 1)
            };
            Ok(Some(make_string_val(&replaced, heap)))
        }
        // The nine operations the `string` module used to own outright. They are
        // receiver-first questions about a string, so they belong here with the
        // rest — and moving them is what lets the module forward instead of
        // holding a second body (see `forward` there).
        "capitalize" => {
            if !positional.is_empty() {
                bail!("string.capitalize() expects no arguments, got {}", positional.len());
            }
            let mut chars = s.chars();
            let mut out = String::with_capacity(s.len());
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
            }
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
            Ok(Some(make_string_val(&out, heap)))
        }
        "title" => {
            if !positional.is_empty() {
                bail!("string.title() expects no arguments, got {}", positional.len());
            }
            let mut out = String::with_capacity(s.len());
            let mut start_of_word = true;
            for ch in s.chars() {
                if ch.is_whitespace() {
                    start_of_word = true;
                    out.push(ch);
                } else if start_of_word {
                    out.extend(ch.to_uppercase());
                    start_of_word = false;
                } else {
                    out.extend(ch.to_lowercase());
                }
            }
            Ok(Some(make_string_val(&out, heap)))
        }
        "count" => {
            if positional.len() != 1 {
                bail!("string.count() expects 1 argument (needle), got {}", positional.len());
            }
            // Zero, for `contains`'s reason.
            let Ok(needle) = extract_string_detached(&positional[0], heap, "string.count() needle") else {
                return Ok(Some(RuntimeVal::Int(0)));
            };
            // An empty needle matches between every pair of characters and at
            // both ends — `str::matches` says so, and counting characters + 1
            // said something else for any multi-byte string.
            Ok(Some(RuntimeVal::Int(s.matches(needle.as_str()).count() as i64)))
        }
        "strip" => {
            if positional.len() != 1 {
                bail!("string.strip() expects 1 argument (chars), got {}", positional.len());
            }
            let chars = extract_string_detached(&positional[0], heap, "string.strip() chars")?;
            let stripped = s.trim_matches(|ch| chars.as_str().contains(ch));
            Ok(Some(make_string_val(stripped, heap)))
        }
        "strip_prefix" | "strip_suffix" => {
            if positional.len() != 1 {
                bail!("string.{method}() expects 1 argument, got {}", positional.len());
            }
            let affix = extract_string_detached(&positional[0], heap, "string.strip_prefix/suffix() affix")?;
            let stripped = if method == "strip_prefix" {
                s.strip_prefix(affix.as_str())
            } else {
                s.strip_suffix(affix.as_str())
            };
            // `String?`: nil when it was not there, which is what makes the
            // answer distinguishable from "it was there and left nothing".
            Ok(Some(match stripped {
                Some(text) => make_string_val(text, heap),
                None => RuntimeVal::Nil,
            }))
        }
        "pad_left" | "pad_right" => {
            if !(1..=2).contains(&positional.len()) {
                bail!(
                    "string.{method}() expects 1 or 2 arguments (width[, fill]), got {}",
                    positional.len()
                );
            }
            let RuntimeVal::Int(width) = &positional[0] else {
                bail!("string.{method}() width must be Int");
            };
            if *width < 0 {
                bail!("string.{method}() width must be non-negative, got {width}");
            }
            let fill = match positional.get(1) {
                None | Some(RuntimeVal::Nil) => " ".to_string(),
                Some(value) => {
                    let fill = extract_string_detached(value, heap, "string.pad_left() fill")?;
                    if fill.as_str().is_empty() {
                        bail!("string.{method}() fill must not be empty");
                    }
                    fill.as_str().to_string()
                }
            };
            // Width counts *characters*, because that is the unit everything
            // else in the language counts — `s.len()`, `s[i]`, `s.slice(a, b)`.
            // And the fill repeats by `cycle().take(n)` rather than by slicing a
            // repeated string, so there is no byte boundary to get wrong: the
            // byte-sliced version panicked the process on `pad_left("a", 5,
            // "中")`, and a Rust panic is not something a script can catch.
            let len = crate::util::text::char_len(s);
            let width = *width as usize;
            if len >= width {
                return Ok(Some(make_string_val(s, heap)));
            }
            let padding: String = fill.chars().cycle().take(width - len).collect();
            let padded = if method == "pad_left" {
                alloc::format!("{padding}{s}")
            } else {
                alloc::format!("{s}{padding}")
            };
            Ok(Some(make_string_val(&padded, heap)))
        }
        "format" => {
            // `"{} and {}".format(a, b)` — the receiver is the template, which
            // is exactly the shape `string.format(template, …)` already had.
            let mut out = String::with_capacity(s.len());
            let mut chars = s.chars().peekable();
            let mut next_arg = 0usize;
            while let Some(ch) = chars.next() {
                if ch == '{' && chars.peek() == Some(&'}') {
                    chars.next();
                    match positional.get(next_arg) {
                        Some(value) => {
                            out.push_str(&crate::vm::display_runtime_value(value, heap));
                            next_arg += 1;
                        }
                        // A placeholder with no argument left stays literal,
                        // which is what `println`'s format does with the same
                        // shape.
                        None => out.push_str("{}"),
                    }
                } else {
                    out.push(ch);
                }
            }
            // …and an argument with no placeholder left is appended, space
            // separated — also `println`'s rule. Dropping it silently is the
            // one answer that loses data.
            if next_arg < positional.len() {
                if !out.is_empty() {
                    out.push(' ');
                }
                for (index, value) in positional[next_arg..].iter().enumerate() {
                    if index > 0 {
                        out.push(' ');
                    }
                    out.push_str(&crate::vm::display_runtime_value(value, heap));
                }
            }
            Ok(Some(make_string_val(&out, heap)))
        }
        _ => Ok(None),
    }
}

/// A `slice` boundary resolved against `len`.
///
/// One convention for positions, the language's own: **negative counts from the
/// end** — `-1` is the last element, exactly as in `xs[-1]` and `xs.get(-1)` —
/// and the result is clamped into `0..=len`, like every other position here.
///
/// The four `slice` implementations had four answers for a negative one. List
/// and Bytes raised; Slice and String clamped it to `0` and returned a window
/// nobody asked for; and the *native* string slice already counted from the end,
/// so `"abcde".slice(1, -1)` was `""` interpreted and `"bcd"` compiled — the
/// same program, two answers. Counting from the end is what the rest of the
/// language already means by a negative position, so that is what this says.
pub(super) fn slice_position(value: &RuntimeVal, len: usize, context: &str) -> anyhow::Result<usize> {
    crate::val::position::read_position(value, len, context)
}

/// A *write* position against a container of `len` elements.
///
/// Negative counts from the end, as everywhere else — `xs.set(-1, v)` writes
/// the last element, which is what `xs[-1]` reads. Still out of range after
/// that is an error and stays one: reading past the end is nil, writing past it
/// is not something a program can mean. The caller does the upper-bound check,
/// because `insert` accepts `len` and the others do not.
pub(super) fn write_index_arg(value: &RuntimeVal, len: usize, context: &str) -> anyhow::Result<usize> {
    crate::val::position::write_position(value, len, context)
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
/// `Mixed`. Reversing a `Vec<Arc<str>>` is a pointer shuffle; the old path cost
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
/// The sum of a list of numbers.
///
/// Integers wrap, floats add as floats, and a mix promotes to float — the same
/// three rules `+` follows, because `xs.sum()` is `+` applied down the list and
/// a second set of rules for it would be a second answer.
///
/// An empty list is `0`, the identity `reduce(0, …)` would have started from.
/// Anything that is not a number is a refusal naming what was found: summing
/// strings has no meaning here (`+` concatenates them, but a list of strings
/// asked for its *sum* is a mistake, not a join).
pub(super) fn typed_list_sum(list: &TypedList, heap: &HeapStore) -> Result<RuntimeVal> {
    match list {
        TypedList::Int(values) => Ok(RuntimeVal::Int(
            values.iter().fold(0i64, |total, value| total.wrapping_add(*value)),
        )),
        TypedList::Float(values) => Ok(RuntimeVal::Float(values.iter().sum())),
        TypedList::Bool(_) => bail!("list.sum() adds numbers, and this is a list of Bool"),
        TypedList::String(_) => bail!("list.sum() adds numbers, and this is a list of String"),
        TypedList::Mixed(values) => {
            let mut total_int: i64 = 0;
            let mut total_float = 0.0f64;
            let mut saw_float = false;
            for value in values {
                match value {
                    RuntimeVal::Int(value) => {
                        total_int = total_int.wrapping_add(*value);
                        total_float += *value as f64;
                    }
                    RuntimeVal::Float(value) => {
                        saw_float = true;
                        total_float += *value;
                    }
                    other => bail!(
                        "list.sum() adds numbers, and this list holds a {}",
                        other.type_name_in(heap)
                    ),
                }
            }
            Ok(if saw_float {
                RuntimeVal::Float(total_float)
            } else {
                RuntimeVal::Int(total_int)
            })
        }
    }
}

/// Where the smallest (or largest) element is, by the order
/// [`typed_list_sorted`] sorts with — the same comparison, not a second one
/// that happens to agree today.
///
/// An *index*, so the caller materializes the element through
/// [`typed_list_element`] like every other single-element read does: a string
/// element has to be allocated into the heap, and that is the one place that
/// knows how.
///
/// `None` for an empty list, which the callers turn into nil — what
/// `first`/`last` answer there, and "the largest of nothing" is the same
/// question.
pub(super) fn typed_list_extreme_index(list: &TypedList, heap: &HeapStore, want_max: bool) -> Option<usize> {
    let better = |left: usize, right: usize| -> bool {
        let ordering = match list {
            TypedList::Int(values) => values[left].cmp(&values[right]),
            TypedList::Float(values) => crate::val::compare_floats(values[left], values[right]),
            TypedList::Bool(values) => values[left].cmp(&values[right]),
            TypedList::String(values) => values[left].as_ref().cmp(values[right].as_ref()),
            TypedList::Mixed(values) => compare_runtime_values(&values[left], &values[right], heap),
        };
        // Ties keep the earlier element: `min`/`max` name a *value*, and the
        // first one that has it is the one a reader would point at.
        match ordering {
            core::cmp::Ordering::Less => !want_max,
            core::cmp::Ordering::Equal => true,
            core::cmp::Ordering::Greater => want_max,
        }
    };
    (0..list.len()).reduce(|best, index| if better(best, index) { best } else { index })
}

pub(super) fn typed_list_sorted(list: &TypedList, heap: &HeapStore) -> TypedList {
    match list {
        TypedList::Int(values) => {
            let mut out = values.to_vec();
            out.sort_unstable();
            TypedList::Int(out)
        }
        TypedList::Float(values) => {
            let mut out = values.to_vec();
            out.sort_by(|left, right| crate::val::compare_floats(*left, *right));
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
/// A set's members in *its own* iteration order.
///
/// Detached from the heap because the answer is built into a fresh set while
/// the source is still borrowed; the keys are cheap to clone and there are two
/// of them to read.
fn set_entries(handle: crate::val::HeapRef, heap: &HeapStore) -> Vec<RuntimeMapKey> {
    match heap.get(handle) {
        Some(HeapValue::Set(values)) => values.entries().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The same, for the argument of a set operation — which must be a `Set`.
fn set_entries_of_value(value: &RuntimeVal, heap: &HeapStore, method: &str) -> Result<Vec<RuntimeMapKey>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("set.{method}() argument must be a Set");
    };
    match heap.get(*handle) {
        Some(HeapValue::Set(values)) => Ok(values.entries().cloned().collect()),
        _ => bail!("set.{method}() argument must be a Set"),
    }
}

pub(super) fn typed_list_position(list: &TypedList, needle: &RuntimeVal, heap: &HeapStore) -> Result<Option<usize>> {
    let mut found = None;
    typed_list_scan(list, needle, heap, |index| {
        found = Some(index);
        false
    })?;
    Ok(found)
}

/// How many elements equal `needle`, under the same rules.
pub(super) fn typed_list_count(list: &TypedList, needle: &RuntimeVal, heap: &HeapStore) -> Result<usize> {
    let mut found = 0;
    typed_list_scan(list, needle, heap, |_| {
        found += 1;
        true
    })?;
    Ok(found)
}

/// Every index whose element equals `needle`, in order, until `on_match`
/// answers `false`.
///
/// One function rather than one per question, because the *rules* are the
/// payload: an `Int` element equals a `Float` needle when the numbers match
/// (`1.0 == 1`, the language's rule for `==`), a `Float` list compares by value
/// so `0.0` finds `-0.0`, and a `Mixed` list defers to `runtime_values_equal`.
/// `index_of` and `count` are the same scan with different accumulators, and
/// writing them apart is how two spellings of one operation come to disagree.
fn typed_list_scan(
    list: &TypedList,
    needle: &RuntimeVal,
    heap: &HeapStore,
    mut on_match: impl FnMut(usize) -> bool,
) -> Result<()> {
    fn scan<T>(values: &[T], mut eq: impl FnMut(&T) -> bool, on_match: &mut impl FnMut(usize) -> bool) {
        for (index, value) in values.iter().enumerate() {
            if eq(value) && !on_match(index) {
                return;
            }
        }
    }
    match list {
        TypedList::Int(values) => match needle {
            RuntimeVal::Int(needle) => scan(values, |value| value == needle, &mut on_match),
            RuntimeVal::Float(needle) => scan(values, |value| *value as f64 == *needle, &mut on_match),
            _ => {}
        },
        TypedList::Float(values) => match needle {
            RuntimeVal::Float(needle) => scan(values, |value| value == needle, &mut on_match),
            RuntimeVal::Int(needle) => scan(values, |value| *value == *needle as f64, &mut on_match),
            _ => {}
        },
        TypedList::Bool(values) => {
            if let RuntimeVal::Bool(needle) = needle {
                scan(values, |value| value == needle, &mut on_match);
            }
        }
        TypedList::String(values) => {
            if let Some(needle) = runtime_value_text(needle, heap) {
                scan(values, |value| value.as_ref() == needle, &mut on_match);
            }
        }
        TypedList::Mixed(values) => {
            for (index, value) in values.iter().enumerate() {
                if crate::val::runtime_values_equal(value, needle, heap)? && !on_match(index) {
                    break;
                }
            }
        }
    }
    Ok(())
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
pub(super) fn typed_list_unique(list: &TypedList, heap: &HeapStore) -> Result<TypedList> {
    Ok(match list {
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
                let mut seen_before = false;
                for seen in &unique {
                    if crate::val::runtime_values_equal(seen, value, heap)? {
                        seen_before = true;
                        break;
                    }
                }
                if !seen_before {
                    unique.push(*value);
                }
            }
            TypedList::Mixed(unique)
        }
    })
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
///
/// Containers were the other half of that hole and are handled below: two
/// *lists* compare element by element, and every other pair of heap values by
/// their kind. Before that they were both `Obj`, one rank, therefore equal —
/// so sorting a list of lists also did nothing at all:
///
/// ```text
/// [[1,"b"], [1,"a"], [0,"c"]].sort()   → unchanged
/// ```
fn compare_runtime_values(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> core::cmp::Ordering {
    compare_runtime_values_at(left, right, heap, 0)
}

fn compare_runtime_values_at(
    left: &RuntimeVal,
    right: &RuntimeVal,
    heap: &HeapStore,
    depth: u32,
) -> core::cmp::Ordering {
    match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => core::cmp::Ordering::Equal,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left.cmp(right),
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left.cmp(right),
        // Every float-involving arm goes through the total order: a mixed list
        // sorts with this comparator too, so a NaN anywhere in it had the same
        // panic as a float list.
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => crate::val::compare_floats(*left, *right),
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => crate::val::compare_floats(*left as f64, *right),
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => crate::val::compare_floats(*left, *right as f64),
        _ => match (runtime_value_text(left, heap), runtime_value_text(right, heap)) {
            // Two strings, wherever each of them lives.
            (Some(left), Some(right)) => left.cmp(right),
            _ => compare_heap_values(left, right, heap, depth),
        },
    }
}

/// Two values of which at least one is a heap object.
///
/// Lists (and windows over them, which are lists by every other measure)
/// compare lexicographically — element by element, and a prefix sorts before
/// what extends it, which is what `==` already treats them as. Everything else
/// compares by *kind*: a map has no order against another map, but grouping
/// them deterministically is still better than calling them equal.
fn compare_heap_values(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore, depth: u32) -> core::cmp::Ordering {
    let (RuntimeVal::Obj(left_handle), RuntimeVal::Obj(right_handle)) = (left, right) else {
        return runtime_val_kind_rank(left).cmp(&runtime_val_kind_rank(right));
    };
    let (Some(left_value), Some(right_value)) = (heap.get(*left_handle), heap.get(*right_handle)) else {
        return runtime_val_kind_rank(left).cmp(&runtime_val_kind_rank(right));
    };
    // Past the bound the values are cyclic or pathological. `sort_by` wants an
    // `Ordering`, not a `Result` — and raising half way through a sort would
    // leave the list rearranged anyway — so this is the one place the depth
    // limit answers rather than reports. See `crate::val::MAX_VALUE_DEPTH`.
    if depth < crate::val::MAX_VALUE_DEPTH
        && let (Some(left_items), Some(right_items)) = (list_view(left_value, heap), list_view(right_value, heap))
    {
        return compare_list_views(&left_items, &right_items, heap, depth + 1);
    }
    heap_kind_rank(left_value).cmp(&heap_kind_rank(right_value))
}

/// A list, or the window a slice reads through — both are sequences here.
fn list_view(value: &HeapValue, heap: &HeapStore) -> Option<TypedList> {
    match value {
        HeapValue::List(list) => Some(list.clone()),
        HeapValue::Slice(slice) => {
            let RuntimeVal::Obj(source) = slice.source else {
                return Some(TypedList::Mixed(Vec::new()));
            };
            let Some(HeapValue::List(list)) = heap.get(source) else {
                return Some(TypedList::Mixed(Vec::new()));
            };
            Some(list.window(slice.start, slice.live_len(heap)))
        }
        _ => None,
    }
}

fn compare_list_views(left: &TypedList, right: &TypedList, heap: &HeapStore, depth: u32) -> core::cmp::Ordering {
    for index in 0..left.len().min(right.len()) {
        let ordering = match (list_item_text(left, index), list_item_text(right, index)) {
            (Some(left), Some(right)) => left.cmp(right),
            _ => compare_runtime_values_at(
                &list_item_value(left, index),
                &list_item_value(right, index),
                heap,
                depth,
            ),
        };
        if ordering != core::cmp::Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

/// A `TypedList::String` element is an `Arc<str>`, which no `RuntimeVal`
/// carries past seven bytes — the same reason equality reads it as text.
fn list_item_text(list: &TypedList, index: usize) -> Option<&str> {
    match list {
        TypedList::String(values) => values.get(index).map(|text| text.as_ref()),
        _ => None,
    }
}

fn list_item_value(list: &TypedList, index: usize) -> RuntimeVal {
    match list {
        TypedList::Mixed(values) => values.get(index).copied().unwrap_or(RuntimeVal::Nil),
        TypedList::Int(values) => values.get(index).copied().map_or(RuntimeVal::Nil, RuntimeVal::Int),
        TypedList::Float(values) => values.get(index).copied().map_or(RuntimeVal::Nil, RuntimeVal::Float),
        TypedList::Bool(values) => values.get(index).copied().map_or(RuntimeVal::Nil, RuntimeVal::Bool),
        TypedList::String(values) => values
            .get(index)
            .and_then(|text| ShortStr::new(text).map(RuntimeVal::ShortStr))
            .unwrap_or(RuntimeVal::Nil),
    }
}

/// Heap kinds in a fixed order, so a list and a map sort into groups instead of
/// comparing equal. Arbitrary, but stated once and stable.
fn heap_kind_rank(value: &HeapValue) -> u8 {
    match value {
        HeapValue::String(_) => 0,
        HeapValue::Bytes(_) => 1,
        HeapValue::List(_) | HeapValue::Slice(_) => 2,
        HeapValue::Map(_) => 3,
        HeapValue::Set(_) => 4,
        HeapValue::Object(_) => 5,
        HeapValue::Callable(_) => 6,
        HeapValue::ErrorVal(_) => 7,
        _ => 8,
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
                    other => bail!("list.join(): element is not a string ({})", other.type_name_in(heap)),
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
    // Owned because `parts_mut` takes the heap mutably below, and this borrows
    // it: `type_name_in` names a struct instance `P`, which is not a `'static`
    // string. That is the whole point — these messages used to say "Object has
    // no method 'nonexistent'" while `declared_type` sat two lines down with the
    // real name in it, already computed for dispatch.
    let receiver_type_name = receiver.type_name_in(runtime.heap()).to_string();
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
    let declared_type = receiver_type.display();
    let Some(impl_ref) = ctx
        .trait_method(&receiver_scope, &declared_type, method.as_str())
        .cloned()
    else {
        // A map is the one receiver where a miss has two possible causes, so it
        // says both: `m.thing()` looks for a method *and* for a key holding a
        // function, and "Map has no method `thing`" left the second half out —
        // for the receiver whose members are usually keys.
        if matches!(receiver_type_name.as_str(), "Map") {
            bail!(
                "a Map has no method `{method}`, and this map has no key `{method}` holding a function \
                 either"
            );
        }
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
            // A channel's `capacity`/`type` and a task's `value` used to be
            // readable here as *properties*, and nothing could reach them: the
            // checker refuses a field access on either type, and the dynamic
            // route refuses them as not indexable (`index target object is not
            // indexable: "Channel"`). Instrumented, both arms were dead in every
            // example, every test and every probe. The spelling the language has
            // is the module function — `chans.capacity(ch)`, which
            // `concurrency_demo.lk` uses and docs/semantics.md documents.
            //
            // They were also the only reason this read a `RuntimeAccess` enum
            // rather than an `Option<RuntimeVal>`: one arm needed a payload
            // copied out of another heap and one needed a string allocated,
            // both while the heap was still borrowed. Neither remains.
            Ok(
                match heap
                    .get(*handle)
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                {
                    HeapValue::String(value) => runtime_string_access(value.as_ref(), field),
                    HeapValue::Bytes(value) => match field {
                        "len" => Some(RuntimeVal::Int(value.len() as i64)),
                        _ => None,
                    },
                    HeapValue::List(values) => runtime_list_access(values, field),
                    HeapValue::Map(values) => values.get_str(field),
                    HeapValue::Slice(slice) => match field {
                        "len" => Some(RuntimeVal::Int(slice.len as i64)),
                        _ => None,
                    },
                    HeapValue::Object(object) => object.get_field(field),
                    _ => None,
                },
            )
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
        other => bail!(
            "{helper} expects positional arguments as list, got {}",
            other.kind().scalar_type_name()
        ),
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
        other => bail!(
            "{helper} expects named arguments as map, got {}",
            other.type_name_in(heap)
        ),
    };

    let heap_val = heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::Map(_) = heap_val else {
        bail!("{helper} expects named arguments as map, got {}", heap_val.type_name());
    };
    Ok(Some(handle))
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
        // The element is dropped, as it is for a list and a map above: an impl
        // target names the *constructor* (`impl Channel`), so a receiver
        // carrying its own inner type would key on something no impl registers
        // under. `Task` already did; these two did not.
        HeapValue::Task(_) => Type::Task(Box::new(Type::Any)),
        HeapValue::Channel(_) => Type::Channel(Box::new(Type::Any)),
        HeapValue::Stream(_) => Type::Generic {
            name: "Stream".to_string(),
            params: vec![Type::Any],
        },
        HeapValue::StreamCursor(_) => Type::Named("StreamCursor".to_string()),
        // `Slice<Any>`, not a bare `Slice`: an impl target written `Slice` is
        // parsed as `Slice<Any>` the way `List` is parsed as `List<Any>`, and
        // this is the key the registration is looked up by.
        HeapValue::Slice(_) => crate::typ::slice_of(Type::Any),
        HeapValue::Resource(resource) => Type::Named(resource.kind.to_string()),
        HeapValue::Object(object) => Type::Named(object.type_name().to_string()),
        HeapValue::UpvalCell(_) => Type::Any,
        HeapValue::ErrorVal(_) => Type::Named("Error".to_string()),
    }
}
