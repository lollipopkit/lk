pub mod collections;
pub mod concurrency_chan;
pub mod concurrency_task;
pub mod datetime;
pub mod io;
pub mod iter;
pub mod json;
pub mod list;
pub mod map;
pub mod math;
pub mod os;
pub mod stream;
pub mod string;
pub mod tcp;
pub mod time;
pub mod toml;
pub mod yaml;

#[cfg(test)]
mod datetime_test;
#[cfg(test)]
mod globals_test;
#[cfg(test)]
mod list_test;
#[cfg(test)]
mod math_test;
#[cfg(test)]
mod os_test;
#[cfg(test)]
mod stream_test;
#[cfg(test)]
mod string_test;
#[cfg(test)]
mod tcp_test;

use anyhow::{Result, anyhow};
use lk_core::{
    module::ModuleRegistry,
    rt, val,
    val::{ChannelValue, TaskValue, Val},
    vm::VmContext,
};
use std::sync::Arc;

/// Register all stdlib modules with the given registry
pub fn register_stdlib_modules(registry: &mut ModuleRegistry) -> Result<()> {
    for name in [
        "io", "json", "yaml", "toml", "iter", "math", "string", "list", "map", "datetime", "os", "tcp", "stream",
        "task", "chan", "time",
    ] {
        register_stdlib_module_by_name(registry, name)?;
    }
    Ok(())
}

/// Register a selected subset of stdlib modules. Unknown names are ignored so
/// package modules can share the same import collection path and resolve later.
pub fn register_stdlib_modules_named(registry: &mut ModuleRegistry, names: &[String]) -> Result<()> {
    for name in names {
        register_stdlib_module_by_name(registry, name)?;
    }
    Ok(())
}

fn register_stdlib_module_by_name(registry: &mut ModuleRegistry, name: &str) -> Result<()> {
    match name {
        "io" => registry.register_module("io", Box::new(io::IoModule::new()))?,
        "json" => registry.register_module("json", Box::new(json::JsonModule::new()))?,
        "yaml" => registry.register_module("yaml", Box::new(yaml::YamlModule::new()))?,
        "toml" => registry.register_module("toml", Box::new(toml::TomlModule::new()))?,
        "iter" => registry.register_module("iter", Box::new(iter::IterModule::new()))?,
        "math" => registry.register_module("math", Box::new(math::MathModule::new()))?,
        "string" => registry.register_module("string", Box::new(string::StringModule::new()))?,
        "list" => registry.register_module("list", Box::new(list::ListModule::new()))?,
        "map" => registry.register_module("map", Box::new(map::MapModule::new()))?,
        "datetime" => registry.register_module("datetime", Box::new(datetime::DateTimeModule::new()))?,
        "os" => registry.register_module("os", Box::new(os::OsModule::new()))?,
        "tcp" => registry.register_module("tcp", Box::new(tcp::TcpModule::new()))?,
        "stream" => registry.register_module("stream", Box::new(stream::StreamModule::new()))?,
        "task" => registry.register_module("task", Box::new(concurrency_task::TaskModule::new()))?,
        "chan" => registry.register_module("chan", Box::new(concurrency_chan::ChannelModule::new()))?,
        "time" => registry.register_module("time", Box::new(time::TimeModule::new()))?,
        _ => {}
    }
    Ok(())
}

pub fn register_stdlib_module_io(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("io", Box::new(io::IoModule::new()))
}

pub fn register_stdlib_module_json(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("json", Box::new(json::JsonModule::new()))
}

pub fn register_stdlib_module_yaml(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("yaml", Box::new(yaml::YamlModule::new()))
}

pub fn register_stdlib_module_toml(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("toml", Box::new(toml::TomlModule::new()))
}

pub fn register_stdlib_module_iter(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("iter", Box::new(iter::IterModule::new()))
}

pub fn register_stdlib_module_math(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("math", Box::new(math::MathModule::new()))
}

pub fn register_stdlib_module_string(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("string", Box::new(string::StringModule::new()))
}

pub fn register_stdlib_module_list(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("list", Box::new(list::ListModule::new()))
}

pub fn register_stdlib_module_map(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("map", Box::new(map::MapModule::new()))
}

pub fn register_stdlib_module_datetime(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("datetime", Box::new(datetime::DateTimeModule::new()))
}

pub fn register_stdlib_module_os(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("os", Box::new(os::OsModule::new()))
}

pub fn register_stdlib_module_tcp(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("tcp", Box::new(tcp::TcpModule::new()))
}

pub fn register_stdlib_module_stream(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("stream", Box::new(stream::StreamModule::new()))
}

pub fn register_stdlib_module_task(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("task", Box::new(concurrency_task::TaskModule::new()))
}

