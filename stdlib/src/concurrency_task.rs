//! Task module for LK concurrency.
//!
//! Module exports use RuntimeNative32. Global concurrency helpers in
//! `stdlib::register_stdlib_concurrency_globals` are migrated separately.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{self, Module, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    rt,
    val::{HeapStore, HeapValue, RuntimeVal, TaskValue},
    vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeExport32},
};
use std::sync::Arc;

#[derive(Debug)]
pub struct TaskModule;

impl Default for TaskModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for TaskModule {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Task management functions for concurrent operations"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn register(&self, registry: &mut module::ModuleRegistry) -> Result<()> {
        registry.register_runtime_builtin("task::await", NativeFunction32::Plain(task_await32), 1);
        registry.register_runtime_builtin("task::try_await", NativeFunction32::Plain(task_try_await32), 1);
        registry.register_runtime_builtin(
            "task::join_all",
            NativeFunction32::Plain(task_join_all32),
            NativeEntry32::VARIADIC,
        );
        registry.register_runtime_builtin("task::sleep", NativeFunction32::Plain(task_sleep32), 1);
        registry.register_runtime_builtin(
            "task::spawn_blocking",
            NativeFunction32::Plain(task_spawn_blocking32),
            1,
        );
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("await", task_await32, 1),
                RuntimeNativeExport32::plain("try_await", task_try_await32, 1),
                RuntimeNativeExport32::plain("join_all", task_join_all32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("sleep", task_sleep32, 1),
                RuntimeNativeExport32::plain("spawn_blocking", task_spawn_blocking32, 1),
            ],
            &[],
        ))
    }
}

impl TaskModule {
    pub fn new() -> Self {
        Self
    }
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

fn task_arg(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<Arc<TaskValue>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{name} expects a Task argument");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::Task(task) => Ok(task.clone()),
        other => Err(anyhow!("{name} expects a Task argument, got {}", other.type_name())),
    }
}

fn numeric_millis(value: &RuntimeVal, name: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        RuntimeVal::Float(value) => Ok(*value as i64),
        other => Err(anyhow!("{name} expects a numeric argument, got {:?}", other.kind())),
    }
}

fn is_callable(value: &RuntimeVal, heap: &HeapStore) -> Result<bool> {
    let RuntimeVal::Obj(handle) = value else {
        return Ok(false);
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    Ok(matches!(value, HeapValue::Callable(_)))
}

fn task_await32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "task.await()")?;
    let task = task_arg(args.get(0).expect("checked arity"), runtime.heap(), "task.await()")?;
    let value = rt::with_runtime(|rt| rt.block_on(rt.join_task(task.id)))
        .map_err(|err| anyhow!("Failed to await task: {err}"))?;
    value.into_value(runtime.heap_mut())
}

fn task_try_await32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "task.try_await()")?;
    let task = task_arg(args.get(0).expect("checked arity"), runtime.heap(), "task.try_await()")?;
    let value = match &task.value {
        Some(value) => value.clone_value_into(runtime.heap_mut())?,
        None => RuntimeVal::Nil,
    };
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(
        lk_core::val::TypedList::Mixed(vec![RuntimeVal::Bool(task.value.is_some()), value]),
    ))))
}

fn task_join_all32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args.as_slice() {
        let task = task_arg(arg, runtime.heap(), "task.join_all()")?;
        let value = rt::with_runtime(|rt| rt.block_on(rt.join_task(task.id)))
            .map_err(|err| anyhow!("Failed to await task: {err}"))?;
        values.push(value.into_value(runtime.heap_mut())?);
    }
    let list = lk_core::val::TypedList::from_runtime_values(values, runtime.heap());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn task_sleep32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "task.sleep()")?;
    let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "task.sleep()")?;
    rt::with_runtime(|rt| {
        let duration = std::time::Duration::from_millis(duration_ms as u64);
        rt.block_on(async {
            tokio::time::sleep(duration).await;
            Ok(RuntimeVal::Nil)
        })
    })
    .map_err(|err| anyhow!("Failed to sleep: {err}"))
}

