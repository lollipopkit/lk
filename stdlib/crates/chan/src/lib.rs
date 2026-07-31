// Channel module for LK.
//
// The module-level API uses the RuntimeNative ABI. Global concurrency
// builtins are still registered separately while compiler lowering is being
// migrated.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    rt::RuntimePayload,
    val::{ChannelValue, HeapStore, HeapValue, RuntimeVal},
    vm::{NativeArgs, NativeRuntime},
};
use std::sync::Arc;

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "chan", docs = "Channel operations for inter-task communication")]
pub struct ChannelModule;

/// Creates a channel — the implementation behind both `chan.new(…)` and the
/// bare `chan(…)` global.
///
/// Both spellings exist because importing the module *shadows* the global:
/// after `use chan;` the name is the module, so `chan(3)` stopped being a call
/// at all and there was no way left to make a channel. One implementation, two
/// names, and the module is now complete on its own.
/// `chan(capacity[, type])` — a capacity, and an optional type hint.
///
/// Public because the registration tells the type checker the same numbers, and
/// they are these ones.
pub const CHAN_ARITY: (u16, u16) = (1, 2);

pub fn create_channel_value(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.len() < CHAN_ARITY.0 as usize || args.len() > CHAN_ARITY.1 as usize {
        bail!("chan() expects 1 or 2 arguments: capacity[, type_str]");
    }
    let values = args.as_slice();
    let capacity = match &values[0] {
        RuntimeVal::Int(value) => *value,
        RuntimeVal::Float(value) => *value as i64,
        other => bail!("chan() capacity must be numeric, got {:?}", other.kind()),
    };
    let inner_type = if values.len() == 2 {
        match &values[1] {
            RuntimeVal::Nil => lk_core::val::Type::Nil,
            value => {
                let text = runtime_native::runtime_string_arg(value, runtime.heap(), "chan() type")?;
                lk_core::val::Type::parse(text.as_ref()).unwrap_or(lk_core::val::Type::Nil)
            }
        }
    } else {
        lk_core::val::Type::Nil
    };
    // `0` is *unbuffered*, as it is in every channel API a reader has seen —
    // not unbounded, which is what it used to mean here. The runtime's mpsc has
    // no true rendezvous form, so `0` takes the smallest bound it offers.
    if capacity < 0 {
        bail!("chan() capacity cannot be negative, got {capacity}");
    }
    let channel_id = runtime
        .async_runtime()
        .with(|runtime| runtime.create_channel(Some((capacity as usize).max(1))))
        .map_err(|error| anyhow!("Failed to create channel: {error}"))?;
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Channel(Arc::new(
        ChannelValue {
            id: channel_id,
            capacity: Some(capacity),
            inner_type,
        },
    )))))
}

/// Blocking send — the implementation behind both `chan.send(c, v)` and the
/// bare `send(c, v)` global.
///
/// Returns Nil on delivery and raises a catchable error once the channel is
/// closed (v2 error model: failures raise, they don't return status values —
/// Go's panic-on-closed-send).
pub fn blocking_send_value(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>, name: &str) -> Result<RuntimeVal> {
    let values = args.as_slice();
    if values.len() != 2 {
        bail!("{name} expects 2 arguments: channel, value");
    }
    let channel = channel_arg(&values[0], runtime.heap(), name)?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = runtime
        .async_runtime()
        .with(|rt| rt.block_on(rt.guard_blocking("send", rt.send_async(channel.id, value))))
        .map_err(|error| anyhow!("Send operation failed: {error}"))?;
    if !sent {
        bail!("send on closed channel");
    }
    Ok(RuntimeVal::Nil)
}

/// Blocking receive — the implementation behind both `chan.recv(c)` and the
/// bare `recv(c)` global.
///
/// Returns the value; raises a catchable error once the channel is closed and
/// drained (no `[ok, value]` pairs — a consume-until-closed loop wraps itself
/// in try/catch, or polls `chan.is_closed`/`chan.try_recv`).
pub fn blocking_recv_value(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>, name: &str) -> Result<RuntimeVal> {
    let values = args.as_slice();
    if values.len() != 1 {
        bail!("{name} expects 1 argument: channel");
    }
    let channel = channel_arg(&values[0], runtime.heap(), name)?;
    let (ok, value) = runtime
        .async_runtime()
        .with(|rt| rt.block_on(rt.guard_blocking("recv", rt.recv_async(channel.id))))
        .map_err(|error| anyhow!("Receive operation failed: {error}"))?;
    if !ok {
        bail!("receive on closed channel");
    }
    value.into_value(runtime.heap_mut())
}

