#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// From `alloc` directly, not `lk_core::compat::prelude`: feature
// unification can give lk-core `std` while this crate stays no_std, and
// then that prelude does not exist. What alloc provides does not depend
// on anyone else's features.
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

use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeVal},
    vm::{NativeArgs, NativeRuntime},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::runtime_string_arg;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "bytes", docs = "Byte buffer helpers")]
pub struct BytesModule;

pub fn runtime_bytes_value(bytes: impl Into<Arc<[u8]>>, heap: &mut HeapStore) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::Bytes(bytes.into())))
}

pub fn runtime_bytes_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<[u8]>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects Bytes");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Bytes(value) => Ok(value.clone()),
        other => bail!("{context} expects Bytes, got {}", other.type_name()),
    }
}

pub fn runtime_bytes_or_string_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<[u8]>> {
    match runtime_bytes_arg(value, heap, context) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Ok(Arc::<[u8]>::from(runtime_string_arg(value, heap, context)?.as_bytes())),
    }
}

#[lk_stdlib_common::stdlib_exports]
impl BytesModule {
    /// `xs.to_bytes()`, spelled as a constructor.
    ///
    /// The body is the method's, as everywhere else in this module: a module
    /// function whose first parameter is the receiver **is** the method, and
    /// two bodies for one operation is how `bytes.slice(b, 3, 1)` came to raise
    /// while `b.slice(3, 1)` answered an empty window.
    #[stdlib_export(name = "from_list", params(values: List), returns = Bytes)]
    fn from_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("to_bytes", args, runtime)
    }

    /// `s.bytes()`, spelled as a constructor — one operation, and now one body,
    /// even though the two spellings live in different modules.
    #[stdlib_export(name = "from_string", params(value: String), returns = Bytes)]
    fn from_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("bytes", args, runtime)
    }

    #[stdlib_export(name = "len", params(value: Bytes), returns = Int)]
    fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("len", args, runtime)
    }

    #[stdlib_export(name = "is_empty", params(value: Bytes), returns = Bool)]
    fn is_empty(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("is_empty", args, runtime)
    }

    #[stdlib_export(name = "get", params(value: Bytes, index: Int), returns = Int?)]
    fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("get", args, runtime)
    }

    #[stdlib_export(name = "first", params(value: Bytes), returns = Int?)]
    fn first(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("first", args, runtime)
    }

    #[stdlib_export(name = "last", params(value: Bytes), returns = Int?)]
    fn last(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("last", args, runtime)
    }

    #[stdlib_export(name = "contains", params(value: Bytes, byte: Int), returns = Bool)]
    fn contains(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("contains", args, runtime)
    }

    #[stdlib_export(name = "index_of", params(value: Bytes, byte: Int), returns = Int?)]
    fn index_of(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("index_of", args, runtime)
    }

    #[stdlib_export(name = "sum", params(value: Bytes), returns = Int)]
    fn sum(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("sum", args, runtime)
    }

    #[stdlib_export(name = "min", params(value: Bytes), returns = Int?)]
    fn min(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("min", args, runtime)
    }

    #[stdlib_export(name = "max", params(value: Bytes), returns = Int?)]
    fn max(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("max", args, runtime)
    }

    #[stdlib_export(name = "take", params(value: Bytes, count: Int), returns = Bytes)]
    fn take(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("take", args, runtime)
    }

    #[stdlib_export(name = "skip", params(value: Bytes, count: Int), returns = Bytes)]
    fn skip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("skip", args, runtime)
    }

    /// Window positions: negative counts from the end and out of range clamps,
    /// including a reversed window, which is empty rather than a raise. The
    /// module used to refuse `end < start` while the method answered `Bytes([])`
    /// — the last surviving difference between the two spellings.
    #[stdlib_export(name = "slice", params(value: Bytes, start: Int, end?: Int), named(start, end), returns = Bytes)]
    fn slice(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("slice", args, runtime)
    }

    #[stdlib_export(name = "to_list", params(value: Bytes), returns = List)]
    fn to_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("to_list", args, runtime)
    }

    #[stdlib_export(name = "to_string_utf8", params(value: Bytes), returns = String, docs = "Decodes bytes as UTF-8 and raises an error for invalid input.")]
    fn to_string_utf8(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("to_string_utf8", args, runtime)
    }

    #[stdlib_export(name = "to_string_lossy", params(value: Bytes), returns = String)]
    fn to_string_lossy(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("to_string_lossy", args, runtime)
    }

    #[stdlib_export(name = "concat", params(left: Bytes, right: Bytes), named(right), returns = Bytes)]
    fn concat(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("concat", args, runtime)
    }
}

/// The module spelling of a method: the receiver written first.
///
/// See `lk_stdlib_string::forward` for why this shape rather than a second
/// body — this module is the one that proved the point twice, with `get` and
/// then with `slice`.
fn forward(method: &'static str, args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some((receiver, rest)) = values.split_first() else {
        bail!("bytes.{method} expects its receiver as the first argument");
    };
    lk_core::vm::core_call_method_windowed(*receiver, method, rest, runtime)
}
