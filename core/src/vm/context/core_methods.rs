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
    // Try dispatch for methods that need runtime state BEFORE heap closure
    let method_str = method.as_str();
    if matches!(method_str, "filter" | "map" | "reduce") {
        // Check if receiver is a list
        let is_list = match &receiver {
            RuntimeVal::Obj(h) => matches!(runtime.heap().get(*h), Some(HeapValue::List(_))),
            _ => false,
        };
        if is_list {
            let items: Vec<RuntimeVal> = clone_list(&receiver, runtime.heap_mut())?.into_iter_owned();
            let pos_args: Vec<RuntimeVal> = match &positional {
                MethodPositionalArgs::Empty => vec![],
                MethodPositionalArgs::List(handle) => match runtime.heap().get(*handle) {
                    Some(HeapValue::List(list)) => list.collect_owned(),
                    _ => vec![],
                },
            };
            if let Some((state, mut ctx, module)) = runtime.parts_mut() {
                let result = match method_str {
                    "filter" => list_filter(&items, &pos_args, state, module, &mut ctx)?,
                    "map" => list_map(&items, &pos_args, state, module, &mut ctx)?,
                    "reduce" => list_reduce(&items, &pos_args, state, module, &mut ctx)?,
                    _ => None,
                };
                if let Some(r) = result {
                    return Ok(r);
                }
            }
            return call_trait_method_runtime(receiver, method, positional, runtime);
        }
    }
    if let Some(prop) = runtime_access(&receiver, method_str, runtime.heap_mut())? {
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
        dispatch_map_builtin_method(&receiver, method_str, positional, heap)
    })? {
        return Ok(result);
    }
    if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
        dispatch_string_builtin_method(&receiver, method_str, positional, heap)
    })? {
        return Ok(result);
    }
    if let Some(result) = positional.with_slice(runtime.heap_mut(), |positional, heap| {
        dispatch_list_builtin_method(&receiver, method_str, positional, heap)
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
                        match k {
                            RuntimeMapKey::String(ref s) => ks.push(make_string_val(s, heap)),
                            RuntimeMapKey::ShortStr(ref s) => ks.push(make_string_val(s.as_str(), heap)),
                            _ => {}
                        }
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
            let mut parts = Vec::new();
            for part in s.split(delim.as_ref()) {
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
            let needle = extract_string_arc(&positional[0], heap, "string.find() needle")?;
            match s.find(needle.as_ref()) {
                Some(pos) => Ok(Some(RuntimeVal::Int(pos as i64))),
                None => Ok(Some(RuntimeVal::Int(-1))),
            }
        }
        "substring" => {
            if positional.len() != 2 {
                bail!(
                    "string.substring() expects 2 arguments (start, end), got {}",
                    positional.len()
                );
            }
            let RuntimeVal::Int(start) = &positional[0] else {
                bail!("string.substring() start must be Int");
            };
            let RuntimeVal::Int(end) = &positional[1] else {
                bail!("string.substring() end must be Int");
            };
            let start = *start as usize;
            let end = *end as usize;
            let s_len = s.len();
            let start = start.min(s_len);
            let end = end.min(s_len);
            if end <= start {
                Ok(Some(make_string_val("", heap)))
            } else {
                Ok(Some(make_string_val(&s[start..end], heap)))
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
            let from = extract_string_arc(&positional[0], heap, "string.replace() from")?;
            let to = extract_string_arc(&positional[1], heap, "string.replace() to")?;
            Ok(Some(make_string_val(&s.replace(from.as_ref(), to.as_ref()), heap)))
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
        "first" => {
            if !positional.is_empty() {
                bail!("list.first() expects no arguments, got {}", positional.len());
            }
            let list = clone_list(receiver, heap)?;
            if list.len() == 0 {
                return Ok(Some(RuntimeVal::Nil));
            }
            let first = list.into_iter_owned().into_iter().next().unwrap_or(RuntimeVal::Nil);
            Ok(Some(first))
        }
        "last" => {
            if !positional.is_empty() {
                bail!("list.last() expects no arguments, got {}", positional.len());
            }
            let list = clone_list(receiver, heap)?;
            let items = list.into_iter_owned();
            let last = items.into_iter().last().unwrap_or(RuntimeVal::Nil);
            Ok(Some(last))
        }
        "get" => {
            if positional.len() != 1 {
                bail!("list.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(idx) = &positional[0] else {
                bail!("list.get() index must be Int");
            };
            let list = clone_list(receiver, heap)?;
            if *idx < 0 || *idx as usize >= list.len() {
                return Ok(Some(RuntimeVal::Nil));
            }
            let items = list.into_iter_owned();
            Ok(Some(items.into_iter().nth(*idx as usize).unwrap_or(RuntimeVal::Nil)))
        }
        "skip" => {
            if positional.len() != 1 {
                bail!("list.skip() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("list.skip() count must be Int");
            };
            let mut list = clone_list(receiver, heap)?;
            if *n > 0 {
                list.drain_prefix(*n as usize);
            }
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(list)))))
        }
        "take" => {
            if positional.len() != 1 {
                bail!("list.take() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("list.take() count must be Int");
            };
            let list = clone_list(receiver, heap)?;
            let taken = list.take_prefix(*n as usize);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(taken)))))
        }
        "unique" => {
            if !positional.is_empty() {
                bail!("list.unique() expects no arguments, got {}", positional.len());
            }
            let items = clone_list(receiver, heap)?.into_iter_owned();
            let mut unique: Vec<RuntimeVal> = Vec::new();
            for item in items {
                if !unique.iter().any(|seen| runtime_values_equal(seen, &item)) {
                    unique.push(item);
                }
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(unique))),
            )))
        }
        "contains" => {
            if positional.len() != 1 {
                bail!("list.contains() expects 1 argument (value), got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            Ok(Some(RuntimeVal::Bool(
                items.iter().any(|item| runtime_values_equal(item, &positional[0])),
            )))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!("list.index_of() expects 1 argument (value), got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let index = items
                .iter()
                .position(|item| runtime_values_equal(item, &positional[0]))
                .map(|index| index as i64)
                .unwrap_or(-1);
            Ok(Some(RuntimeVal::Int(index)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("list.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(clone_list(receiver, heap)?.len() == 0)))
        }
        "reverse" => {
            if !positional.is_empty() {
                bail!("list.reverse() expects no arguments, got {}", positional.len());
            }
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.reverse();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "pop" => {
            if !positional.is_empty() {
                bail!("list.pop() expects no arguments, got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            Ok(Some(items.into_iter().last().unwrap_or(RuntimeVal::Nil)))
        }
        "push" => {
            if positional.len() != 1 {
                bail!("list.push() expects 1 argument (value), got {}", positional.len());
            }
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.push(positional[0].clone());
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "slice" => {
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "list.slice() expects 1 or 2 arguments (start[, end]), got {}",
                    positional.len()
                );
            }
            let start = list_index_arg(&positional[0], "list.slice() start")?;
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let end = match positional.get(1) {
                Some(value) => list_index_arg(value, "list.slice() end")?.min(items.len()),
                None => items.len(),
            };
            let sliced = if start >= end {
                Vec::new()
            } else {
                items[start..end].to_vec()
            };
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(sliced))),
            )))
        }
        "insert" => {
            if positional.len() != 2 {
                bail!(
                    "list.insert() expects 2 arguments (index, value), got {}",
                    positional.len()
                );
            }
            let index = list_index_arg(&positional[0], "list.insert() index")?;
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            if index > items.len() {
                bail!("list.insert() index {} out of bounds (len={})", index, items.len());
            }
            items.insert(index, positional[1].clone());
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "remove_at" => {
            if positional.len() != 1 {
                bail!("list.remove_at() expects 1 argument (index), got {}", positional.len());
            }
            let index = list_index_arg(&positional[0], "list.remove_at() index")?;
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            if index >= items.len() {
                bail!("list.remove_at() index {} out of bounds (len={})", index, items.len());
            }
            let old = items.remove(index);
            let updated = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(items))));
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(vec![updated, old]))),
            )))
        }
        "set" => {
            if positional.len() != 2 {
                bail!(
                    "list.set() expects 2 arguments (index, value), got {}",
                    positional.len()
                );
            }
            let index = list_index_arg(&positional[0], "list.set() index")?;
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let Some(slot) = items.get_mut(index) else {
                bail!("list.set() index {} out of bounds (len={})", index, items.len());
            };
            let old = std::mem::replace(slot, positional[1].clone());
            let updated = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(items))));
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(vec![updated, old]))),
            )))
        }
        "sort" => {
            if !positional.is_empty() {
                bail!("list.sort() expects no arguments, got {}", positional.len());
            }
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.sort_by(compare_runtime_values);
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "concat" => {
            if positional.len() != 1 {
                bail!("list.concat() expects 1 argument (list), got {}", positional.len());
            }
            let lhs = clone_list(receiver, heap)?.into_iter_owned();
            let rhs = clone_list(&positional[0], heap)?.into_iter_owned();
            let merged: Vec<RuntimeVal> = lhs.into_iter().chain(rhs.into_iter()).collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(merged))),
            )))
        }
        "zip" => {
            if positional.len() != 1 {
                bail!("list.zip() expects 1 argument (other list), got {}", positional.len());
            }
            let lhs = clone_list(receiver, heap)?.into_iter_owned();
            let rhs = clone_list(&positional[0], heap)?.into_iter_owned();
            let mut pairs = Vec::with_capacity(lhs.len().min(rhs.len()));
            for (a, b) in lhs.into_iter().zip(rhs.into_iter()) {
                pairs.push(RuntimeVal::Obj(
                    heap.alloc(HeapValue::List(TypedList::Mixed(vec![a, b]))),
                ));
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
            )))
        }
        "flatten" => {
            if !positional.is_empty() {
                bail!("list.flatten() expects no arguments, got {}", positional.len());
            }
            let items = clone_list(receiver, heap)?.into_iter_owned();
            let mut flat: Vec<RuntimeVal> = Vec::new();
            for item in items {
                if let RuntimeVal::Obj(h) = &item {
                    if let Some(HeapValue::List(inner)) = heap.get(*h) {
                        flat.extend(inner.clone().into_iter_owned());
                        continue;
                    }
                }
                flat.push(item);
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(flat))),
            )))
        }
        "chunk" => {
            if positional.len() != 1 {
                bail!("list.chunk() expects 1 argument (size), got {}", positional.len());
            }
            let RuntimeVal::Int(size) = &positional[0] else {
                bail!("list.chunk() size must be Int");
            };
            if *size <= 0 {
                bail!("list.chunk() size must be positive");
            }
            let items = clone_list(receiver, heap)?.into_iter_owned();
            let mut chunks: Vec<RuntimeVal> = Vec::new();
            let mut i = 0;
            while i < items.len() {
                let end = (i + *size as usize).min(items.len());
                let chunk: Vec<RuntimeVal> = items[i..end].to_vec();
                chunks.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(chunk)))));
                i = end;
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(chunks))),
            )))
        }
        "enumerate" => {
            if !positional.is_empty() {
                bail!("list.enumerate() expects no arguments, got {}", positional.len());
            }
            let items = clone_list(receiver, heap)?.into_iter_owned();
            let mut pairs = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                pairs.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![
                    RuntimeVal::Int(i as i64),
                    item,
                ])))));
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
            )))
        }
        "chain" => {
            if positional.len() != 1 {
                bail!("list.chain() expects 1 argument (list), got {}", positional.len());
            }
            let lhs = clone_list(receiver, heap)?.into_iter_owned();
            let rhs = clone_list(&positional[0], heap)?.into_iter_owned();
            let merged: Vec<RuntimeVal> = lhs.into_iter().chain(rhs.into_iter()).collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(merged))),
            )))
        }
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

