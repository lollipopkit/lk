//! Time module for LK concurrency.
//!
//! Module-level functions use RuntimeNative32 while the older global
//! concurrency builtins are still migrated separately.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{self, Module},
    rt::with_runtime,
    val::{ChannelValue, HeapValue, RuntimeVal, Type, Val},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub struct TimeModule;

impl Default for TimeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for TimeModule {
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
        for (name, value) in self.exports() {
            registry.register_builtin(&format!("{}::{}", self.name(), name), value);
        }
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        let mut functions = HashMap::new();
        register_native(&mut functions, "sleep", time_sleep32, 1);
        register_native(&mut functions, "timeout", time_timeout32, 1);
        register_native(&mut functions, "after", time_after32, 1);
        register_native(&mut functions, "now", time_now32, 0);
        register_native(&mut functions, "since", time_since32, 2);
        functions
    }
}

impl TimeModule {
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

fn runtime_channel(id: u64, capacity: i64, inner_type: Type, runtime: &mut NativeRuntime32<'_>) -> RuntimeVal {
    RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Channel(Arc::new(ChannelValue {
        id,
        capacity: Some(capacity),
        inner_type,
    }))))
}

fn time_sleep32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

fn time_timeout32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "time.timeout()")?;
    let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "time.timeout()")?;
    let channel_id = spawn_timer(duration_ms, RuntimeVal::Nil)?;
    Ok(runtime_channel(channel_id, 1, Type::Nil, runtime))
}

fn time_after32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "time.after()")?;
    let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "time.after()")?;
    let channel_id = spawn_timer(duration_ms, RuntimeVal::Int(epoch_millis()))?;
    Ok(runtime_channel(channel_id, 1, Type::Int, runtime))
}

fn time_now32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "time.now()")?;
    Ok(RuntimeVal::Int(epoch_millis()))
}

fn time_since32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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
                RuntimeVal::Nil => Val::Nil,
                RuntimeVal::Int(_) => Val::Int(epoch_millis()),
                other => return Err(anyhow!("unsupported timer payload {:?}", other.kind())),
            };
            with_runtime(|runtime| runtime.try_send(channel_id, value))
                .map_err(|err| anyhow!("Failed to send timer signal: {err}"))?;
            Ok(Val::Nil)
        };
        runtime.spawn(future)?;
        Ok(channel_id)
    })
    .map_err(|err| anyhow!("Failed to create timer: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{
        module::Module,
        val::{CallableValue, HeapStore},
        vm::{NativeFunction32, RuntimeModuleState32},
    };

    fn time_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = TimeModule::new().exports();
        let value = exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?;
        let Val::Obj(object) = value else {
            bail!("{name} must be a heap callable");
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            bail!("{name} must be RuntimeNative32");
        };
        Ok((*arity, function.clone()))
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState32) -> Result<RuntimeVal> {
        let (_, function) = time_native(name)?;
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

    #[test]
    fn time_exports_use_runtime_native32() -> Result<()> {
        for name in ["sleep", "timeout", "after", "now", "since"] {
            let (arity, function) = time_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry32::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn time_now_and_since_return_runtime_ints() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        assert!(matches!(call("now", &[], &mut state)?, RuntimeVal::Int(value) if value > 0));
        assert_eq!(
            call("since", &[RuntimeVal::Int(100), RuntimeVal::Float(175.0)], &mut state)?,
            RuntimeVal::Int(75)
        );
        Ok(())
    }

    #[test]
    fn time_timeout_and_after_return_channels() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        for (name, expected_type) in [("timeout", Type::Nil), ("after", Type::Int)] {
            let value = call(name, &[RuntimeVal::Int(0)], &mut state)?;
            let RuntimeVal::Obj(handle) = value else {
                panic!("{name} should return heap channel");
            };
            let HeapValue::Channel(channel) = state.heap.get(handle).expect("channel object") else {
                panic!("{name} should return Channel");
            };
            assert_eq!(channel.capacity, Some(1));
            assert_eq!(channel.inner_type, expected_type);
        }
        Ok(())
    }

    #[test]
    fn time_sleep_accepts_zero_duration() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        assert_eq!(call("sleep", &[RuntimeVal::Int(0)], &mut state)?, RuntimeVal::Nil);
        Ok(())
    }
}
