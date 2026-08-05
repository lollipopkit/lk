//! Native `json`/`yaml`/`toml` decoding (deep-coverage plan I): the exact
//! crates and conversion rules of the VM's `core/src/val/de.rs`, so values —
//! numbers, nesting, and **map iteration order** — match byte-for-byte.
//!
//! Order argument: the VM inserts each decoded object's entries, in the
//! serde iteration order (serde_json `Value::Object` is a BTreeMap → sorted;
//! serde_yaml `Mapping` and `toml::Table` preserve/sort per their own
//! defaults — the same crates at the same lockfile versions produce the same
//! sequence), into a fresh `FastHashMap` and rebuilds the typed map from
//! *its* iteration (`typed_map_from_entries`). [`str_dyn_map_mirrored`]
//! replays both stages.
//!
//! Arrays decode to dyn lists (the VM shapes uniform scalars into typed
//! lists — indexing/len/eq agree; display quoting of a uniform *string*
//! array would differ, which the differential gates would catch if the
//! corpus exercised it).

// `alloc`, not the std prelude: this module is part of the computation-only
// subset that builds without an OS.
#[allow(unused_imports)]
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use alloc::ffi::CString;
use core::ffi::{CStr, c_char};

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_LIST, DYN_MAP, DYN_SLICE, LkDyn, is_list_tag};
use crate::lkstr::arena_c_string;
use crate::state::arena_handle;
use crate::vm_mirror::str_dyn_map_mirrored;

fn input(s: *const c_char) -> &'static str {
    if s.is_null() {
        return "";
    }
    // SAFETY: parse inputs are NUL-terminated LK strings.
    unsafe { CStr::from_ptr(s) }.to_str().unwrap_or("")
}

fn dyn_str_of(text: &str) -> LkDyn {
    let ptr = arena_c_string(CString::new(text).unwrap_or_default());
    LkDyn {
        tag: crate::lkdyn::DYN_STR,
        payload: ptr as i64,
    }
}

fn dyn_bool(value: bool) -> LkDyn {
    LkDyn {
        tag: DYN_BOOL,
        payload: i64::from(value),
    }
}

fn dyn_int(value: i64) -> LkDyn {
    LkDyn {
        tag: DYN_I64,
        payload: value,
    }
}

fn dyn_float(value: f64) -> LkDyn {
    LkDyn {
        tag: DYN_F64,
        payload: value.to_bits() as i64,
    }
}

fn dyn_list_of(items: Vec<LkDyn>) -> LkDyn {
    LkDyn {
        tag: DYN_LIST,
        payload: arena_handle(items) as i64,
    }
}

fn dyn_map_of(pairs: Vec<(String, LkDyn)>) -> LkDyn {
    LkDyn {
        tag: DYN_MAP,
        payload: str_dyn_map_mirrored(pairs) as i64,
    }
}

/// `number_to_runtime`: integer when it fits, float otherwise, nil never
/// (serde numbers always carry one of the two).
fn dyn_number(int_value: Option<i64>, float_value: Option<f64>) -> LkDyn {
    match (int_value, float_value) {
        (Some(v), _) => dyn_int(v),
        (None, Some(v)) => dyn_float(v),
        (None, None) => LkDyn::NIL,
    }
}

fn json_to_dyn(value: serde_json::Value) -> LkDyn {
    match value {
        serde_json::Value::Null => LkDyn::NIL,
        serde_json::Value::Bool(value) => dyn_bool(value),
        serde_json::Value::Number(value) => dyn_number(value.as_i64(), value.as_f64()),
        serde_json::Value::String(value) => dyn_str_of(&value),
        serde_json::Value::Array(values) => dyn_list_of(values.into_iter().map(json_to_dyn).collect()),
        serde_json::Value::Object(values) => dyn_map_of(values.into_iter().map(|(k, v)| (k, json_to_dyn(v))).collect()),
    }
}

/// `json.parse(text)` — a parse error is the VM's catchable raise.
///
/// # Safety
/// `text` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_json_parse(text: *const c_char) -> LkDyn {
    match serde_json::from_str::<serde_json::Value>(input(text)) {
        Ok(value) => json_to_dyn(value),
        Err(_) => crate::panic::raise_str("Invalid JSON"),
    }
}

