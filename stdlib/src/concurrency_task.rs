//! Task module for LK concurrency.
//!
//! Module exports use RuntimeNative32. Global concurrency helpers in
//! `stdlib::register_stdlib_concurrency_globals` are migrated separately.

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{self, Module},
    rt,
    rt::RuntimePayload,
    val::{HeapStore, HeapValue, RuntimeVal, TaskValue, Val},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32, copy_runtime_value},
};
use std::{collections::HashMap, sync::Arc};

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
        for (name, value) in self.exports() {
            registry.register_builtin(&format!("{}::{}", self.name(), name), value);
        }
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        let mut functions = HashMap::new();
        register_native(&mut functions, "await", task_await32, 1);
        register_native(&mut functions, "try_await", task_try_await32, 1);
        register_native(
            &mut functions,
            "join_all",
            task_join_all32,
            lk_core::vm::NativeEntry32::VARIADIC,
        );
        register_native(&mut functions, "sleep", task_sleep32, 1);
        register_native(&mut functions, "spawn_blocking", task_spawn_blocking32, 1);
        functions
    }
}

impl TaskModule {
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
    let task = task_arg(args.get(0).expect("checked arity"), &runtime.state.heap, "task.await()")?;
    let value = rt::with_runtime(|rt| rt.block_on(rt.join_task(task.id)))
        .map_err(|err| anyhow!("Failed to await task: {err}"))?;
    runtime_payload_into_value(value, runtime.heap_mut())
}

fn task_try_await32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "task.try_await()")?;
    let task = task_arg(
        args.get(0).expect("checked arity"),
        &runtime.state.heap,
        "task.try_await()",
    )?;
    let value = match &task.value {
        Some(value) => runtime_payload_ref_to_value(value, runtime.heap_mut())?,
        None => RuntimeVal::Nil,
    };
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(
        lk_core::val::TypedList::Mixed(vec![RuntimeVal::Bool(task.value.is_some()), value]),
    ))))
}

fn task_join_all32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args.as_slice() {
        let task = task_arg(arg, &runtime.state.heap, "task.join_all()")?;
        let value = rt::with_runtime(|rt| rt.block_on(rt.join_task(task.id)))
            .map_err(|err| anyhow!("Failed to await task: {err}"))?;
        values.push(runtime_payload_into_value(value, runtime.heap_mut())?);
    }
    let list = lk_core::val::TypedList::from_runtime_values(values, &runtime.state.heap);
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn runtime_payload_into_value(payload: RuntimePayload, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let mut payload_heap = payload.heap;
    copy_runtime_value(&payload.value, &mut payload_heap, heap)
}

fn runtime_payload_ref_to_value(payload: &RuntimePayload, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let mut payload_heap = payload.heap.clone();
    copy_runtime_value(&payload.value, &mut payload_heap, heap)
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
    if !is_callable(args.get(0).expect("checked arity"), &runtime.state.heap)? {
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
    use lk_core::{module::Module, val::CallableValue, vm::RuntimeModuleState32};

    fn task_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = TaskModule::new().exports();
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
        let (_, function) = task_native(name)?;
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

    fn resolved_task(value: Val, heap: &mut HeapStore) -> RuntimeVal {
        let value = lk_core::val::val_to_runtime_val(&value, heap).expect("test value converts to runtime");
        let payload = RuntimePayload::new(value, heap.clone());
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
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let task = resolved_task(Val::Int(42), &mut state.heap);
        let result = call("try_await", &[task], &mut state)?;
        assert_eq!(
            expect_list(&result, &state.heap),
            vec![RuntimeVal::Bool(true), RuntimeVal::Int(42)]
        );
        Ok(())
    }

    #[test]
    fn task_join_all_empty_returns_empty_list() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let result = call("join_all", &[], &mut state)?;
        assert_eq!(expect_list(&result, &state.heap), Vec::<RuntimeVal>::new());
        Ok(())
    }

    #[test]
    fn task_sleep_accepts_zero_duration() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        assert_eq!(call("sleep", &[RuntimeVal::Int(0)], &mut state)?, RuntimeVal::Nil);
        Ok(())
    }

    #[test]
    fn task_spawn_blocking_rejects_non_callable() {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let err = call("spawn_blocking", &[RuntimeVal::Int(1)], &mut state).expect_err("non-callable should fail");
        assert!(err.to_string().contains("expects a function"));
    }

    #[test]
    fn task_try_await_pending_returns_false_nil() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
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
