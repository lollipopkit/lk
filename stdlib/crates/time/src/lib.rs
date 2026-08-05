//! Time module for LK concurrency.
//!
//! Module-level functions use RuntimeNative.

use anyhow::{Result, anyhow};
use lk_core::{
    rt::{AsyncRuntimeHandle, RuntimePayload},
    val::{ChannelValue, HeapValue, RuntimeVal, Type},
    vm::{NativeArgs, NativeRuntime},
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "time", docs = "Timing and scheduling functions for concurrent operations")]
pub struct TimeModule;

#[lk_stdlib_common::stdlib_exports(module = "time", runtime_builtins = true)]
impl TimeModule {
    #[stdlib_export(name = "sleep", params(ms: Int | Float), returns = Nil)]
    fn sleep(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let duration_ms = lk_stdlib_common::duration_millis(args.get(0).expect("checked arity"), "time.sleep()")?;
        runtime
            .async_runtime()
            .with(|runtime| {
                let duration = Duration::from_millis(duration_ms as u64);
                runtime.block_on(async {
                    tokio::time::sleep(duration).await;
                    Ok(RuntimeVal::Nil)
                })
            })
            .map_err(|err| anyhow!("Failed to sleep: {err}"))
    }

    #[stdlib_export(name = "timeout", params(ms: Int | Float), returns = Channel)]
    fn timeout(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let duration_ms = lk_stdlib_common::duration_millis(args.get(0).expect("checked arity"), "time.timeout()")?;
        let channel_id = spawn_timer(&runtime.async_runtime(), duration_ms, RuntimeVal::Nil)?;
        Ok(runtime_channel(channel_id, 1, Type::Nil, runtime))
    }

    #[stdlib_export(name = "after", params(ms: Int | Float), returns = Channel)]
    fn after(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let duration_ms = lk_stdlib_common::duration_millis(args.get(0).expect("checked arity"), "time.after()")?;
        let channel_id = spawn_timer(&runtime.async_runtime(), duration_ms, RuntimeVal::Int(epoch_millis()))?;
        Ok(runtime_channel(channel_id, 1, Type::Int, runtime))
    }

    #[stdlib_export(name = "now", params(), returns = Int)]
    fn now(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Int(epoch_millis()))
    }

    #[stdlib_export(name = "since", params(start_ms: Int | Float, end_ms: Int | Float), named(end_ms), returns = Int)]
    fn since(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let start = numeric_millis(&values[0], "time.since()")?;
        let end = numeric_millis(&values[1], "time.since()")?;
        Ok(RuntimeVal::Int(end - start))
    }
}

/// An *instant* in milliseconds, of either sign.
///
/// Distinct from `lk_stdlib_common::duration_millis`, which is a *duration* and
/// must be non-negative: `time.since(start, end)` takes two points on a clock,
/// and their difference is the thing with a direction.
fn numeric_millis(value: &RuntimeVal, name: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(ms) => Ok(*ms),
        RuntimeVal::Float(ms) => Ok(*ms as i64),
        other => Err(anyhow!("{name} expects a numeric argument, got {:?}", other.kind())),
    }
}

fn epoch_millis() -> i64 {
    // A clock set before 1970 answers `Err`, and unwrapping it aborted the
    // process — every `time.*` call, on a machine whose clock is merely wrong.
    // Zero is the epoch, which is what a pre-epoch clock is closest to.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn runtime_channel(id: u64, capacity: i64, inner_type: Type, runtime: &mut NativeRuntime<'_>) -> RuntimeVal {
    RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Channel(Arc::new(ChannelValue {
        id,
        capacity: Some(capacity),
        inner_type,
    }))))
}

fn spawn_timer(handle: &AsyncRuntimeHandle, duration_ms: i64, payload: RuntimeVal) -> Result<u64> {
    let send_handle = handle.clone();
    handle
        .with(|runtime| {
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
                send_handle
                    .with(|runtime| runtime.try_send(channel_id, value))
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
            anyhow::bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    /// Registered arity follows the declaration: a member with `named(...)`
    /// registers variadic, because a named argument occupies no positional
    /// slot and the generated precheck — which knows the names — checks the
    /// bounds instead.
    #[test]
    fn time_exports_use_runtime_native() -> Result<()> {
        for name in ["sleep", "timeout", "after", "now", "since"] {
            let (arity, function) = time_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            let path = format!("time.{name}");
            let nameable = TimeModule::stdlib_metadata()
                .signatures
                .iter()
                .find(|signature| signature.path == path)
                .is_some_and(|signature| signature.params.iter().any(|param| param.named));
            if nameable {
                assert_eq!(arity, lk_core::vm::NativeEntry::VARIADIC, "{name} declares named(...)");
            } else {
                assert_ne!(arity, lk_core::vm::NativeEntry::VARIADIC, "{name}");
            }
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
