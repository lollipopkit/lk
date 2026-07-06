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

#[lk_stdlib_common::stdlib_exports(module = "chan", runtime_builtins = true)]
impl ChannelModule {
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
            call("try_send", &[channel.clone(), value], &mut state, &mut ctx)?,
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
}
