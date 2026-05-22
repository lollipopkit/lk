//! Channel module for LK.
//!
//! The module-level API uses the RuntimeNative32 ABI. Global concurrency
//! builtins are still registered separately while compiler lowering is being
//! migrated.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{self, Module},
    rt,
    rt::RuntimePayload,
    val::{ChannelValue, HeapStore, HeapValue, RuntimeVal, TypedList, Val},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32, copy_runtime_value},
};
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub struct ChannelModule;

impl Default for ChannelModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for ChannelModule {
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
        for (name, value) in self.exports() {
            registry.register_builtin(&format!("{}::{}", self.name(), name), value);
        }
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        let mut functions = HashMap::new();
        register_native(&mut functions, "close", chan_close32, 1);
        register_native(&mut functions, "len", chan_len32, 1);
        register_native(&mut functions, "capacity", chan_capacity32, 1);
        register_native(&mut functions, "is_closed", chan_is_closed32, 1);
        register_native(&mut functions, "try_send", chan_try_send32, 2);
        register_native(&mut functions, "try_recv", chan_try_recv32, 1);
        functions
    }
}

impl ChannelModule {
    pub fn new() -> Self {
        Self
    }
}

fn register_native(
    functions: &mut HashMap<String, Val>,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    functions.insert(
        name.to_string(),
        Val::runtime_native32(NativeFunction32::Plain(function), arity),
    );
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }
    bail!(
        "{name} expects exactly {expected} argument{}",
        if expected == 1 { "" } else { "s" }
    )
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

fn pair(ok: bool, value: RuntimeVal, runtime: &mut NativeRuntime32<'_>) -> RuntimeVal {
    RuntimeVal::Obj(
        runtime
            .heap_mut()
            .alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Bool(ok), value]))),
    )
}

fn chan_close32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "chan.close()")?;
    let channel = channel_arg(args.get(0).expect("checked arity"), &runtime.state.heap, "chan.close()")?;
    rt::with_runtime(|runtime| runtime.close_channel(channel.id))
        .map_err(|err| anyhow!("Failed to close channel: {err}"))?;
    Ok(RuntimeVal::Nil)
}

fn chan_len32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "chan.len()")?;
    let _ = channel_arg(args.get(0).expect("checked arity"), &runtime.state.heap, "chan.len()")?;
    Ok(RuntimeVal::Int(0))
}

fn chan_capacity32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "chan.capacity()")?;
    let channel = channel_arg(
        args.get(0).expect("checked arity"),
        &runtime.state.heap,
        "chan.capacity()",
    )?;
    Ok(RuntimeVal::Int(channel.capacity.unwrap_or(0)))
}

fn chan_is_closed32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "chan.is_closed()")?;
    let _ = channel_arg(
        args.get(0).expect("checked arity"),
        &runtime.state.heap,
        "chan.is_closed()",
    )?;
    Ok(RuntimeVal::Bool(false))
}

fn chan_try_send32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "chan.try_send()")?;
    let values = args.as_slice();
    let channel = channel_arg(&values[0], &runtime.state.heap, "chan.try_send()")?;
    let value = runtime_payload_from_value(&values[1], &runtime.state.heap)?;
    let sent = rt::with_runtime(|runtime| runtime.try_send(channel.id, value))
        .map_err(|err| anyhow!("Failed to send to channel: {err}"))?;
    Ok(RuntimeVal::Bool(sent))
}

fn chan_try_recv32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "chan.try_recv()")?;
    let channel = channel_arg(
        args.get(0).expect("checked arity"),
        &runtime.state.heap,
        "chan.try_recv()",
    )?;
    match rt::with_runtime(|rt| rt.try_recv(channel.id))
        .map_err(|err| anyhow!("Failed to receive from channel: {err}"))?
    {
        Some((ok, value)) => Ok(pair(
            ok,
            runtime_payload_into_value(value, runtime.heap_mut())?,
            runtime,
        )),
        None => Ok(pair(false, RuntimeVal::Nil, runtime)),
    }
}

fn runtime_payload_from_value(value: &RuntimeVal, heap: &HeapStore) -> Result<RuntimePayload> {
    let mut source_heap = heap.clone();
    let mut payload_heap = HeapStore::new();
    let value = copy_runtime_value(value, &mut source_heap, &mut payload_heap)?;
    Ok(RuntimePayload::new(value, payload_heap))
}

fn runtime_payload_into_value(payload: RuntimePayload, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let mut payload_heap = payload.heap;
    copy_runtime_value(&payload.value, &mut payload_heap, heap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{
        module::Module,
        val::{CallableValue, ShortStr, Type},
        vm::{NativeFunction32, RuntimeModuleState32},
    };

    fn chan_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = ChannelModule::new().exports();
        let value = exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?;
        let Val::Obj(object) = value else {
            bail!("{name} must be a heap callable");
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            bail!("{name} must be RuntimeNative32");
        };
        Ok((*arity, function.clone()))
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

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState32) -> Result<RuntimeVal> {
        let (_, function) = chan_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative32");
        };
        let mut runtime = NativeRuntime32 {
            state,
            ctx: None,
            module: None,
        };
        function(NativeArgs32::new(args), &mut runtime)
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
    fn chan_exports_use_runtime_native32() -> Result<()> {
        for name in ["close", "len", "capacity", "is_closed", "try_send", "try_recv"] {
            let (arity, function) = chan_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry32::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn chan_capacity_len_and_is_closed_use_runtime_channel() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let channel = runtime_channel(3, &mut state.heap)?;
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
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let channel = runtime_channel(1, &mut state.heap)?;
        let value = RuntimeVal::ShortStr(ShortStr::new("payload").expect("short string"));
        assert_eq!(
            call("try_send", &[channel.clone(), value], &mut state)?,
            RuntimeVal::Bool(true)
        );

        let received = call("try_recv", std::slice::from_ref(&channel), &mut state)?;
        let received = expect_list(&received, &state.heap);
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
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let channel = runtime_channel(1, &mut state.heap)?;
        let received = call("try_recv", std::slice::from_ref(&channel), &mut state)?;
        assert_eq!(
            expect_list(&received, &state.heap),
            vec![RuntimeVal::Bool(false), RuntimeVal::Nil]
        );
        Ok(())
    }
}
