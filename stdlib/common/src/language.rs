//! The globals the *language* is written in terms of, whatever the host is.
//!
//! Everything else in the standard library is a module a program asks for by
//! name, and a host that cannot back one says so — `use fs` on bare metal
//! answers "not available on bare metal". `error` is not that: it is the global
//! `catch` catches, and a host without it turns every raising program into
//! "undefined function" at run time, after the parser and the type checker have
//! both approved it. `bare-metal-x86`'s interpreter answered exactly that, and
//! the browser playground did too — both build their global list by hand, and
//! both lists were written before `error` was.
//!
//! `try$call` is here for a different reason: nothing calls it. `try`/`catch`
//! compiles to `TryBegin`/`TryEnd` opcodes now, and the desugar-to-a-call it is
//! named after is history. The umbrella host still registers it, so the
//! implementation lives here rather than in two places — but a host that leaves
//! it out loses nothing, which is why the two bare hosts do not register it.
//!
//! Nothing here needs an OS: these call through the VM's own machinery and
//! allocate from its heap, both of which a bare host has.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use anyhow::{Result, anyhow};
use lk_core::val::{HeapValue, RuntimeVal, TypedList};
use lk_core::vm::{NativeArgs, NativeRuntime, call_runtime_value_runtime};

use crate::runtime_native::runtime_display_value;

/// `error(value)` — raise, carrying `value` itself where it can be carried.
///
/// A raised heap value has to survive the collection that can happen at any
/// native-call safepoint while the error unwinds, so it is pinned as a GC root
/// until a `try$call` catches it. A primitive is `Copy` and needs no pinning; a
/// host with no full VM state cannot pin at all, and falls back to the rendered
/// message — the value is lost, the report is not.
pub fn error(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if let [value] = args.as_slice() {
        let value = *value;
        // Rendered up front: once the error unwinds out of the program, the heap
        // it points into is gone and there is nothing left to render.
        let rendered = joined_display(args.as_slice(), runtime)?;
        let carry_first_class = if matches!(value, RuntimeVal::Obj(_)) {
            match runtime.state_ctx_module_mut() {
                Some((state, _, _)) => {
                    state.set_pending_raise_root(Some(value));
                    true
                }
                None => false,
            }
        } else {
            true
        };
        if carry_first_class {
            return Err(anyhow!(lk_core::vm::LkRaisedValue {
                value,
                rendered: Arc::<str>::from(rendered.as_str()),
            }));
        }
        return Err(anyhow!("{rendered}"));
    }
    let message = if args.is_empty() {
        alloc::string::String::from("error")
    } else {
        joined_display(args.as_slice(), runtime)?
    };
    Err(anyhow!("{message}"))
}

/// `try$call(f, args…) -> [ok, result_or_error]` — the protected call `try`
/// desugars to.
///
/// Answers `[true, result]` or `[false, error]` rather than propagating. A
/// first-class error value round-trips as itself; anything else arrives as its
/// message.
pub fn try_call(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some((&callee, call_args)) = values.split_first() else {
        return Err(anyhow!("try$call expects at least 1 argument: the function to call"));
    };
    let call_args = call_args.to_vec();
    let outcome = {
        let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
            return Err(anyhow!("try$call requires full VM state"));
        };
        call_runtime_value_runtime(callee, &call_args, state, module, ctx)
    };
    if outcome.is_err()
        && let Some((state, ctx, _)) = runtime.state_ctx_module_mut()
    {
        // Caught here, so the root pin is released: the value stays valid for
        // the allocations below, and stops being a stray root once the program
        // resumes.
        state.set_pending_raise_root(None);
        // The frames the failed call accumulated are not part of a later,
        // unrelated report.
        if let Some(ctx) = ctx {
            ctx.truncate_call_stack(0);
        }
    }
    let (ok, value) = match outcome {
        Ok(result) => (true, result),
        Err(err) => {
            // The call machinery adds context, so the raised value is at the
            // deepest cause rather than at the top.
            let root = err.root_cause();
            if let Some(raised) = root.downcast_ref::<lk_core::vm::LkRaisedValue>() {
                (false, raised.value)
            } else {
                let message = alloc::format!("{root}");
                let handle = runtime
                    .heap_mut()
                    .alloc(HeapValue::String(Arc::<str>::from(message.as_str())));
                (false, RuntimeVal::Obj(handle))
            }
        }
    };
    let list = runtime
        .heap_mut()
        .alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Bool(ok), value])));
    Ok(RuntimeVal::Obj(list))
}

fn joined_display(values: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<alloc::string::String> {
    let mut out = alloc::string::String::new();
    let rendered: Vec<alloc::string::String> = values
        .iter()
        .map(|value| runtime_display_value(value, runtime.heap()))
        .collect::<Result<_>>()?;
    for (index, piece) in rendered.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(piece);
    }
    Ok(out)
}