pub fn register_stdlib_module_chan(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("chan", Box::new(concurrency_chan::ChannelModule::new()))
}

pub fn register_stdlib_module_time(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("time", Box::new(time::TimeModule::new()))
}

/// Register global builtin functions available without import
/// - print(fmt, ...args): print formatted text without newline; returns nil
/// - println(fmt, ...args): print formatted text with newline; returns nil
/// - panic([msg]): raise a runtime error with optional message and backtrace
pub fn register_stdlib_core_globals(registry: &mut ModuleRegistry) {
    fn format_variadic(args: &[Val], ctx: &mut VmContext) -> String {
        if args.is_empty() {
            return String::new();
        }
        if let Val::Str(fmt) = &args[0] {
            // Simple {} placeholder formatting; additional args appended with spaces
            let rest = &args[1..];
            let mut out = String::with_capacity(fmt.len() + rest.len() * 8);
            let chars: Vec<char> = fmt.chars().collect();
            let mut i = 0usize;
            let mut arg_idx = 0usize;
            while i < chars.len() {
                if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '}' {
                    if arg_idx < rest.len() {
                        out.push_str(&rest[arg_idx].display_string(Some(ctx)));
                        arg_idx += 1;
                    } else {
                        out.push('{');
                        out.push('}');
                    }
                    i += 2;
                } else {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            // Append any remaining args separated by spaces
            if arg_idx < rest.len() {
                if !out.is_empty() {
                    out.push(' ');
                }
                for (j, v) in rest[arg_idx..].iter().enumerate() {
                    if j > 0 {
                        out.push(' ');
                    }
                    out.push_str(&v.display_string(Some(ctx)));
                }
            }
            out
        } else {
            // No format string; join all args by spaces
            let mut out = String::new();
            for (i, v) in args.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&v.display_string(Some(ctx)));
            }
            out
        }
    }

    fn print_fn(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
        let out = format_variadic(args, ctx);
        print!("{}", out);
        Ok(Val::Nil)
    }

    fn println_fn(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
        let out = format_variadic(args, ctx);
        println!("{}", out);
        Ok(Val::Nil)
    }

    fn panic_fn(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
        // Compose message from all arguments for better diagnostics
        let mut msg = if args.is_empty() {
            "panic".to_string()
        } else {
            let mut s = String::new();
            for (i, v) in args.iter().enumerate() {
                if i > 0 {
                    s.push(' ');
                }
                s.push_str(&v.to_string());
            }
            s
        };
        // Attach a backtrace explicitly so users always see it regardless of env var
        let bt = std::backtrace::Backtrace::force_capture();
        msg.push_str("\nBacktrace:\n");
        msg.push_str(&format!("{}", bt));
        panic!("{}", msg);
    }

    registry.register_builtin("print", Val::RustFunction(print_fn));
    registry.register_builtin("println", Val::RustFunction(println_fn));
    registry.register_builtin("panic", Val::RustFunction(panic_fn));
}

