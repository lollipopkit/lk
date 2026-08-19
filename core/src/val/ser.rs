//! Runtime values *out* — the sibling [`super::de`] never had.
//!
//! JSON, YAML and TOML could be read and not written, so the most ordinary
//! script there is — read a config, change a field, write it back — could only
//! do the first two thirds. `base64`, `hex` and `url` next door are all pairs;
//! a parser without its serializer is half an operation.
//!
//! Everything goes through `serde_json::Value` first, so escaping, number
//! formatting and object nesting are decided once by a library that has already
//! argued about them, and the YAML and TOML writers get the same input.
//!
//! # What refuses, and why
//!
//! A value with no JSON counterpart is an error rather than a guess:
//!
//! - **A map key that is not a string.** JSON object keys are strings, and both
//!   Python and JavaScript quietly stringify an integer key — so `1` and `"1"`
//!   land on the same entry and the round trip stops being one. Refusing says
//!   so at the point the program can still choose.
//! - **NaN and the infinities.** JSON has no spelling for them; `null` is what
//!   JavaScript substitutes, which turns a broken computation into a missing
//!   field.
//! - **A set, a byte buffer, a function, a channel, a task.** An array would
//!   read back as a list, base64 is the caller's decision, and the rest are not
//!   data.
//!
//! A `struct` *is* written, as an object of its fields — it reads back as a
//! map, which is what a JSON object is.
//!
//! Object keys come out **sorted**, because `serde_json::Map` is a `BTreeMap`.
//! That is left alone rather than worked around: a config written twice from
//! the same data is byte-identical, which is what makes the output diffable,
//! and JSON says nothing about key order anyway. It does mean `stringify` and
//! `println` order a struct's fields differently — `println` shows the
//! declaration order, which is what a reader wrote.
//!
//! **Reading is the other way round, on purpose.** [`super::de`] hands back a
//! document's keys in the order the document has them, because an LK map's
//! order is a contract and a parsed document has an order to keep. The two are
//! not in tension: writing imposes an order so the bytes are stable, reading
//! reports the order it was given. What *was* wrong is that reading used to sort
//! too — not by decision, but because `serde_json::Value` is a `BTreeMap` and
//! nobody had looked.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::val::{HeapStore, HeapValue, MAX_VALUE_DEPTH, RuntimeMapKey, RuntimeVal, TypedList, TypedMap};
use alloc::string::{String, ToString};
use anyhow::{Result, bail};

/// `value` as compact JSON text.
pub fn to_json_string(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    Ok(to_serde_value(value, heap, 0)?.to_string())
}

/// `value` as YAML text.
#[cfg(feature = "std")]
pub fn to_yaml_string(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    let value = to_serde_value(value, heap, 0)?;
    serde_yaml::to_string(&value).map_err(|error| anyhow::anyhow!("cannot write YAML: {error}"))
}

/// `value` as TOML text.
///
/// TOML has no top-level scalar or array — a document *is* a table — so
/// anything but a map is refused here rather than producing a file no TOML
/// parser will read back.
#[cfg(feature = "std")]
pub fn to_toml_string(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    let value = to_serde_value(value, heap, 0)?;
    if !value.is_object() {
        bail!("a TOML document is a table, so the top level must be a map");
    }
    toml::to_string(&value).map_err(|error| anyhow::anyhow!("cannot write TOML: {error}"))
}

fn to_serde_value(value: &RuntimeVal, heap: &HeapStore, depth: u32) -> Result<serde_json::Value> {
    if depth >= MAX_VALUE_DEPTH {
        bail!("value nested deeper than {MAX_VALUE_DEPTH} levels; it is cyclic or too deeply nested to write");
    }
    Ok(match value {
        RuntimeVal::Nil => serde_json::Value::Null,
        RuntimeVal::Bool(value) => serde_json::Value::Bool(*value),
        RuntimeVal::Int(value) => serde_json::Value::from(*value),
        RuntimeVal::Float(value) => match serde_json::Number::from_f64(*value) {
            Some(number) => serde_json::Value::Number(number),
            // `null` is what JavaScript substitutes here, which turns a broken
            // computation into a missing field.
            None => bail!("{value} has no JSON form (NaN and the infinities do not)"),
        },
        RuntimeVal::ShortStr(value) => serde_json::Value::String(value.as_str().to_string()),
        RuntimeVal::Obj(handle) => {
            let Some(object) = heap.get(*handle) else {
                bail!("heap object {} out of bounds", handle.index());
            };
            heap_value_to_serde(object, heap, depth)?
        }
    })
}