fn compare_runtime_values(left: &RuntimeVal, right: &RuntimeVal) -> std::cmp::Ordering {
    match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => std::cmp::Ordering::Equal,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left.cmp(right),
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left.cmp(right),
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => {
            left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
        }
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => {
            (*left as f64).partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
        }
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => {
            left.partial_cmp(&(*right as f64)).unwrap_or(std::cmp::Ordering::Equal)
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
    state: &mut crate::vm::RuntimeModuleState32,
    module: Option<&crate::vm::Module32>,
    ctx: &mut Option<&mut crate::vm::VmContext>,
) -> anyhow::Result<Option<RuntimeVal>> {
    if args.len() != 1 {
        bail!("list.filter() expects 1 argument (predicate), got {}", args.len());
    }
    let pred = args[0].clone();
    let mut filtered = Vec::with_capacity(items.len());
    for item in items {
        let result =
            crate::vm::call_runtime_value32_runtime(pred.clone(), &[item.clone()], state, module, ctx.as_deref_mut())?;
        let keep = match &result {
            RuntimeVal::Bool(b) => *b,
            RuntimeVal::Nil => false,
            _ => true,
        };
        if keep {
            filtered.push(item.clone());
        }
    }
    let result = TypedList::Mixed(filtered);
    Ok(Some(RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(result)))))
}