#[lk_stdlib_common::stdlib_exports(module = "chan", runtime_builtins = true)]
impl ChannelModule {
    /// `chan.new(capacity[, type])` — the module spelling of the `chan(…)`
    /// global, and the only one reachable after `use chan;`.
    #[stdlib_export(name = "new", params(capacity: Int, type?: String), returns = Channel)]
    fn new_channel(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        create_channel_value(args, runtime)
    }

    #[stdlib_export(name = "close", params(channel: Channel), returns = Nil)]
    fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.close()")?;
        runtime
            .async_runtime()
            .with(|runtime| runtime.close_channel(channel.id))
            .map_err(|err| anyhow!("Failed to close channel: {err}"))?;
        Ok(RuntimeVal::Nil)
    }

    #[stdlib_export(name = "len", params(channel: Channel), returns = Int)]
    fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.len()")?;
        let len = runtime
            .async_runtime()
            .with(|runtime| runtime.channel_len(channel.id))
            .map_err(|err| anyhow!("Failed to read channel length: {err}"))?;
        Ok(RuntimeVal::Int(len as i64))
    }

    #[stdlib_export(name = "capacity", params(channel: Channel), returns = Int)]
    fn capacity(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.capacity()")?;
        Ok(RuntimeVal::Int(channel.capacity.unwrap_or(0)))
    }

    #[stdlib_export(name = "is_closed", params(channel: Channel), returns = Bool)]
    fn is_closed(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.is_closed()")?;
        let closed = runtime
            .async_runtime()
            .with(|runtime| runtime.channel_is_closed(channel.id))
            .map_err(|err| anyhow!("Failed to read channel closed state: {err}"))?;
        Ok(RuntimeVal::Bool(closed))
    }

    /// `chan.send(c, v)` — blocking send, the module spelling of the `send`
    /// global. The module had `try_send` but not this, so `use chan;` produced a
    /// channel you could only poll: the blocking half was reachable only through
    /// an unqualified global.
    #[stdlib_export(name = "send", params(channel: Channel, value: Any), returns = Nil)]
    fn send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        blocking_send_value(args, runtime, "chan.send()")
    }

    /// `chan.recv(c)` — blocking receive, the module spelling of the `recv`
    /// global.
    #[stdlib_export(name = "recv", params(channel: Channel), returns = Any)]
    fn recv(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        blocking_recv_value(args, runtime, "chan.recv()")
    }

    #[stdlib_export(name = "try_send", params(channel: Channel, value: Any), returns = Bool)]
    fn try_send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let channel = channel_arg(&values[0], runtime.heap(), "chan.try_send()")?;
        let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
        let sent = runtime
            .async_runtime()
            .with(|runtime| runtime.try_send(channel.id, value))
            .map_err(|err| anyhow!("Failed to send to channel: {err}"))?;
        Ok(RuntimeVal::Bool(sent))
    }

    /// Non-blocking receive: the value when one is ready, `nil` when empty
    /// (pairs with postfix `!` to assert), raises once the channel closed.
    #[stdlib_export(name = "try_recv", params(channel: Channel), returns = Any)]
    fn try_recv(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.try_recv()")?;
        match runtime
            .async_runtime()
            .with(|rt| rt.try_recv(channel.id))
            .map_err(|err| anyhow!("Failed to receive from channel: {err}"))?
        {
            Some((true, value)) => value.into_value(runtime.heap_mut()),
            Some((false, _)) => Err(anyhow!("receive on closed channel")),
            None => Ok(RuntimeVal::Nil),
        }
    }
}

