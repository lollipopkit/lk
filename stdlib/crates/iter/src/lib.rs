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

use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeRuntime},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}
pub use lk_stdlib_common::typed_list_from_values;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "iter", docs = "List-oriented iterator utilities")]
pub struct IterModule;

#[lk_stdlib_common::stdlib_exports(module = "iter")]
impl IterModule {
    // `List | Slice | Bytes`, because a window and a `Bytes` have elements too and
    // this forwards to the method that reads them. Only the exports whose result
    // does *not* depend on which sequence came in can widen: `take` on a `Bytes`
    // answers `Bytes`, which no single declared return type can say.
    #[stdlib_export(params(values: List<_> | Slice<_> | Bytes, f: Fn), returns = List, kind = "full_state")]
    fn map(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("map", args, runtime)
    }

    #[stdlib_export(params(values: List<_>, predicate: Fn), returns = List, kind = "full_state")]
    fn filter(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("filter", args, runtime)
    }

    // The three reductions, forwarded like the rest: the module spelling is the
    // method with the receiver written first, and a method that had no module
    // spelling would be the kind of half-surface this module exists to avoid.
    #[stdlib_export(params(values: List<_> | Slice<_> | Bytes), returns = Any, kind = "full_state")]
    fn min(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("min", args, runtime)
    }

    #[stdlib_export(params(values: List<_> | Slice<_> | Bytes), returns = Any, kind = "full_state")]
    fn max(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("max", args, runtime)
    }

    #[stdlib_export(params(values: List<_> | Slice<_> | Bytes), returns = Any, kind = "full_state")]
    fn sum(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("sum", args, runtime)
    }

    #[stdlib_export(params(values: List<_> | Slice<_> | Bytes, initial: Any, f: Fn), returns = Any, kind = "full_state")]
    fn reduce(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("reduce", args, runtime)
    }

    #[stdlib_export(params(values: List<_>), returns = List, kind = "full_state")]
    fn enumerate(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("enumerate", args, runtime)
    }

    #[stdlib_export(params(stop: Int; start: Int, stop: Int, step?: Int), returns = List)]
    fn range(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let (start, end, step) = match values {
            [end] => (0, int_arg(end, "iter.range end")?, 1),
            [start, end] => (int_arg(start, "iter.range start")?, int_arg(end, "iter.range end")?, 1),
            [start, end, step] => (
                int_arg(start, "iter.range start")?,
                int_arg(end, "iter.range end")?,
                int_arg(step, "iter.range step")?,
            ),
            _ => bail!("iter.range expects (end), (start, end), or (start, end, step)"),
        };
        if step == 0 {
            bail!("iter.range step cannot be zero");
        }

        let mut out = Vec::new();
        let mut current = start;
        if step > 0 {
            while current < end {
                out.push(current);
                current += step;
            }
        } else {
            while current > end {
                out.push(current);
                current += step;
            }
        }
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::Int(out))),
        ))
    }

    #[stdlib_export(params(left: List<_>, right: List<_>), named(right), returns = List, kind = "full_state")]
    fn zip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("zip", args, runtime)
    }

    #[stdlib_export(params(values: List<_>, count: Int), returns = List, kind = "full_state")]
    fn take(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("take", args, runtime)
    }

    #[stdlib_export(params(values: List<_>, count: Int), returns = List, kind = "full_state")]
    fn skip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("skip", args, runtime)
    }

    #[stdlib_export(params(left: List<_>, right: List<_>), named(right), returns = List, kind = "full_state")]
    fn chain(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("chain", args, runtime)
    }

    #[stdlib_export(params(values: List<_>), returns = List, kind = "full_state")]
    fn flatten(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("flatten", args, runtime)
    }

    #[stdlib_export(params(values: List<_>), returns = List, kind = "full_state")]
    fn unique(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("unique", args, runtime)
    }

    #[stdlib_export(params(values: List<_>, size: Int), returns = List, kind = "full_state")]
    fn chunk(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("chunk", args, runtime)
    }

    /// `iter.next(xs)` is `xs.first()` — the name is the iterator vocabulary,
    /// the operation is the list one.
    #[stdlib_export(params(values: List<_> | Slice<_> | Bytes), returns = Any, kind = "full_state")]
    fn next(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("first", args, runtime)
    }

    #[stdlib_export(params(values: List<_>), returns = List)]
    fn collect(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        // The one export with no method behind it: a list *is* the iterator
        // here, so "collect" means "copy", and no list method spells that.
        let copied = {
            let input = typed_list_arg_ref(&args.as_slice()[0], runtime.heap(), "iter.collect")?;
            input.window(0, input.len())
        };
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(copied))))
    }
}

/// Call the built-in method `method` on the first argument, passing the rest.
///
/// Every `iter.f(xs, ...)` above is defined as `xs.f(...)`, which is the whole
/// point of this module now: the module form is a *spelling* of the method
/// form, not a second implementation of it. The two used to be written out
/// separately — 14 exports' worth of snapshotting, truthiness, host-root
/// pinning and result-list construction, each of which had to be kept in step
/// with `core_methods` by whoever remembered. They agreed, when this was
/// written, on every case tested; `take(-1)` was the one that did not, and it
/// took a deliberate comparison to find.
fn forward(method: &'static str, args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some((receiver, rest)) = values.split_first() else {
        bail!("iter.{method} expects a list as its first argument");
    };
    lk_core::vm::core_call_method_windowed(*receiver, method, rest, runtime)
}
/// The one argument check every export shares: the receiver must be a list.
fn typed_list_arg_ref<'a>(value: &RuntimeVal, heap: &'a HeapStore, context: &str) -> Result<&'a TypedList> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(list),
        _ => bail!("{context} expects a list"),
    }
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be an integer")),
    }
}