fn list_map(
    items: &[RuntimeVal],
    args: &[RuntimeVal],
    state: &mut crate::vm::RuntimeModuleState32,
    module: Option<&crate::vm::Module32>,
    ctx: &mut Option<&mut crate::vm::VmContext>,
) -> anyhow::Result<Option<RuntimeVal>> {
    if args.len() != 1 {
        bail!("list.map() expects 1 argument (transform), got {}", args.len());
    }
    let transform = args[0].clone();
    let mut mapped = Vec::with_capacity(items.len());
    for item in items {
        let result = crate::vm::call_runtime_value32_runtime(
            transform.clone(),
            &[item.clone()],
            state,
            module,
            ctx.as_deref_mut(),
        )?;
        mapped.push(result);
    }
    let result = TypedList::Mixed(mapped);
    Ok(Some(RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(result)))))
}

fn list_reduce(
    items: &[RuntimeVal],
    args: &[RuntimeVal],
    state: &mut crate::vm::RuntimeModuleState32,
    module: Option<&crate::vm::Module32>,
    ctx: &mut Option<&mut crate::vm::VmContext>,
) -> anyhow::Result<Option<RuntimeVal>> {
    if args.len() != 2 {
        bail!(
            "list.reduce() expects 2 arguments (initial, accumulator), got {}",
            args.len()
        );
    }
    let acc_fn = args[1].clone();
    let mut acc = args[0].clone();
    for item in items {
        acc = crate::vm::call_runtime_value32_runtime(
            acc_fn.clone(),
            &[acc, item.clone()],
            state,
            module,
            ctx.as_deref_mut(),
        )?;
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