fn task_spawn_blocking32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "task.spawn_blocking()")?;
    if !is_callable(args.get(0).expect("checked arity"), runtime.heap())? {
        bail!("task.spawn_blocking() expects a function argument");
    }
    let task_id = rt::with_runtime(|rt| {
        let future = async move { Err(anyhow!("task.spawn_blocking() needs VmContext lifetime management")) };
        rt.spawn(future)
    })
    .map_err(|err| anyhow!("Failed to spawn blocking task: {err}"))?;
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Task(Arc::new(
        TaskValue {
            id: task_id,
            value: None,
        },
    )))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{rt::RuntimePayload, vm::RuntimeModuleState32};

    fn task_native(name: &str) -> Result<(u16, NativeFunction32)> {
        crate::runtime_native::runtime_native_export(&TaskModule::new(), name)
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState32) -> Result<RuntimeVal> {
        let (_, function) = task_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative32");
        };
        let mut runtime = NativeRuntime32::new(state, None, None);
        function(NativeArgs32::new(args), &mut runtime)
    }

    fn resolved_task(value: RuntimeVal, heap: &mut HeapStore) -> RuntimeVal {
        let payload = RuntimePayload::new(value, HeapStore::new());
        RuntimeVal::Obj(heap.alloc(HeapValue::Task(Arc::new(TaskValue {
            id: 0,
            value: Some(payload),
        }))))
    }

    fn expect_list(value: &RuntimeVal, heap: &HeapStore) -> Vec<RuntimeVal> {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("expected runtime list heap value");
        };
        match list {
            lk_core::val::TypedList::Mixed(values) => values.clone(),
            lk_core::val::TypedList::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            lk_core::val::TypedList::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            lk_core::val::TypedList::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            lk_core::val::TypedList::String(values) => values
                .iter()
                .map(|value| RuntimeVal::ShortStr(lk_core::val::ShortStr::new(value).expect("short test string")))
                .collect(),
            lk_core::val::TypedList::OwnedRuntime(values) => values.values.clone(),
        }
    }

    #[test]
    fn task_exports_use_runtime_native32() -> Result<()> {
        for name in ["await", "try_await", "join_all", "sleep", "spawn_blocking"] {
            let (_, function) = task_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
        assert_eq!(task_native("join_all")?.0, lk_core::vm::NativeEntry32::VARIADIC);
        Ok(())
    }

    #[test]
    fn task_try_await_uses_runtime_task_value() -> Result<()> {
        let mut state = RuntimeModuleState32::default();
        let task = resolved_task(RuntimeVal::Int(42), &mut state.heap);
        let result = call("try_await", &[task], &mut state)?;
        assert_eq!(
            expect_list(&result, &state.heap),
            vec![RuntimeVal::Bool(true), RuntimeVal::Int(42)]
        );
        Ok(())
    }

    #[test]
    fn task_join_all_empty_returns_empty_list() -> Result<()> {
        let mut state = RuntimeModuleState32::default();
        let result = call("join_all", &[], &mut state)?;
        assert_eq!(expect_list(&result, &state.heap), Vec::<RuntimeVal>::new());
        Ok(())
    }

    #[test]
    fn task_sleep_accepts_zero_duration() -> Result<()> {
        let mut state = RuntimeModuleState32::default();
        assert_eq!(call("sleep", &[RuntimeVal::Int(0)], &mut state)?, RuntimeVal::Nil);
        Ok(())
    }

    #[test]
    fn task_spawn_blocking_rejects_non_callable() {
        let mut state = RuntimeModuleState32::default();
        let err = call("spawn_blocking", &[RuntimeVal::Int(1)], &mut state).expect_err("non-callable should fail");
        assert!(err.to_string().contains("expects a function"));
    }

    #[test]
    fn task_try_await_pending_returns_false_nil() -> Result<()> {
        let mut state = RuntimeModuleState32::default();
        let task = RuntimeVal::Obj(state.heap.alloc(HeapValue::Task(Arc::new(TaskValue {
            id: 999_999,
            value: None,
        }))));
        let result = call("try_await", &[task], &mut state)?;
        assert_eq!(
            expect_list(&result, &state.heap),
            vec![RuntimeVal::Bool(false), RuntimeVal::Nil]
        );
        Ok(())
    }
}
