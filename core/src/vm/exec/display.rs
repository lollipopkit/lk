//! How a value looks — the one rendering, used by everything that prints.
//!
//! There were two. `println` went through the standard library's renderer and
//! the VM had its own for the REPL, `lk-api`, and (once it stopped erroring)
//! template interpolation. They disagreed about separators, about quoting
//! strings inside a list, and about which types they had heard of:
//!
//! | | `println(v)` | `"${v}"` |
//! |---|---|---|
//! | list | `[1,2]` | `[1, 2]` |
//! | map | `{"a":1}` | `{a: 1}` |
//! | bytes | `<Bytes 2 bytes>` | `<value>` |
//!
//! (Bytes now renders its contents, `Bytes([104, 105])`, like every other
//! container — neither of the two old answers said what was in it.)
//!
//! The standard library's is the one every test and differential comparison
//! pins, so it is the one that moved here — where core can use it and the
//! standard library can call back into it. `show` dispatch stays a layer up in
//! `lk_stdlib_common::language`: it needs to call user code, which is not
//! something a renderer can do.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow};
use core::fmt::Write as _;

use crate::val::{
    CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, SliceValue, TypedList, TypedMap,
};

/// A value inside a container, where a string is quoted.
///
/// Quoting is what tells `["1"]` from `[1]`, and `["a, b"]` from `["a","b"]`.
/// It used to depend on the list's *internal representation*, which no program
/// can see: a `TypedList::String` quoted its elements and a `TypedList::Mixed`
/// did not, so
///
/// ```text
/// ["a", "b"]      → ["a","b"]
/// [1, "a"]        → [1,a]
/// {"k": "v"}      → {"k":v}      the key quoted, the value not
/// ```
///
/// A string on its own is still its text: `println("abc")` prints `abc`. The
/// split is the usual one — a value shown *as data* is quoted, a string printed
/// *as output* is not.
fn runtime_display_nested(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(quote_string(value.as_str())),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(quote_string(value)),
            other => runtime_display_heap_value(other, heap),
        },
        other => runtime_display_value(other, heap),
    }
}

pub fn runtime_display_value(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    match value {
        RuntimeVal::Nil => Ok("nil".to_string()),
        RuntimeVal::Bool(value) => Ok(value.to_string()),
        RuntimeVal::Int(value) => Ok(value.to_string()),
        RuntimeVal::Float(value) => Ok(value.to_string()),
        RuntimeVal::ShortStr(value) => Ok(value.as_str().to_string()),
        RuntimeVal::Obj(handle) => {
            let value = heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            runtime_display_heap_value(value, heap)
        }
    }
}
fn runtime_display_heap_value(value: &HeapValue, heap: &HeapStore) -> Result<String> {
    match value {
        HeapValue::String(value) => Ok(value.to_string()),
        HeapValue::Bytes(value) => Ok(runtime_display_bytes(value)),
        HeapValue::List(values) => runtime_display_list(values, heap),
        HeapValue::Slice(slice) => runtime_display_slice(slice, heap),
        HeapValue::Map(values) => runtime_display_map(values, heap),
        HeapValue::Set(values) => runtime_display_set(values),
        HeapValue::Callable(value) => Ok(runtime_display_callable(value)),
        HeapValue::Object(value) => {
            let mut out = value.type_name().to_string();
            append_display_entries(
                &mut out,
                value
                    .fields
                    .iter()
                    .map(|(key, value)| Ok((key.to_string(), runtime_display_value(value, heap)?))),
            )?;
            Ok(out)
        }
        other => Ok(format!("<{}>", other.type_name())),
    }
}
/// `Bytes([104, 105])` — the contents, in the shape `Set` already uses.
///
/// It used to be `<Bytes 2 bytes>`: a count where every other container shows
/// what is in it, so the one way to see a byte buffer was to convert it
/// (`b.to_list()`), and printing one while debugging told you nothing. The
/// wrapper keeps it distinct from the list `[104, 105]`, which is a different
/// value.
fn runtime_display_bytes(value: &[u8]) -> String {
    let mut out = String::from("Bytes([");
    let mut first = true;
    for byte in value {
        push_display_sep(&mut out, &mut first);
        let _ = write!(out, "{byte}");
    }
    out.push_str("])");
    out
}

