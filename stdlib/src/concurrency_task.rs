//! Concurrency module for LKR
//!
//! Provides Go-style concurrency primitives including tasks, channels, and synchronization.

use anyhow::{Result, anyhow};
use lkr_core::{
    module::{self, Module},
    rt,
    val::{TaskValue, Val},
    vm::VmContext,
};
use std::{collections::HashMap, sync::Arc};

/// Task module - provides task management functions
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
        let exports = self.exports();
        for (name, value) in exports {
            registry.register_builtin(&format!("{}::{}", self.name(), name), value);
        }
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        let mut functions = HashMap::new();

        functions.insert("await".to_string(), Val::RustFunction(task_await));
        functions.insert("try_await".to_string(), Val::RustFunction(task_try_await));
        functions.insert("join_all".to_string(), Val::RustFunction(task_join_all));
        functions.insert("sleep".to_string(), Val::RustFunction(task_sleep));
        functions.insert("spawn_blocking".to_string(), Val::RustFunction(task_spawn_blocking));

        functions
    }
}

impl TaskModule {
    pub fn new() -> Self {
        Self
    }
}

/// Await a task to complete and return its result
fn task_await(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("task::await() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Task(task) => match rt::with_runtime(|runtime| runtime.block_on(runtime.join_task(task.id))) {
            Ok(result) => Ok(result),
            Err(e) => Err(anyhow!("Failed to await task: {}", e)),
        },
        _ => Err(anyhow!("task::await() expects a Task argument")),
    }
}

/// Try to await a task, returning None if not ready
fn task_try_await(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("task::try_await() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Task(task) => {
            // In the current implementation, we don't have non-blocking await
            // For now, just check if we have a pre-computed value
            match &task.value {
                Some(val) => Ok(Val::List(vec![Val::Bool(true), val.clone()].into())),
                None => Ok(Val::List(vec![Val::Bool(false), Val::Nil].into())),
            }
        }
        _ => Err(anyhow!("task::try_await() expects a Task argument")),
    }
}

/// Join multiple tasks and return their results as a list
fn task_join_all(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.is_empty() {
        return Ok(Val::List(vec![].into()));
    }

    let mut results = Vec::new();

    for arg in args {
        match arg {
            Val::Task(task) => match rt::with_runtime(|runtime| runtime.block_on(runtime.join_task(task.id))) {
                Ok(result) => results.push(result),
                Err(e) => return Err(anyhow!("Failed to await task: {}", e)),
            },
            _ => return Err(anyhow!("task::join_all() expects Task arguments")),
        }
    }

    Ok(Val::List(results.into()))
}

/// Sleep for the specified duration in milliseconds
fn task_sleep(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("task::sleep() expects exactly 1 argument"));
    }

    let duration_ms = match &args[0] {
        Val::Int(ms) => *ms,
        Val::Float(ms) => *ms as i64,
        _ => return Err(anyhow!("task::sleep() expects a numeric argument")),
    };

    match rt::with_runtime(|runtime| {
        let duration = std::time::Duration::from_millis(duration_ms as u64);
        runtime.block_on(async {
            tokio::time::sleep(duration).await;
            Ok(Val::Nil)
        })
    }) {
        Ok(result) => Ok(result),
        Err(e) => Err(anyhow!("Failed to sleep: {}", e)),
    }
}

/// Spawn a blocking task (CPU-intensive work)
fn task_spawn_blocking(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("task::spawn_blocking() expects exactly 1 argument"));
    }

    // Extract the function from the argument
    let _func = match &args[0] {
        Val::RustFunction(f) => *f,
        Val::Closure(_) => {
            return Err(anyhow!("task::spawn_blocking() does not support closures yet"));
        }
        _ => {
            return Err(anyhow!("task::spawn_blocking() expects a function argument"));
        }
    };

    // Note: This is a simplified implementation that doesn't capture env
    // In a full implementation, we'd need to handle the lifetime issues
    match rt::with_runtime(|runtime| {
        let future = async move {
            // For now, execute with empty args - ctx handling would need proper lifetime management
            // TODO: Fix this when we have proper VmContext cloning/serialization
            Err(anyhow!("task::spawn_blocking() needs VmContext lifetime management"))
            // func(&[], ctx)
        };
        runtime.spawn(future)
    }) {
        Ok(task_id) => Ok(Val::Task(Arc::new(TaskValue {
            id: task_id,
            value: None,
        }))),
        Err(e) => Err(anyhow!("Failed to spawn blocking task: {}", e)),
    }
}
