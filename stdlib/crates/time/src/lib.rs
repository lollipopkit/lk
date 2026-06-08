//! Time module for LK concurrency.
//!
//! Module-level functions use RuntimeNative.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{self, ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    rt::{RuntimePayload, with_runtime},
    val::{ChannelValue, HeapValue, RuntimeVal, Type},
    vm::{NativeArgs, NativeFunction, NativeRuntime, RuntimeExport},
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[derive(Debug)]
pub struct TimeModule;

impl Default for TimeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for TimeModule {
    fn name(&self) -> &str {
        "time"
    }

    fn description(&self) -> &str {
        "Timing and scheduling functions for concurrent operations"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn register(&self, registry: &mut module::ModuleRegistry) -> Result<()> {
        registry.register_runtime_builtin("time::sleep", NativeFunction::Plain(time_sleep), 1);
        registry.register_runtime_builtin("time::timeout", NativeFunction::Plain(time_timeout), 1);
        registry.register_runtime_builtin("time::after", NativeFunction::Plain(time_after), 1);
        registry.register_runtime_builtin("time::now", NativeFunction::Plain(time_now), 0);
        registry.register_runtime_builtin("time::since", NativeFunction::Plain(time_since), 2);
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("sleep", time_sleep, 1),
                RuntimeNativeExport::plain("timeout", time_timeout, 1),
                RuntimeNativeExport::plain("after", time_after, 1),
                RuntimeNativeExport::plain("now", time_now, 0),
                RuntimeNativeExport::plain("since", time_since, 2),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("time", Box::new(TimeModule::new()))
}

impl TimeModule {
    pub fn new() -> Self {
        Self
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }
    bail!(
        "{name} expects exactly {expected} argument{}",
        if expected == 1 { "" } else { "s" }
    )
}

fn numeric_millis(value: &RuntimeVal, name: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(ms) => Ok(*ms),
        RuntimeVal::Float(ms) => Ok(*ms as i64),
        other => Err(anyhow!("{name} expects a numeric argument, got {:?}", other.kind())),
    }
}

fn epoch_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

fn runtime_channel(id: u64, capacity: i64, inner_type: Type, runtime: &mut NativeRuntime<'_>) -> RuntimeVal {
    RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Channel(Arc::new(ChannelValue {
        id,
        capacity: Some(capacity),
        inner_type,
    }))))
}

fn time_sleep(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "time.sleep()")?;
    let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "time.sleep()")?;
    with_runtime(|runtime| {
        let duration = Duration::from_millis(duration_ms as u64);
        runtime.block_on(async {
            tokio::time::sleep(duration).await;
            Ok(RuntimeVal::Nil)
        })
    })
    .map_err(|err| anyhow!("Failed to sleep: {err}"))
}

fn time_timeout(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "time.timeout()")?;
    let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "time.timeout()")?;
    let channel_id = spawn_timer(duration_ms, RuntimeVal::Nil)?;
    Ok(runtime_channel(channel_id, 1, Type::Nil, runtime))
}

fn time_after(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "time.after()")?;
    let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "time.after()")?;
    let channel_id = spawn_timer(duration_ms, RuntimeVal::Int(epoch_millis()))?;
    Ok(runtime_channel(channel_id, 1, Type::Int, runtime))
}

fn time_now(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "time.now()")?;
    Ok(RuntimeVal::Int(epoch_millis()))
}

fn time_since(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "time.since()")?;
    let values = args.as_slice();
    let start = numeric_millis(&values[0], "time.since()")?;
    let end = numeric_millis(&values[1], "time.since()")?;
    Ok(RuntimeVal::Int(end - start))
}

fn spawn_timer(duration_ms: i64, payload: RuntimeVal) -> Result<u64> {
    with_runtime(|runtime| {
        let channel_id = runtime.create_channel(Some(1))?;
        let future = async move {
            tokio::time::sleep(Duration::from_millis(duration_ms as u64)).await;
            let value = match payload {
                RuntimeVal::Nil => RuntimePayload::nil(),
                RuntimeVal::Int(_) => {
                    RuntimePayload::new(RuntimeVal::Int(epoch_millis()), lk_core::val::HeapStore::new())
                }
                other => return Err(anyhow!("unsupported timer payload {:?}", other.kind())),
            };
            with_runtime(|runtime| runtime.try_send(channel_id, value))
                .map_err(|err| anyhow!("Failed to send timer signal: {err}"))?;
            Ok(RuntimePayload::nil())
        };
        runtime.spawn(future)?;
        Ok(channel_id)
    })
    .map_err(|err| anyhow!("Failed to create timer: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::vm::{NativeFunction, RuntimeModuleState};

    fn time_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&TimeModule::new(), name)
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState) -> Result<RuntimeVal> {
        let (_, function) = time_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    #[test]
    fn time_exports_use_runtime_native() -> Result<()> {
        for name in ["sleep", "timeout", "after", "now", "since"] {
            let (arity, function) = time_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn time_now_and_since_return_runtime_ints() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        assert!(matches!(call("now", &[], &mut state)?, RuntimeVal::Int(value) if value > 0));
        assert_eq!(
            call("since", &[RuntimeVal::Int(100), RuntimeVal::Float(175.0)], &mut state)?,
            RuntimeVal::Int(75)
        );
        Ok(())
    }

    #[test]
    fn time_timeout_and_after_return_channels() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        for (name, expected_type) in [("timeout", Type::Nil), ("after", Type::Int)] {
            let value = call(name, &[RuntimeVal::Int(0)], &mut state)?;
            let RuntimeVal::Obj(handle) = value else {
                panic!("{name} should return heap channel");
            };
            let HeapValue::Channel(channel) = state.heap().get(handle).expect("channel object") else {
                panic!("{name} should return Channel");
            };
            assert_eq!(channel.capacity, Some(1));
            assert_eq!(channel.inner_type, expected_type);
        }
        Ok(())
    }

    #[test]
    fn time_sleep_accepts_zero_duration() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        assert_eq!(call("sleep", &[RuntimeVal::Int(0)], &mut state)?, RuntimeVal::Nil);
        Ok(())
    }
}