pub fn register_stdlib_concurrency_globals(registry: &mut ModuleRegistry) {
    // Concurrency conveniences available as globals for VM-lowered constructs
    // - spawn(closure) -> Task
    // - chan(capacity[, type_str]) -> Channel

    use val::Type as LkType;

    fn spawn_fn(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("spawn() expects exactly 1 argument (closure/function)"));
        }
        // Clone environment for use inside async task
        let env_cloned = ctx.clone();

        #[allow(clippy::redundant_closure)]
        let fut: core::pin::Pin<Box<dyn core::future::Future<Output = Result<Val>> + Send>> = match &args[0] {
            Val::Closure(_) => {
                let func = args[0].clone();
                Box::pin(async move {
                    let mut temp_ctx = env_cloned;
                    func.call(&[], &mut temp_ctx)
                })
            }
            Val::RustFunction(fptr) => {
                let f = *fptr;
                Box::pin(async move {
                    let mut temp_ctx = env_cloned;
                    f(&[], &mut temp_ctx)
                })
            }
            other => {
                return Err(anyhow!(
                    "spawn() expects a function or closure, got {}",
                    other.type_name()
                ));
            }
        };

        match rt::with_runtime(|runtime| runtime.spawn(fut)) {
            Ok(task_id) => Ok(Val::Task(Arc::new(TaskValue {
                id: task_id,
                value: None,
            }))),
            Err(e) => Err(anyhow!("Failed to spawn task: {}", e)),
        }
    }

    fn chan_fn(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.is_empty() || args.len() > 2 {
            return Err(anyhow!("chan() expects 1 or 2 arguments: capacity[, type_str]"));
        }
        let capacity = match &args[0] {
            Val::Int(n) => *n,
            Val::Float(f) => *f as i64,
            other => return Err(anyhow!("chan() capacity must be numeric, got {}", other.type_name())),
        };
        let inner_type = if args.len() >= 2 {
            match &args[1] {
                Val::Str(s) => LkType::parse(s.as_ref()).unwrap_or(LkType::Nil),
                Val::Nil => LkType::Nil,
                other => {
                    return Err(anyhow!(
                        "chan() type must be a string when provided, got {}",
                        other.type_name()
                    ));
                }
            }
        } else {
            LkType::Nil
        };

        let cap_opt = if capacity <= 0 { None } else { Some(capacity as usize) };
        match rt::with_runtime(|runtime| runtime.create_channel(cap_opt)) {
            Ok(ch_id) => Ok(Val::Channel(Arc::new(ChannelValue {
                id: ch_id,
                capacity: Some(capacity),
                inner_type,
            }))),
            Err(e) => Err(anyhow!("Failed to create channel: {}", e)),
        }
    }

    fn send_fn(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("send() expects exactly 2 arguments"));
        }

        let channel_id = match &args[0] {
            Val::Channel(channel) => channel.id,
            other => {
                return Err(anyhow!(
                    "send() expects Channel as first argument, got {}",
                    other.type_name()
                ));
            }
        };

        match rt::with_runtime(|runtime| runtime.block_on(runtime.send_async(channel_id, args[1].clone()))) {
            Ok(sent) => Ok(Val::Bool(sent)),
            Err(e) => Err(anyhow!("Send operation failed: {}", e)),
        }
    }

    fn recv_fn(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("recv() expects exactly 1 argument"));
        }

        let channel_id = match &args[0] {
            Val::Channel(channel) => channel.id,
            other => {
                return Err(anyhow!(
                    "recv() expects Channel as first argument, got {}",
                    other.type_name()
                ));
            }
        };

        match rt::with_runtime(|runtime| runtime.block_on(runtime.recv_async(channel_id))) {
            Ok((ok, value)) => Ok(Val::List(vec![Val::Bool(ok), value].into())),
            Err(e) => Err(anyhow!("Receive operation failed: {}", e)),
        }
    }

    registry.register_builtin("spawn", Val::RustFunction(spawn_fn));
    registry.register_builtin("chan", Val::RustFunction(chan_fn));
    registry.register_builtin("send", Val::RustFunction(send_fn));
    registry.register_builtin("recv", Val::RustFunction(recv_fn));
    // Expose non-blocking channel helpers as global builtins for VM-lowered select/send/recv
    fn chan_try_send_fn(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("chan::try_send() expects exactly 2 arguments"));
        }
        let ch_id = match &args[0] {
            Val::Channel(channel) => channel.id,
            other => {
                return Err(anyhow!(
                    "chan::try_send() expects Channel as first arg, got {}",
                    other.type_name()
                ));
            }
        };
        match rt::with_runtime(|runtime| runtime.try_send(ch_id, args[1].clone())) {
            Ok(sent) => Ok(Val::Bool(sent)),
            Err(e) => Err(anyhow!("Failed to send to channel: {}", e)),
        }
    }

    fn chan_try_recv_fn(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("chan::try_recv() expects exactly 1 argument"));
        }
        let ch_id = match &args[0] {
            Val::Channel(channel) => channel.id,
            other => {
                return Err(anyhow!(
                    "chan::try_recv() expects Channel as first arg, got {}",
                    other.type_name()
                ));
            }
        };
        match rt::with_runtime(|runtime| runtime.try_recv(ch_id))? {
            Some((ok, value)) => Ok(Val::List(vec![Val::Bool(ok), value].into())),
            None => Ok(Val::List(vec![Val::Bool(false), Val::Nil].into())),
        }
    }

    registry.register_builtin("chan::try_send", Val::RustFunction(chan_try_send_fn));
    registry.register_builtin("chan::try_recv", Val::RustFunction(chan_try_recv_fn));

    // Blocking select helper for VM-lowered select semantics
    fn select_block_fn(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        use rt::SelectOperation;
        if args.len() != 5 {
            return Err(anyhow!(
                "select$block expects 5 arguments: types, channels, values, guards, has_default"
            ));
        }
        // Unpack lists
        let types = match &args[0] {
            Val::List(l) => l.clone(),
            _ => return Err(anyhow!("select$block: types must be a List")),
        };
        let channels = match &args[1] {
            Val::List(l) => l.clone(),
            _ => return Err(anyhow!("select$block: channels must be a List")),
        };
        let values = match &args[2] {
            Val::List(l) => l.clone(),
            _ => return Err(anyhow!("select$block: values must be a List")),
        };
        let guards = match &args[3] {
            Val::List(l) => l.clone(),
            _ => return Err(anyhow!("select$block: guards must be a List")),
        };
        let has_default = match &args[4] {
            Val::Bool(b) => *b,
            _ => return Err(anyhow!("select$block: has_default must be a Bool")),
        };
        let n = types.len();
        if channels.len() != n || values.len() != n || guards.len() != n {
            return Err(anyhow!("select$block: all lists must have equal length"));
        }

        let mut sel = SelectOperation::new();
        for i in 0..n {
            // Only include guarded arms
            let guard_true = matches!(guards[i].clone(), Val::Bool(true));
            if !guard_true {
                continue;
            }
            match (&types[i], &channels[i]) {
                (Val::Int(t), Val::Channel(channel)) if *t == 0 => {
                    sel.add_recv(i, channel.id);
                }
                (Val::Int(t), Val::Channel(channel)) if *t == 1 => {
                    let v = values[i].clone();
                    sel.add_send(i, channel.id, v);
                }
                _ => return Err(anyhow!("select$block: invalid arm entry types")),
            }
        }

        let result = rt::with_runtime(|runtime| runtime.block_on(sel.execute(runtime, has_default)))?;
        if result.is_default {
            // [is_default=true, case_index=-1, payload=nil]
            Ok(Val::List(vec![Val::Bool(true), Val::Int(-1), Val::Nil].into()))
        } else {
            let idx = result
                .case_index
                .ok_or_else(|| anyhow!("select returned no case index"))? as i64;
            let payload = match result.recv_payload {
                Some((ok, v)) => Val::List(vec![Val::Bool(ok), v].into()),
                None => Val::Nil,
            };
            Ok(Val::List(vec![Val::Bool(false), Val::Int(idx), payload].into()))
        }
    }

    registry.register_builtin("select$block", Val::RustFunction(select_block_fn));
}