#[cfg(feature = "std")]
fn yaml_to_dyn(value: serde_yaml::Value) -> LkDyn {
    match value {
        serde_yaml::Value::Null => LkDyn::NIL,
        serde_yaml::Value::Bool(value) => dyn_bool(value),
        serde_yaml::Value::Number(value) => dyn_number(value.as_i64(), value.as_f64()),
        serde_yaml::Value::String(value) => dyn_str_of(&value),
        serde_yaml::Value::Sequence(values) => dyn_list_of(values.into_iter().map(yaml_to_dyn).collect()),
        serde_yaml::Value::Mapping(values) => {
            let mut pairs = Vec::with_capacity(values.len());
            for (key, value) in values {
                // The VM only accepts nil/bool/int/string YAML keys; the
                // native map carrier is string-keyed, so only string keys
                // reach it (others raise — same loud failure family).
                let serde_yaml::Value::String(key) = key else {
                    crate::panic::raise_str("unsupported YAML map key");
                };
                pairs.push((key, yaml_to_dyn(value)));
            }
            dyn_map_of(pairs)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_dyn(tagged.value),
    }
}

/// `yaml.parse(text)`.
///
/// # Safety
/// `text` must be a valid C string, or null.
#[cfg(feature = "std")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_yaml_parse(text: *const c_char) -> LkDyn {
    match serde_yaml::from_str::<serde_yaml::Value>(input(text)) {
        Ok(value) => yaml_to_dyn(value),
        Err(_) => crate::panic::raise_str("Invalid YAML"),
    }
}

#[cfg(feature = "std")]
fn toml_to_dyn(value: toml::Value) -> LkDyn {
    match value {
        toml::Value::String(value) => dyn_str_of(&value),
        toml::Value::Integer(value) => dyn_int(value),
        toml::Value::Float(value) => dyn_float(value),
        toml::Value::Boolean(value) => dyn_bool(value),
        toml::Value::Datetime(value) => dyn_str_of(&value.to_string()),
        toml::Value::Array(values) => dyn_list_of(values.into_iter().map(toml_to_dyn).collect()),
        toml::Value::Table(values) => dyn_map_of(values.into_iter().map(|(k, v)| (k, toml_to_dyn(v))).collect()),
    }
}

/// `toml.parse(text)`.
///
/// # Safety
/// `text` must be a valid C string, or null.
#[cfg(feature = "std")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_toml_parse(text: *const c_char) -> LkDyn {
    match toml::from_str::<toml::Value>(input(text)) {
        Ok(value) => toml_to_dyn(value),
        Err(_) => crate::panic::raise_str("Invalid TOML"),
    }
}

/// The write direction: an LK value as `serde_json::Value`, by exactly the
/// rules of the VM's `core/src/val/ser.rs`.
///
/// Object keys come out **sorted**, and that is not a choice made here: both
/// sides build a `serde_json::Map`, which is a `BTreeMap`. So `stringify` is
/// the one place where a map's iteration order does *not* show — the ordering
/// argument that governs `parse` does not apply in reverse.
mod write {
    use super::*;
    use crate::lkdyn::{DYN_BYTES, DYN_NIL, DYN_RAW, DYN_SET, DYN_STR, is_map_tag, map_entries};
    use crate::vm_mirror::{RtKey, key_str};

    /// The VM's `MAX_VALUE_DEPTH`, and its refusal names the number.
    const MAX_VALUE_DEPTH: u32 = 512;

