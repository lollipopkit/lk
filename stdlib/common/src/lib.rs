//! Shared runtime helpers for LK standard library modules.
//!
//! Builds no_std (+ alloc) under `--no-default-features` so that the
//! computation-only stdlib modules can be offered on bare metal. See
//! `stdlib/bare`.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// From `alloc` directly, not `lk_core::compat::prelude`: feature
// unification can give lk-core `std` while this crate stays no_std, and
// then that prelude does not exist.
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

pub mod language;
pub mod metadata;
pub mod resource;
pub mod runtime_native;

pub use lk_stdlib_macros::{StdlibModule, stdlib_exports};

#[macro_export]
macro_rules! stdlib_runtime_exports {
    ([$($kind:ident $name:literal => $function:path, $arity:expr),* $(,)?] $(, [$($value_name:literal => $value:expr),* $(,)?])? $(,)?) => {
        ::lk_core::module::runtime_export_from_plain_native_entries(
            &[
                $(
                    ::lk_core::module::RuntimeNativeExport::$kind($name, $function, $arity),
                )*
            ],
            &[
                $($(
                    ::lk_core::module::RuntimeValueExport::new($value_name, $value),
                )*)?
            ],
        )
    };
}

#[macro_export]
macro_rules! stdlib_register_runtime_builtins {
    ($registry:expr, [$($kind:ident $name:literal => $function:path, $arity:expr),* $(,)?] $(,)?) => {{
        $(
            $registry.register_runtime_builtin(
                $name,
                $crate::stdlib_register_runtime_builtins!(@function $kind, $function),
                $arity,
            );
        )*
    }};
    (@function plain, $function:path) => {
        ::lk_core::vm::NativeFunction::Plain($function)
    };
    (@function full_state, $function:path) => {
        ::lk_core::vm::NativeFunction::FullState($function)
    };
}

use alloc::sync::Arc;
use lk_core::{
    val,
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
};

/// A duration argument, in milliseconds.
///
/// Rejects a negative **before** truncating, which is where the spellings of
/// this one operation used to disagree — four answers for the same call:
///
/// | | `-1` | `-0.5` |
/// | --- | --- | --- |
/// | `time.sleep` | `Duration::from_millis(-1 as u64)` — a 584-million-year sleep | returned at once |
/// | `task.sleep` | refused | returned at once |
/// | `time.timeout` / `time.after` | a timer that never fires, silently | — |
///
/// The `-0.5` column is the reason the check has to come first: `task.sleep`
/// *had* a `< 0` guard, and it ran after `as i64` had already turned the value
/// into `0`.
pub fn duration_millis(value: &RuntimeVal, name: &str) -> anyhow::Result<i64> {
    let ms = match value {
        RuntimeVal::Int(ms) => *ms as f64,
        RuntimeVal::Float(ms) => *ms,
        other => {
            return Err(anyhow::anyhow!(
                "{name} expects a numeric argument, got {:?}",
                other.kind()
            ));
        }
    };
    if ms < 0.0 {
        return Err(anyhow::anyhow!(
            "{name} expects a non-negative duration in milliseconds, got {ms}"
        ));
    }
    Ok(ms as i64)
}

pub fn typed_list_from_values(values: Vec<RuntimeVal>, heap: &HeapStore) -> TypedList {
    if values.is_empty() {
        return TypedList::Mixed(values);
    }

    let mut ints: Option<Vec<i64>> = None;
    let mut floats: Option<Vec<f64>> = None;
    let mut bools: Option<Vec<bool>> = None;
    let mut strings: Option<Vec<Arc<str>>> = None;
    for value in &values {
        match value {
            RuntimeVal::Int(value) if floats.is_none() && bools.is_none() && strings.is_none() => {
                ints.get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(*value);
            }
            RuntimeVal::Float(value) if ints.is_none() && bools.is_none() && strings.is_none() => {
                floats
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(*value);
            }
            RuntimeVal::Bool(value) if ints.is_none() && floats.is_none() && strings.is_none() => {
                bools
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(*value);
            }
            RuntimeVal::ShortStr(value) if ints.is_none() && floats.is_none() && bools.is_none() => {
                strings
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(Arc::<str>::from(value.as_str()));
            }
            RuntimeVal::Obj(handle) if ints.is_none() && floats.is_none() && bools.is_none() => {
                let Some(HeapValue::String(value)) = heap.get(*handle) else {
                    return TypedList::Mixed(values);
                };
                strings
                    .get_or_insert_with(|| Vec::with_capacity(values.len()))
                    .push(value.clone());
            }
            _ => return TypedList::Mixed(values),
        }
    }

    if let Some(ints) = ints {
        TypedList::Int(ints)
    } else if let Some(floats) = floats {
        TypedList::Float(floats)
    } else if let Some(bools) = bools {
        TypedList::Bool(bools)
    } else if let Some(strings) = strings {
        TypedList::String(strings)
    } else {
        TypedList::Mixed(values)
    }
}

pub fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = val::ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

#[cfg(test)]
mod duration_tests {
    use super::*;

    /// A negative duration is refused, and refused *before* truncation.
    ///
    /// One operation had four answers: `time.sleep(-1)` cast to `u64` and slept
    /// for 584 million years, `task.sleep(-1)` refused, `time.timeout(-1)` and
    /// `time.after(-1)` armed a timer that never fires — and every one of them
    /// accepted `-0.5`, because the only `< 0` check ran after `as i64` had
    /// already turned it into `0`.
    #[test]
    fn a_duration_cannot_be_negative() {
        for value in [RuntimeVal::Int(-1), RuntimeVal::Float(-0.5), RuntimeVal::Float(-1e-9)] {
            let err = duration_millis(&value, "time.sleep()").expect_err("refused");
            assert!(
                err.to_string().contains("non-negative duration in milliseconds"),
                "{err}"
            );
        }
        assert_eq!(
            duration_millis(&RuntimeVal::Int(0), "x").expect("zero is a duration"),
            0
        );
        assert_eq!(duration_millis(&RuntimeVal::Int(7), "x").expect("positive"), 7);
        // Truncation toward zero is unchanged for the values that are allowed.
        assert_eq!(
            duration_millis(&RuntimeVal::Float(0.9), "x").expect("sub-millisecond"),
            0
        );
        assert!(duration_millis(&RuntimeVal::Bool(true), "x").is_err());
    }
}