fn runtime_display_set(values: &RuntimeSet) -> Result<String> {
    let mut out = String::from("Set(");
    out.push('[');
    let mut first = true;
    let mut entries = values.entries().map(runtime_display_map_key).collect::<Vec<_>>();
    entries.sort();
    for key in entries {
        push_display_sep(&mut out, &mut first);
        out.push_str(&key);
    }
    out.push(']');
    out.push(')');
    Ok(out)
}
fn runtime_display_callable(value: &CallableValue) -> String {
    match value {
        CallableValue::Closure {
            function_index,
            captures,
        } => format!("<fn #{}({} captures)>", function_index, captures.len()),
        CallableValue::RuntimeNative { name, arity, .. } => {
            if *arity == crate::vm::NativeEntry::VARIADIC {
                format!("<native fn {}(...)>", name)
            } else {
                format!("<native fn {}({} args)>", name, arity)
            }
        }
        CallableValue::Runtime(function) => {
            format!(
                "<fn {} ({} captures)>",
                function.display_signature(),
                function.capture_count()
            )
        }
    }
}
fn runtime_display_list(values: &TypedList, heap: &HeapStore) -> Result<String> {
    let mut out = String::from("[");
    let mut first = true;
    match values {
        TypedList::Mixed(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                out.push_str(&runtime_display_nested(value, heap)?);
            }
        }
        TypedList::Int(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                write!(&mut out, "{value}").expect("write to String cannot fail");
            }
        }
        TypedList::Float(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                write!(&mut out, "{value}").expect("write to String cannot fail");
            }
        }
        TypedList::Bool(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                write!(&mut out, "{value}").expect("write to String cannot fail");
            }
        }
        TypedList::String(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                out.push_str(&quote_string(value));
            }
        }
    }
    out.push(']');
    Ok(out)
}
fn runtime_display_slice(slice: &SliceValue, heap: &HeapStore) -> Result<String> {
    let RuntimeVal::Obj(source) = slice.source else {
        return Ok("[]".to_string());
    };
    let Some(HeapValue::List(values)) = heap.get(source) else {
        return Ok("[]".to_string());
    };
    let window = values.window(slice.start, slice.len);
    runtime_display_list(&window, heap)
}
fn runtime_display_map(values: &TypedMap, heap: &HeapStore) -> Result<String> {
    let mut out = String::new();
    match values {
        TypedMap::Mixed(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((runtime_display_map_key(key), runtime_display_nested(value, heap)?))),
        )?,
        TypedMap::StringMixed(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), runtime_display_nested(value, heap)?))),
        )?,
        TypedMap::StringInt(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), value.to_string()))),
        )?,
        TypedMap::StringFloat(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), value.to_string()))),
        )?,
        TypedMap::StringBool(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), value.to_string()))),
        )?,
    }
    Ok(out)
}
fn runtime_display_map_key(key: &RuntimeMapKey) -> String {
    match key {
        RuntimeMapKey::Nil => "nil".to_string(),
        RuntimeMapKey::Bool(value) => value.to_string(),
        RuntimeMapKey::Int(value) => value.to_string(),
        RuntimeMapKey::ShortStr(value) => quote_string(value.as_str()),
        RuntimeMapKey::String(value) => quote_string(value),
        RuntimeMapKey::Obj(value) => format!("<object:{}>", value.index()),
    }
}
fn append_display_entries(out: &mut String, entries: impl IntoIterator<Item = Result<(String, String)>>) -> Result<()> {
    out.push('{');
    let mut first = true;
    for entry in entries {
        let (key, value) = entry?;
        push_display_sep(out, &mut first);
        out.push_str(&key);
        out.push(':');
        out.push_str(&value);
    }
    out.push('}');
    Ok(())
}
fn push_display_sep(out: &mut String, first: &mut bool) {
    if *first {
        *first = false;
    } else {
        out.push(',');
    }
}
fn quote_string(value: &str) -> String {
    format!("{value:?}")
}