fn channel_arg(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<Arc<ChannelValue>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{name} expects a Channel argument");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::Channel(channel) => Ok(channel.clone()),
        other => Err(anyhow!("{name} expects a Channel argument, got {}", other.type_name())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{
        rt::AsyncRuntimeHandle,
        val::{ShortStr, Type},
        vm::{NativeFunction, RuntimeModuleState, VmContext},
    };

    fn chan_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&ChannelModule::new(), name)
    }

    fn runtime_channel(capacity: i64, heap: &mut HeapStore, handle: &AsyncRuntimeHandle) -> Result<RuntimeVal> {
        let id = handle.with(|runtime| runtime.create_channel(Some(capacity as usize)))?;
        Ok(RuntimeVal::Obj(heap.alloc(HeapValue::Channel(Arc::new(
            ChannelValue {
                id,
                capacity: Some(capacity),
                inner_type: Type::Nil,
            },
        )))))
    }

    // Share one VmContext (hence one async runtime) across channel creation and
    // every call so the channel is visible to send/recv, matching real VM
    // execution where native calls always receive the running context.
    fn call(
        name: &str,
        args: &[RuntimeVal],
        state: &mut RuntimeModuleState,
        ctx: &mut VmContext,
    ) -> Result<RuntimeVal> {
        let (_, function) = chan_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, Some(ctx), None);
        function(NativeArgs::new(args), &mut runtime)
    }

    #[test]
    fn chan_exports_use_runtime_native() -> Result<()> {
        for name in ["close", "len", "capacity", "is_closed", "try_send", "try_recv"] {
            let (arity, function) = chan_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry::VARIADIC);
        }
        // `new` alone is variadic: its element type is optional, and an
        // optional parameter means the call site can supply fewer arguments
        // than the declaration lists.
        let (arity, function) = chan_native("new")?;
        assert!(matches!(function, NativeFunction::Plain(_)));
        assert_eq!(arity, lk_core::vm::NativeEntry::VARIADIC);
        Ok(())
    }

    #[test]
    fn chan_capacity_len_and_is_closed_use_runtime_channel() -> Result<()> {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let mut state = RuntimeModuleState::default();
        let channel = runtime_channel(3, state.heap_mut(), ctx.async_runtime())?;
        assert_eq!(
            call("capacity", std::slice::from_ref(&channel), &mut state, &mut ctx)?,
            RuntimeVal::Int(3)
        );
        assert_eq!(
            call("len", std::slice::from_ref(&channel), &mut state, &mut ctx)?,
            RuntimeVal::Int(0)
        );
        assert_eq!(
            call("is_closed", std::slice::from_ref(&channel), &mut state, &mut ctx)?,
            RuntimeVal::Bool(false)
        );
        Ok(())
    }

    #[test]
    fn chan_try_send_and_recv_round_trips_runtime_values() -> Result<()> {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let mut state = RuntimeModuleState::default();
        let channel = runtime_channel(1, state.heap_mut(), ctx.async_runtime())?;
        let value = RuntimeVal::ShortStr(ShortStr::new("payload").expect("short string"));
        assert_eq!(
            call("try_send", &[channel, value], &mut state, &mut ctx)?,
            RuntimeVal::Bool(true)
        );

        let received = call("try_recv", std::slice::from_ref(&channel), &mut state, &mut ctx)?;
        assert_eq!(
            received,
            RuntimeVal::ShortStr(ShortStr::new("payload").expect("short string"))
        );
        Ok(())
    }

    #[test]
    fn chan_try_recv_empty_returns_nil() -> Result<()> {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let mut state = RuntimeModuleState::default();
        let channel = runtime_channel(1, state.heap_mut(), ctx.async_runtime())?;
        let received = call("try_recv", std::slice::from_ref(&channel), &mut state, &mut ctx)?;
        assert_eq!(received, RuntimeVal::Nil);
        Ok(())
    }

    /// Importing the module shadows the bare `chan(…)` global — they share the
    /// name — so before `chan.new` existed, `use chan;` left no way at all to
    /// create a channel.
    #[test]
    fn chan_new_creates_a_channel_and_rejects_a_negative_capacity() -> Result<()> {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let mut state = RuntimeModuleState::default();

        let created = call("new", &[RuntimeVal::Int(3)], &mut state, &mut ctx)?;
        assert_eq!(
            call("capacity", std::slice::from_ref(&created), &mut state, &mut ctx)?,
            RuntimeVal::Int(3)
        );

        // Zero is *unbuffered*, and it is a capacity like any other — not the
        // unbounded queue it used to mean.
        let unbuffered = call("new", &[RuntimeVal::Int(0)], &mut state, &mut ctx)?;
        assert_eq!(
            call("capacity", std::slice::from_ref(&unbuffered), &mut state, &mut ctx)?,
            RuntimeVal::Int(0)
        );

        let error = call("new", &[RuntimeVal::Int(-1)], &mut state, &mut ctx).expect_err("negative capacity");
        assert!(error.to_string().contains("cannot be negative"), "{error}");
        Ok(())
    }
}