    pub(super) fn to_serde(value: LkDyn, depth: u32) -> Result<serde_json::Value, String> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(format!(
                "value nested deeper than {MAX_VALUE_DEPTH} levels; it is cyclic or too deeply nested to write"
            ));
        }
        Ok(match value.tag {
            DYN_NIL => serde_json::Value::Null,
            DYN_BOOL => serde_json::Value::Bool(value.payload != 0),
            DYN_I64 => serde_json::Value::from(value.payload),
            DYN_F64 => {
                let number = f64::from_bits(value.payload as u64);
                match serde_json::Number::from_f64(number) {
                    Some(number) => serde_json::Value::Number(number),
                    None => return Err(format!("{number} has no JSON form (NaN and the infinities do not)")),
                }
            }
            DYN_STR => serde_json::Value::String(input(value.payload as *const c_char).to_string()),
            // Every list representation, not only the boxed one: a typed
            // carrier boxes in place now, so `json.stringify([[1]])` sees a
            // `DYN_TLIST_*` tag where it used to see a rebuilt `DYN_LIST`.
            tag if is_list_tag(tag) => {
                let mut out = Vec::new();
                for element in crate::lkdyn::dyn_list_values(value).iter() {
                    out.push(to_serde(*element, depth + 1)?);
                }
                serde_json::Value::Array(out)
            }
            // A `Bytes` and a `Set` are the VM's refusals, by their type names.
            DYN_BYTES => return Err("Bytes has no JSON form".to_string()),
            DYN_SET => return Err("Set has no JSON form".to_string()),
            DYN_RAW => return Err("Object has no JSON form".to_string()),
            // A window is a list, and the VM encodes it as one:
            // `json.stringify([xs.slice(0, 2)])` is `[[1,2]]` there and was
            // `value has no JSON form` here. `DYN_SLICE` was added to the tag
            // space after this match was written and the catch-all swallowed
            // it — the third arm in this runtime to lose a carrier that way
            // (see `contains_eq`, and `container_ty` in the lowering).
            DYN_SLICE => {
                let mut out = Vec::new();
                // SAFETY: a `DYN_SLICE` payload is a live window handle — the
                // tag is only ever set by `lkrt_dyn_from_slice`.
                let handle = value.payload as *mut core::ffi::c_void;
                let len = unsafe { crate::lkslice::lkrt_lkslice_i64_len(handle) };
                for index in 0..len {
                    // SAFETY: as above, and `index` is inside `len`.
                    let element = unsafe { crate::lkslice::lkrt_lkslice_i64_get_pair(handle, index) };
                    out.push(serde_json::Value::Number(element.value.into()));
                }
                serde_json::Value::Array(out)
            }
            tag if is_map_tag(tag) => {
                let mut out = serde_json::Map::new();
                for (key, element) in map_entries(value) {
                    out.insert(object_key(&key)?, to_serde(element, depth + 1)?);
                }
                serde_json::Value::Object(out)
            }
            _ => return Err("value has no JSON form".to_string()),
        })
    }

    /// A JSON object key, or the VM's refusal — verbatim, because a caught
    /// error's message is program output and this one *tells the program what
    /// to write instead*.
    fn object_key(key: &RtKey) -> Result<String, String> {
        match key {
            RtKey::ShortStr(_) | RtKey::String(_) => Ok(key_str(key).to_string()),
            RtKey::Int(value) => Err(format!(
                "a JSON object key is a String, and `{value}` is an Int — write it as \"{value}\" if that is what you mean"
            )),
            RtKey::Bool(value) => Err(format!("a JSON object key is a String, and `{value}` is a Bool")),
            RtKey::Nil => Err("a JSON object key is a String, and `nil` is not one".to_string()),
            RtKey::Obj(_) => Err("a JSON object key is a String".to_string()),
        }
    }
}

/// `encoding.json.stringify(value)` — compact, the `serde_json::Value`
/// `Display`.
///
/// The raise carries the member's name in front of the reason, which is the
/// stdlib's `write_format` wrapper doing it there.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_json_stringify(value: LkDyn) -> *mut c_char {
    match write::to_serde(value, 0).map(|value| value.to_string()) {
        Ok(text) => arena_c_string(CString::new(text).unwrap_or_default()),
        Err(message) => crate::panic::raise_str(&format!("encoding.json.stringify: {message}")),
    }
}

/// `encoding.yaml.stringify(value)`.
#[cfg(feature = "std")]
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_yaml_stringify(value: LkDyn) -> *mut c_char {
    let text = write::to_serde(value, 0)
        .and_then(|value| serde_yaml::to_string(&value).map_err(|error| format!("cannot write YAML: {error}")));
    match text {
        Ok(text) => arena_c_string(CString::new(text).unwrap_or_default()),
        Err(message) => crate::panic::raise_str(&format!("encoding.yaml.stringify: {message}")),
    }
}

/// `encoding.toml.stringify(value)`.
///
/// A TOML document *is* a table, so a top-level scalar or array is refused
/// rather than written out as something no TOML parser reads back — the VM's
/// rule, in the VM's words.
#[cfg(feature = "std")]
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_toml_stringify(value: LkDyn) -> *mut c_char {
    let text = write::to_serde(value, 0).and_then(|value| {
        if !value.is_object() {
            return Err("a TOML document is a table, so the top level must be a map".to_string());
        }
        toml::to_string(&value).map_err(|error| format!("cannot write TOML: {error}"))
    });
    match text {
        Ok(text) => arena_c_string(CString::new(text).unwrap_or_default()),
        Err(message) => crate::panic::raise_str(&format!("encoding.toml.stringify: {message}")),
    }
}
