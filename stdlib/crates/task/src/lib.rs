// Task module for LK concurrency.
//
// Module exports use RuntimeNative. Global concurrency helpers in
// `stdlib::register_stdlib_concurrency_globals` are migrated separately.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeVal, TaskValue},
    vm::{NativeArgs, NativeRuntime},
};
use std::sync::Arc;

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}
pub use lk_stdlib_common::typed_list_from_values;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "task", docs = "Task management functions for concurrent operations")]
pub struct TaskModule;

#[lk_stdlib_common::stdlib_exports(module = "task", runtime_builtins = true)]
impl TaskModule {
    #[stdlib_export(name = "await", params(task: Task), returns = Any)]
    fn task_await(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let task = task_arg(args.get(0).expect("checked arity"), runtime.heap(), "task.await()")?;
        let value = runtime
            .async_runtime()
            .with(|rt| rt.block_on(rt.join_task(task.id)))
            .map_err(|err| anyhow!("Failed to await task: {err}"))?;
        value.into_value(runtime.heap_mut())
    }

    /// Non-blocking await: the task's value once resolved, `nil` while still
    /// running (pairs with postfix `!` to assert completion).
    #[stdlib_export(name = "try_await", params(task: Task), returns = Any)]
    fn try_await(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let task = task_arg(args.get(0).expect("checked arity"), runtime.heap(), "task.try_await()")?;
        match &task.value {
            Some(value) => value.clone_value_into(runtime.heap_mut()),
            None => Ok(RuntimeVal::Nil),
        }
    }

    #[stdlib_export(name = "join_all", params(...tasks: Task), returns = List)]
    fn join_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args.as_slice() {
            let task = task_arg(arg, runtime.heap(), "task.join_all()")?;
            let value = runtime
                .async_runtime()
                .with(|rt| rt.block_on(rt.join_task(task.id)))
                .map_err(|err| anyhow!("Failed to await task: {err}"))?;
            values.push(value.into_value(runtime.heap_mut())?);
        }
        let list = crate::typed_list_from_values(values, runtime.heap());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
    }

    /// Concurrency observability (goroutine-leak diagnosis): counts of
    /// not-yet-awaited tasks and live channels on this VM's runtime. A
    /// steadily growing `active_tasks` means goroutines are being spawned
    /// and never awaited/finished.
    #[stdlib_export(name = "stats", params(), returns = Map)]
    fn stats(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let stats = runtime
            .async_runtime()
            .with(|rt| Ok(rt.stats()))
            .map_err(|err| anyhow!("Failed to read runtime stats: {err}"))?;
        let mut map = lk_core::util::fast_map::fast_hash_map_new();
        map.insert(
            Arc::<str>::from("active_tasks"),
            RuntimeVal::Int(stats.active_tasks as i64),
        );
        map.insert(
            Arc::<str>::from("active_channels"),
            RuntimeVal::Int(stats.active_channels as i64),
        );
        map.insert(
            Arc::<str>::from("multi_threaded"),
            RuntimeVal::Bool(stats.is_multi_threaded),
        );
        Ok(RuntimeVal::Obj(
            runtime
                .heap_mut()
                .alloc(HeapValue::Map(lk_core::val::TypedMap::StringMixed(map))),
        ))
    }

    #[stdlib_export(name = "sleep", params(ms: Int | Float), returns = Nil)]
    fn sleep(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let duration_ms = numeric_millis(args.get(0).expect("checked arity"), "task.sleep()")?;
        if duration_ms < 0 {
            bail!("task.sleep() duration must be non-negative");
        }
        runtime
            .async_runtime()
            .with(|rt| {
                let duration = std::time::Duration::from_millis(duration_ms as u64);
                rt.block_on(async {
                    tokio::time::sleep(duration).await;
                    Ok(RuntimeVal::Nil)
                })
            })
            .map_err(|err| anyhow!("Failed to sleep: {err}"))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{
        rt::RuntimePayload,
        vm::{NativeFunction, RuntimeModuleState},
    };

    fn task_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&TaskModule::new(), name)
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState) -> Result<RuntimeVal> {
        let (_, function) = task_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, None, None);
        function(NativeArgs::new(args), &mut runtime)
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
        }
    }

    #[test]
    fn task_exports_use_runtime_native() -> Result<()> {
        for name in ["await", "try_await", "join_all", "sleep"] {
            let (_, function) = task_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        assert_eq!(task_native("join_all")?.0, lk_core::vm::NativeEntry::VARIADIC);
        Ok(())
    }

    #[test]
    fn task_try_await_uses_runtime_task_value() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        let task = resolved_task(RuntimeVal::Int(42), state.heap_mut());
        let result = call("try_await", &[task], &mut state)?;
        assert_eq!(result, RuntimeVal::Int(42));
        Ok(())
    }

    #[test]
    fn task_join_all_empty_returns_empty_list() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        let result = call("join_all", &[], &mut state)?;
        assert_eq!(expect_list(&result, state.heap()), Vec::<RuntimeVal>::new());
        Ok(())
    }

    #[test]
    fn task_sleep_accepts_zero_duration() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        assert_eq!(call("sleep", &[RuntimeVal::Int(0)], &mut state)?, RuntimeVal::Nil);
        Ok(())
    }

    #[test]
    fn task_try_await_pending_returns_nil() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        let task = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Task(Arc::new(TaskValue {
            id: 999_999,
            value: None,
        }))));
        let result = call("try_await", &[task], &mut state)?;
        assert_eq!(result, RuntimeVal::Nil);
        Ok(())
    }
}
