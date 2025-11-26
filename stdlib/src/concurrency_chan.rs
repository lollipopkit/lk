//! Channel module for LKR
//!
//! Provides channel operations for inter-task communication.

use anyhow::{Result, anyhow};
use lkr_core::{module, module::Module, rt, val::Val, vm::VmContext};
use std::collections::HashMap;

/// Channel module - provides channel operations
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
        let exports = self.exports();
        for (name, value) in exports {
            registry.register_builtin(&format!("{}::{}", self.name(), name), value);
        }
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        let mut functions = HashMap::new();

        functions.insert("close".to_string(), Val::RustFunction(chan_close));
        functions.insert("len".to_string(), Val::RustFunction(chan_len));
        functions.insert("capacity".to_string(), Val::RustFunction(chan_capacity));
        functions.insert("is_closed".to_string(), Val::RustFunction(chan_is_closed));
        functions.insert("try_send".to_string(), Val::RustFunction(chan_try_send));
        functions.insert("try_recv".to_string(), Val::RustFunction(chan_try_recv));

        functions
    }
}

impl ChannelModule {
    pub fn new() -> Self {
        Self
    }
}

/// Close a channel
fn chan_close(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("chan::close() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Channel(channel) => match rt::with_runtime(|runtime| runtime.close_channel(channel.id)) {
            Ok(()) => Ok(Val::Nil),
            Err(e) => Err(anyhow!("Failed to close channel: {}", e)),
        },
        _ => Err(anyhow!("chan::close() expects a Channel argument")),
    }
}

/// Get the current length of a channel (number of buffered items)
fn chan_len(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("chan::len() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Channel(_) => {
            // TODO: This is a simplified implementation
            // In practice, we'd need to track the actual buffer length
            Ok(Val::Int(0))
        }
        _ => Err(anyhow!("chan::len() expects a Channel argument")),
    }
}

/// Get the capacity of a channel
fn chan_capacity(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("chan::capacity() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Channel(channel) => {
            match channel.capacity {
                Some(cap) => Ok(Val::Int(cap)),
                None => Ok(Val::Int(0)), // Unbounded channels return 0 capacity
            }
        }
        _ => Err(anyhow!("chan::capacity() expects a Channel argument")),
    }
}

/// Check if a channel is closed
fn chan_is_closed(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("chan::is_closed() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Channel(_) => {
            // TODO: This is a simplified implementation
            // In practice, we'd need to check the actual channel state
            Ok(Val::Bool(false))
        }
        _ => Err(anyhow!("chan::is_closed() expects a Channel argument")),
    }
}

/// Try to send a value to a channel without blocking
fn chan_try_send(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("chan::try_send() expects exactly 2 arguments"));
    }

    let channel = &args[0];
    let value = &args[1];

    match channel {
        Val::Channel(channel) => match rt::with_runtime(|runtime| runtime.try_send(channel.id, value.clone())) {
            Ok(success) => Ok(Val::Bool(success)),
            Err(e) => Err(anyhow!("Failed to send to channel: {}", e)),
        },
        _ => Err(anyhow!("chan::try_send() expects a Channel as first argument")),
    }
}

/// Try to receive a value from a channel without blocking
fn chan_try_recv(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("chan::try_recv() expects exactly 1 argument"));
    }

    match &args[0] {
        Val::Channel(channel) => match rt::with_runtime(|runtime| runtime.try_recv(channel.id)) {
            Ok(Some((ok, value))) => Ok(Val::List(vec![Val::Bool(ok), value].into())),
            Ok(None) => Ok(Val::List(vec![Val::Bool(false), Val::Nil].into())),
            Err(e) => Err(anyhow!("Failed to receive from channel: {}", e)),
        },
        _ => Err(anyhow!("chan::try_recv() expects a Channel argument")),
    }
}
