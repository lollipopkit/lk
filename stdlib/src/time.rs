//! Time module for LKR concurrency
//!
//! Provides timing and scheduling functions for concurrent operations.

use anyhow::{Result, anyhow};
use lkr_core::{
    module,
    module::Module,
    rt::with_runtime,
    val::{self, ChannelValue, Val},
    vm::VmContext,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{collections::HashMap, sync::Arc};

/// Time module - provides timing functions
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
        let exports = self.exports();
        for (name, value) in exports {
            registry.register_builtin(&format!("{}::{}", self.name(), name), value);
        }
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        let mut functions = HashMap::new();

        functions.insert("sleep".to_string(), Val::RustFunction(time_sleep));
        functions.insert("timeout".to_string(), Val::RustFunction(time_timeout));
        functions.insert("after".to_string(), Val::RustFunction(time_after));
        functions.insert("now".to_string(), Val::RustFunction(time_now));
        functions.insert("since".to_string(), Val::RustFunction(time_since));

        functions
    }
}

impl TimeModule {
    pub fn new() -> Self {
        Self
    }
}

/// Sleep for the specified duration in milliseconds
fn time_sleep(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("time::sleep() expects exactly 1 argument"));
    }

    let duration_ms = match &args[0] {
        Val::Int(ms) => *ms,
        Val::Float(ms) => *ms as i64,
        _ => return Err(anyhow!("time::sleep() expects a numeric argument")),
    };

    match with_runtime(|runtime| {
        let duration = Duration::from_millis(duration_ms as u64);
        runtime.block_on(async {
            tokio::time::sleep(duration).await;
            Ok(Val::Nil)
        })
    }) {
        Ok(result) => Ok(result),
        Err(e) => Err(anyhow!("Failed to sleep: {}", e)),
    }
}

/// Create a timeout channel that fires after the specified duration
fn time_timeout(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("time::timeout() expects exactly 1 argument"));
    }

    let duration_ms = match &args[0] {
        Val::Int(ms) => *ms,
        Val::Float(ms) => *ms as i64,
        _ => return Err(anyhow!("time::timeout() expects a numeric argument")),
    };

    match with_runtime(|runtime| {
        let duration = Duration::from_millis(duration_ms as u64);

        // Create a channel for the timeout
        let channel_id = runtime.create_channel(Some(1))?;

        // Get the channel ID before moving into the async block
        let timeout_channel_id = channel_id;

        // Spawn a task to send a signal after the timeout
        let future = async move {
            tokio::time::sleep(duration).await;
            // Use a new runtime reference to send the signal
            match with_runtime(|rt| rt.try_send(timeout_channel_id, Val::Nil)) {
                Ok(_success) => Ok(Val::Nil),
                Err(e) => Err(anyhow!("Failed to send timeout signal: {}", e)),
            }
        };

        runtime.spawn(future)?;

        Ok(Val::Channel(Arc::new(ChannelValue {
            id: channel_id,
            capacity: Some(1),
            inner_type: val::Type::Nil,
        })))
    }) {
        Ok(channel) => Ok(channel),
        Err(e) => Err(anyhow!("Failed to create timeout: {}", e)),
    }
}

/// Create a one-shot timer that fires after the specified duration
fn time_after(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("time::after() expects exactly 1 argument"));
    }

    let duration_ms = match &args[0] {
        Val::Int(ms) => *ms,
        Val::Float(ms) => *ms as i64,
        _ => return Err(anyhow!("time::after() expects a numeric argument")),
    };

    match with_runtime(|runtime| {
        let duration = Duration::from_millis(duration_ms as u64);

        // Create a channel for the timer
        let channel_id = runtime.create_channel(Some(1))?;

        // Get the channel ID before moving into the async block
        let timer_channel_id = channel_id;

        // Spawn a task to send the current time after the duration
        let future = async move {
            tokio::time::sleep(duration).await;
            let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
            match with_runtime(|rt| rt.try_send(timer_channel_id, Val::Int(current_time))) {
                Ok(_success) => Ok(Val::Nil),
                Err(e) => Err(anyhow!("Failed to send timer signal: {}", e)),
            }
        };

        runtime.spawn(future)?;

        Ok(Val::Channel(Arc::new(ChannelValue {
            id: channel_id,
            capacity: Some(1),
            inner_type: val::Type::Int,
        })))
    }) {
        Ok(channel) => Ok(channel),
        Err(e) => Err(anyhow!("Failed to create timer: {}", e)),
    }
}

/// Get the current time in milliseconds since Unix epoch
fn time_now(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if !args.is_empty() {
        return Err(anyhow!("time::now() expects no arguments"));
    }

    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;

    Ok(Val::Int(current_time))
}

/// Calculate the duration between two timestamps in milliseconds
fn time_since(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("time::since() expects exactly 2 arguments"));
    }

    let start_time = match &args[0] {
        Val::Int(ms) => *ms,
        Val::Float(ms) => *ms as i64,
        _ => return Err(anyhow!("time::since() expects numeric arguments")),
    };

    let end_time = match &args[1] {
        Val::Int(ms) => *ms,
        Val::Float(ms) => *ms as i64,
        _ => return Err(anyhow!("time::since() expects numeric arguments")),
    };

    let duration = end_time - start_time;
    Ok(Val::Int(duration))
}