pub fn register_stdlib_globals(registry: &mut ModuleRegistry) {
    register_stdlib_core_globals(registry);
    register_stdlib_concurrency_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_globals(registry: &mut ModuleRegistry) {
    register_stdlib_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_core_globals(registry: &mut ModuleRegistry) {
    register_stdlib_core_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_concurrency_globals(registry: &mut ModuleRegistry) {
    register_stdlib_concurrency_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_modules(registry: &mut ModuleRegistry) -> Result<()> {
    register_stdlib_modules(registry)
}

macro_rules! export_stdlib_module_registrar {
    ($export:ident, $register:ident) => {
        #[unsafe(no_mangle)]
        pub extern "Rust" fn $export(registry: &mut ModuleRegistry) -> Result<()> {
            $register(registry)
        }
    };
}

export_stdlib_module_registrar!(lk_stdlib_register_module_io, register_stdlib_module_io);
export_stdlib_module_registrar!(lk_stdlib_register_module_json, register_stdlib_module_json);
export_stdlib_module_registrar!(lk_stdlib_register_module_yaml, register_stdlib_module_yaml);
export_stdlib_module_registrar!(lk_stdlib_register_module_toml, register_stdlib_module_toml);
export_stdlib_module_registrar!(lk_stdlib_register_module_iter, register_stdlib_module_iter);
export_stdlib_module_registrar!(lk_stdlib_register_module_math, register_stdlib_module_math);
export_stdlib_module_registrar!(lk_stdlib_register_module_string, register_stdlib_module_string);
export_stdlib_module_registrar!(lk_stdlib_register_module_list, register_stdlib_module_list);
export_stdlib_module_registrar!(lk_stdlib_register_module_map, register_stdlib_module_map);
export_stdlib_module_registrar!(lk_stdlib_register_module_datetime, register_stdlib_module_datetime);
export_stdlib_module_registrar!(lk_stdlib_register_module_os, register_stdlib_module_os);
export_stdlib_module_registrar!(lk_stdlib_register_module_tcp, register_stdlib_module_tcp);
export_stdlib_module_registrar!(lk_stdlib_register_module_stream, register_stdlib_module_stream);
export_stdlib_module_registrar!(lk_stdlib_register_module_task, register_stdlib_module_task);
export_stdlib_module_registrar!(lk_stdlib_register_module_chan, register_stdlib_module_chan);
export_stdlib_module_registrar!(lk_stdlib_register_module_time, register_stdlib_module_time);

#[cfg(test)]
mod aot_registration_tests {
    use super::*;

    #[test]
    fn named_registration_includes_only_requested_modules() {
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules_named(&mut registry, &["math".to_string()]).expect("register math");

        assert!(registry.get_module("math").is_ok());
        assert!(registry.get_module("json").is_err());
    }

    #[test]
    fn core_globals_exclude_concurrency_helpers() {
        let mut registry = ModuleRegistry::new();
        register_stdlib_core_globals(&mut registry);

        assert!(registry.get_builtin("println").is_some());
        assert!(registry.get_builtin("spawn").is_none());
        assert!(registry.get_builtin("select$block").is_none());
    }
}
