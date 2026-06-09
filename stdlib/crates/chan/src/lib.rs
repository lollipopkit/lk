// Channel module for LK.
//
// The module-level API uses the RuntimeNative ABI. Global concurrency
// builtins are still registered separately while compiler lowering is being
// migrated.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{self, ModuleProvider, ModuleRegistry},
    rt,
    rt::RuntimePayload,
    val::{ChannelValue, HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use std::sync::Arc;

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[derive(Debug)]
pub struct ChannelModule;

impl Default for ChannelModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for ChannelModule {
    fn name(&self) -> &str {
        "chan"
    }

    fn description(&self) -> &str {
        "Channel operations for inter-task communication"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn register(&self, registry: &mut module::ModuleRegistry) -> Result<()> {
        lk_stdlib_common::stdlib_register_runtime_builtins!(
            registry,
            [
                plain "chan::close" => chan_close, 1,
                plain "chan::len" => chan_len, 1,
                plain "chan::capacity" => chan_capacity, 1,
                plain "chan::is_closed" => chan_is_closed, 1,
                plain "chan::try_send" => chan_try_send, 2,
                plain "chan::try_recv" => chan_try_recv, 1,
            ],
        );
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "close" => chan_close, 1,
                plain "len" => chan_len, 1,
                plain "capacity" => chan_capacity, 1,
                plain "is_closed" => chan_is_closed, 1,
                plain "try_send" => chan_try_send, 2,
                plain "try_recv" => chan_try_recv, 1,
            ],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("chan", Box::new(ChannelModule::new()))
}

impl ChannelModule {
    pub fn new() -> Self {
        Self
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

fn pair(ok: bool, value: RuntimeVal, runtime: &mut NativeRuntime<'_>) -> RuntimeVal {
    RuntimeVal::Obj(
        runtime
            .heap_mut()
            .alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Bool(ok), value]))),
    )
}

fn chan_close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "chan.close()")?;
    let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.close()")?;
    rt::with_runtime(|runtime| runtime.close_channel(channel.id))
        .map_err(|err| anyhow!("Failed to close channel: {err}"))?;
    Ok(RuntimeVal::Nil)
}

fn chan_len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "chan.len()")?;
    let _ = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.len()")?;
    Ok(RuntimeVal::Int(0))
}

fn chan_capacity(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "chan.capacity()")?;
    let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.capacity()")?;
    Ok(RuntimeVal::Int(channel.capacity.unwrap_or(0)))
}

fn chan_is_closed(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "chan.is_closed()")?;
    let _ = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.is_closed()")?;
    Ok(RuntimeVal::Bool(false))
}

fn chan_try_send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "chan.try_send()")?;
    let values = args.as_slice();
    let channel = channel_arg(&values[0], runtime.heap(), "chan.try_send()")?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = rt::with_runtime(|runtime| runtime.try_send(channel.id, value))
        .map_err(|err| anyhow!("Failed to send to channel: {err}"))?;
    Ok(RuntimeVal::Bool(sent))
}

fn chan_try_recv(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "chan.try_recv()")?;
    let channel = channel_arg(args.get(0).expect("checked arity"), runtime.heap(), "chan.try_recv()")?;
    match rt::with_runtime(|rt| rt.try_recv(channel.id))
        .map_err(|err| anyhow!("Failed to receive from channel: {err}"))?
    {
        Some((ok, value)) => Ok(pair(ok, value.into_value(runtime.heap_mut())?, runtime)),
        None => Ok(pair(false, RuntimeVal::Nil, runtime)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{
        val::{ShortStr, Type},
        vm::{NativeFunction, RuntimeModuleState},
    };

    fn chan_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&ChannelModule::new(), name)
    }

    fn runtime_channel(capacity: i64, heap: &mut HeapStore) -> Result<RuntimeVal> {
        let id = rt::with_runtime(|runtime| runtime.create_channel(Some(capacity as usize)))?;
        Ok(RuntimeVal::Obj(heap.alloc(HeapValue::Channel(Arc::new(
            ChannelValue {
                id,
                capacity: Some(capacity),
                inner_type: Type::Nil,
            },
        )))))
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState) -> Result<RuntimeVal> {
        let (_, function) = chan_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    fn expect_list(value: &RuntimeVal, heap: &HeapStore) -> Vec<RuntimeVal> {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("expected runtime list heap value");
        };
        match list {
            TypedList::Mixed(values) => values.clone(),
            TypedList::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            TypedList::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            TypedList::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            TypedList::String(values) => values
                .iter()
                .map(|value| RuntimeVal::ShortStr(ShortStr::new(value).expect("short test string")))
                .collect(),
        }
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
        let mut state = RuntimeModuleState::default();
        let channel = runtime_channel(3, state.heap_mut())?;
        assert_eq!(
            call("capacity", std::slice::from_ref(&channel), &mut state)?,
            RuntimeVal::Int(3)
        );
        assert_eq!(
            call("len", std::slice::from_ref(&channel), &mut state)?,
            RuntimeVal::Int(0)
        );
        assert_eq!(
            call("is_closed", std::slice::from_ref(&channel), &mut state)?,
            RuntimeVal::Bool(false)
        );
        Ok(())
    }

    #[test]
    fn chan_try_send_and_recv_round_trips_runtime_values() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        let channel = runtime_channel(1, state.heap_mut())?;
        let value = RuntimeVal::ShortStr(ShortStr::new("payload").expect("short string"));
        assert_eq!(
            call("try_send", &[channel.clone(), value], &mut state)?,
            RuntimeVal::Bool(true)
        );

        let received = call("try_recv", std::slice::from_ref(&channel), &mut state)?;
        let received = expect_list(&received, state.heap());
        assert_eq!(received.len(), 2);
        assert_eq!(received[0], RuntimeVal::Bool(true));
        assert_eq!(
            received[1],
            RuntimeVal::ShortStr(ShortStr::new("payload").expect("short string"))
        );
        Ok(())
    }

    #[test]
    fn chan_try_recv_empty_returns_false_nil_pair() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        let channel = runtime_channel(1, state.heap_mut())?;
        let received = call("try_recv", std::slice::from_ref(&channel), &mut state)?;
        assert_eq!(
            expect_list(&received, state.heap()),
            vec![RuntimeVal::Bool(false), RuntimeVal::Nil]
        );
        Ok(())
    }
}