fn heap_value_to_serde(value: &HeapValue, heap: &HeapStore, depth: u32) -> Result<serde_json::Value> {
    Ok(match value {
        HeapValue::String(text) => serde_json::Value::String(text.to_string()),
        HeapValue::List(list) => list_to_serde(list, heap, depth)?,
        HeapValue::Slice(slice) => {
            let RuntimeVal::Obj(source) = slice.source else {
                return Ok(serde_json::Value::Array(Vec::new()));
            };
            let Some(HeapValue::List(list)) = heap.get(source) else {
                return Ok(serde_json::Value::Array(Vec::new()));
            };
            list_to_serde(&list.window(slice.start, slice.live_len(heap)), heap, depth)?
        }
        HeapValue::Map(map) => map_to_serde(map, heap, depth)?,
        // A struct is an object of its fields. No ordering effort here: the
        // map below is a `BTreeMap`, so whatever order they go in they come out
        // sorted — see this module's note.
        HeapValue::Object(object) => {
            let mut out = serde_json::Map::with_capacity(object.fields.len());
            for (name, value) in &object.fields {
                out.insert(name.to_string(), to_serde_value(value, heap, depth + 1)?);
            }
            serde_json::Value::Object(out)
        }
        other => bail!("{} has no JSON form", other.type_name()),
    })
}

fn list_to_serde(list: &TypedList, heap: &HeapStore, depth: u32) -> Result<serde_json::Value> {
    let mut out = Vec::with_capacity(list.len());
    match list {
        TypedList::Mixed(values) => {
            for value in values {
                out.push(to_serde_value(value, heap, depth + 1)?);
            }
        }
        TypedList::Int(values) => out.extend(values.iter().map(|value| serde_json::Value::from(*value))),
        TypedList::Float(values) => {
            for value in values {
                out.push(to_serde_value(&RuntimeVal::Float(*value), heap, depth + 1)?);
            }
        }
        TypedList::Bool(values) => out.extend(values.iter().map(|value| serde_json::Value::Bool(*value))),
        TypedList::String(values) => {
            out.extend(values.iter().map(|value| serde_json::Value::String(value.to_string())));
        }
    }
    Ok(serde_json::Value::Array(out))
}

fn map_to_serde(map: &TypedMap, heap: &HeapStore, depth: u32) -> Result<serde_json::Value> {
    let entries = map.entries_iter();
    let mut out = serde_json::Map::with_capacity(entries.len());
    for (key, value) in entries.iter() {
        out.insert(object_key(key)?, to_serde_value(value, heap, depth + 1)?);
    }
    Ok(serde_json::Value::Object(out))
}

/// A JSON object key, or a refusal.
///
/// Python and JavaScript both stringify a non-string key, so `1` and `"1"` land
/// on the same entry and the round trip stops being one. Saying so is the point
/// where the program can still choose.
fn object_key(key: &RuntimeMapKey) -> Result<String> {
    match key {
        RuntimeMapKey::ShortStr(text) => Ok(text.as_str().to_string()),
        RuntimeMapKey::String(text) => Ok(text.to_string()),
        RuntimeMapKey::Int(value) => bail!(
            "a JSON object key is a String, and `{value}` is an Int — write it as \"{value}\" if that is what you mean"
        ),
        RuntimeMapKey::Bool(value) => bail!("a JSON object key is a String, and `{value}` is a Bool"),
        RuntimeMapKey::Nil => bail!("a JSON object key is a String, and `nil` is not one"),
    }
}
