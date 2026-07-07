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

use core::ffi::{CStr, c_char};
use std::ffi::CString;

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_LIST, DYN_MAP, LkDyn};
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_yaml_parse(text: *const c_char) -> LkDyn {
    match serde_yaml::from_str::<serde_yaml::Value>(input(text)) {
        Ok(value) => yaml_to_dyn(value),
        Err(_) => crate::panic::raise_str("Invalid YAML"),
    }
}

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_toml_parse(text: *const c_char) -> LkDyn {
    match toml::from_str::<toml::Value>(input(text)) {
        Ok(value) => toml_to_dyn(value),
        Err(_) => crate::panic::raise_str("Invalid TOML"),
    }
}
