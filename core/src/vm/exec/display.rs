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
    CallableValue, HeapStore, HeapValue, MAX_VALUE_DEPTH, RuntimeMapKey, RuntimeSet, RuntimeVal, SliceValue, TypedList,
    TypedMap,
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
///
/// Every step further into a container goes through here, so this is where the
/// walk's depth is bounded — see [`MAX_VALUE_DEPTH`].
fn runtime_display_nested(value: &RuntimeVal, heap: &HeapStore, depth: u32) -> Result<String> {
    if depth >= MAX_VALUE_DEPTH {
        return Err(anyhow!(
            "value nested deeper than {MAX_VALUE_DEPTH} levels; it is cyclic or too deeply nested to print"
        ));
    }
    match value {
        RuntimeVal::ShortStr(value) => Ok(quote_string(value.as_str())),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(quote_string(value)),
            other => runtime_display_heap_value(other, heap, depth + 1),
        },
        other => runtime_display_value_at(other, heap, depth + 1),
    }
}

pub fn runtime_display_value(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    runtime_display_value_at(value, heap, 0)
}

fn runtime_display_value_at(value: &RuntimeVal, heap: &HeapStore, depth: u32) -> Result<String> {
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
            runtime_display_heap_value(value, heap, depth)
        }
    }
}
fn runtime_display_heap_value(value: &HeapValue, heap: &HeapStore, depth: u32) -> Result<String> {
    match value {
        HeapValue::String(value) => Ok(value.to_string()),
        HeapValue::Bytes(value) => Ok(runtime_display_bytes(value)),
        HeapValue::List(values) => runtime_display_list(values, heap, depth),
        HeapValue::Slice(slice) => runtime_display_slice(slice, heap, depth),
        HeapValue::Map(values) => runtime_display_map(values, heap, depth),
        HeapValue::Set(values) => runtime_display_set(values),
        HeapValue::Callable(value) => Ok(runtime_display_callable(value)),
        HeapValue::Object(value) => {
            let mut out = value.type_name().to_string();
            // Declaration order — the order the `struct` was written in, which
            // travels with the type (see `DeclaredType::fields`). The fields
            // themselves live in a hash map, so without it the order was the
            // hasher's: `struct Range { start, end }` printed `end` first, and
            // a hasher change would have silently permuted every struct.
            //
            // Sorted by name when the declaration is out of reach — a struct
            // from a module whose type info this executor does not hold, or an
            // object a host built. Arbitrary but stable, which hash order is
            // not.
            let declared = value.ty.fields.as_ref();
            let mut fields: Vec<_> = value.fields.iter().collect();
            if declared.is_empty() {
                fields.sort_by(|(left, _), (right, _)| left.cmp(right));
            } else {
                let position = |name: &alloc::sync::Arc<str>| {
                    declared.iter().position(|field| field == name).unwrap_or(usize::MAX)
                };
                fields.sort_by(|(left, _), (right, _)| {
                    position(left).cmp(&position(right)).then_with(|| left.cmp(right))
                });
            }
            append_display_entries(
                &mut out,
                fields
                    // A field's value is data inside a container, so it quotes
                    // like a list element or a map value. It used to go through
                    // the top-level renderer instead, so `P { name: "a, b" }`
                    // printed as `P{name:a, b}` — which reads as two fields.
                    .into_iter()
                    .map(|(key, value)| Ok((key.to_string(), runtime_display_nested(value, heap, depth)?))),
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
fn runtime_display_list(values: &TypedList, heap: &HeapStore, depth: u32) -> Result<String> {
    let mut out = String::from("[");
    let mut first = true;
    match values {
        TypedList::Mixed(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                out.push_str(&runtime_display_nested(value, heap, depth)?);
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
fn runtime_display_slice(slice: &SliceValue, heap: &HeapStore, depth: u32) -> Result<String> {
    let RuntimeVal::Obj(source) = slice.source else {
        return Ok("[]".to_string());
    };
    let Some(HeapValue::List(values)) = heap.get(source) else {
        return Ok("[]".to_string());
    };
    // `live_len`: the source can have shrunk since the window was taken.
    let window = values.window(slice.start, slice.live_len(heap));
    runtime_display_list(&window, heap, depth)
}
fn runtime_display_map(values: &TypedMap, heap: &HeapStore, depth: u32) -> Result<String> {
    let mut out = String::new();
    match values {
        TypedMap::Mixed(entries) => append_display_entries(
            &mut out,
            entries.iter().map(|(key, value)| {
                Ok((
                    runtime_display_map_key(key),
                    runtime_display_nested(value, heap, depth)?,
                ))
            }),
        )?,
        TypedMap::StringMixed(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), runtime_display_nested(value, heap, depth)?))),
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

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::util::fast_map::fast_hash_map_from_iter;
    use crate::val::{MAX_VALUE_DEPTH, RuntimeObject};
    use crate::vm::{DeclaredType, TypeScope};

    fn object_of(fields: &[(&str, RuntimeVal)]) -> RuntimeObject {
        RuntimeObject::new(
            Arc::new(DeclaredType::new(TypeScope::anonymous(), Arc::<str>::from("P"))),
            fast_hash_map_from_iter(fields.iter().map(|(name, value)| (Arc::<str>::from(*name), *value))),
        )
    }

    fn declared_object_of(declared: &[&str], fields: &[(&str, RuntimeVal)]) -> RuntimeObject {
        RuntimeObject::new(
            Arc::new(DeclaredType::with_fields(
                TypeScope::anonymous(),
                Arc::<str>::from("P"),
                declared.iter().map(|name| Arc::<str>::from(*name)).collect(),
            )),
            fast_hash_map_from_iter(fields.iter().map(|(name, value)| (Arc::<str>::from(*name), *value))),
        )
    }

    /// Fields print in the order the `struct` declares them, whatever order the
    /// value was built in. They lived in a hash map, so the order used to be
    /// the hasher's: `struct Range { start, end }` printed `end` first.
    #[test]
    fn object_fields_follow_the_declaration_order() {
        let mut heap = HeapStore::new();
        let object = RuntimeVal::Obj(heap.alloc(HeapValue::Object(declared_object_of(
            &["start", "end"],
            &[("end", RuntimeVal::Int(9)), ("start", RuntimeVal::Int(1))],
        ))));

        assert_eq!(
            runtime_display_value(&object, &heap).expect("render"),
            "P{start:1,end:9}"
        );
    }

    /// A struct field is data inside a container, so it quotes like a list
    /// element. It went through the top-level renderer instead, and
    /// `P { name: "a, b" }` printed as `P{name:a, b}` — which reads as two
    /// fields. Order is the declaration's, or sorted when the declaration is
    /// out of reach — never the hash order a reader cannot predict.
    #[test]
    fn object_fields_are_quoted_and_ordered() {
        let mut heap = HeapStore::new();
        let text = RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from("a, b"))));
        let object = RuntimeVal::Obj(heap.alloc(HeapValue::Object(object_of(&[
            ("name", text),
            ("count", RuntimeVal::Int(2)),
        ]))));

        assert_eq!(
            runtime_display_value(&object, &heap).expect("render"),
            "P{count:2,name:\"a, b\"}"
        );
    }

    /// Printing a chain deeper than the bound raises instead of overflowing the
    /// Rust stack, which used to abort the process.
    #[test]
    fn nesting_past_the_bound_raises_instead_of_aborting() {
        let mut heap = HeapStore::new();
        let mut node = RuntimeVal::Int(1);
        for _ in 0..(MAX_VALUE_DEPTH + 8) {
            node = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![node]))));
        }

        let error = runtime_display_value(&node, &heap).expect_err("too deep to print");
        assert!(error.to_string().contains("nested deeper than"), "{error}");
    }

    #[test]
    fn nesting_within_the_bound_still_renders() {
        let mut heap = HeapStore::new();
        let mut node = RuntimeVal::Int(1);
        for _ in 0..3 {
            node = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![node]))));
        }

        assert_eq!(runtime_display_value(&node, &heap).expect("render"), "[[[1]]]");
    }
}
