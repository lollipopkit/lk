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
mod runtime_native;
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
    val::{
        CallableValue, ChannelValue, HeapStore, HeapValue, RuntimeVal, TaskValue, TypedList, Val, runtime_val_to_val,
        val_to_runtime_val,
    },
    vm::{
        NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, call_runtime_callable32_runtime,
        runtime_value_to_callable32,
    },
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
    register_runtime_builtin(registry, "print", print32, NativeEntry32::VARIADIC);
    register_runtime_builtin(registry, "println", println32, NativeEntry32::VARIADIC);
    register_runtime_builtin(registry, "panic", panic32, NativeEntry32::VARIADIC);
}

pub fn register_stdlib_concurrency_globals(registry: &mut ModuleRegistry) {
    register_runtime_builtin(registry, "spawn", spawn32, 1);
    register_runtime_builtin(registry, "chan", chan32, NativeEntry32::VARIADIC);
    register_runtime_builtin(registry, "send", send32, 2);
    register_runtime_builtin(registry, "recv", recv32, 1);
    register_runtime_builtin(registry, "chan::try_send", chan_try_send32, 2);
    register_runtime_builtin(registry, "chan::try_recv", chan_try_recv32, 1);
    register_runtime_builtin(registry, "select$block", select_block32, 5);
}

fn register_runtime_builtin(
    registry: &mut ModuleRegistry,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    registry.register_builtin(name, Val::runtime_native32(NativeFunction32::Plain(function), arity));
}

fn print32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    print!("{}", format_variadic_runtime(args.as_slice(), &runtime.state.heap)?);
    Ok(RuntimeVal::Nil)
}

fn println32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    println!("{}", format_variadic_runtime(args.as_slice(), &runtime.state.heap)?);
    Ok(RuntimeVal::Nil)
}

fn panic32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let mut msg = if args.is_empty() {
        "panic".to_string()
    } else {
        join_runtime_display(args.as_slice(), &runtime.state.heap)?
    };
    let bt = std::backtrace::Backtrace::force_capture();
    msg.push_str("\nBacktrace:\n");
    msg.push_str(&format!("{}", bt));
    panic!("{}", msg);
}

fn spawn32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 1, "spawn")?;
    let function = runtime_callable_arg(args.get(0).expect("arity checked"), runtime, "spawn argument")?;
    let mut ctx = runtime
        .ctx
        .as_deref()
        .cloned()
        .unwrap_or_else(lk_core::vm::VmContext::new_without_core_vm_builtins);

    let fut: core::pin::Pin<Box<dyn core::future::Future<Output = Result<Val>> + Send>> = Box::pin(async move {
        let mut heap = HeapStore::new();
        let result = call_runtime_callable32_runtime(&function, NativeArgs32::new(&[]), &mut heap, Some(&mut ctx))?;
        runtime_val_to_val(&result, &heap)
    });

    let task_id =
        rt::with_runtime(|runtime| runtime.spawn(fut)).map_err(|error| anyhow!("Failed to spawn task: {}", error))?;
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Task(Arc::new(
        TaskValue {
            id: task_id,
            value: None,
        },
    )))))
}

fn chan32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    if args.is_empty() || args.len() > 2 {
        return Err(anyhow!("chan() expects 1 or 2 arguments: capacity[, type_str]"));
    }
    let values = args.as_slice();
    let capacity = match &values[0] {
        RuntimeVal::Int(value) => *value,
        RuntimeVal::Float(value) => *value as i64,
        other => {
            return Err(anyhow!(
                "chan() capacity must be numeric, got {}",
                runtime_type_name(other, &runtime.state.heap)
            ));
        }
    };
    let inner_type = if values.len() == 2 {
        match &values[1] {
            RuntimeVal::Nil => val::Type::Nil,
            value => {
                let text = runtime_string(value, &runtime.state.heap, "chan() type")?;
                val::Type::parse(text.as_ref()).unwrap_or(val::Type::Nil)
            }
        }
    } else {
        val::Type::Nil
    };
    let cap_opt = if capacity <= 0 { None } else { Some(capacity as usize) };
    let channel_id = rt::with_runtime(|runtime| runtime.create_channel(cap_opt))
        .map_err(|error| anyhow!("Failed to create channel: {}", error))?;
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Channel(Arc::new(
        ChannelValue {
            id: channel_id,
            capacity: Some(capacity),
            inner_type,
        },
    )))))
}

fn send32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 2, "send")?;
    let values = args.as_slice();
    let channel_id = channel_id_arg(&values[0], &runtime.state.heap, "send first argument")?;
    let value = runtime_val_to_val(&values[1], &runtime.state.heap)?;
    let sent = rt::with_runtime(|runtime| runtime.block_on(runtime.send_async(channel_id, value)))
        .map_err(|error| anyhow!("Send operation failed: {}", error))?;
    Ok(RuntimeVal::Bool(sent))
}

fn recv32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 1, "recv")?;
    let channel_id = channel_id_arg(
        args.get(0).expect("arity checked"),
        &runtime.state.heap,
        "recv first argument",
    )?;
    let (ok, value) = rt::with_runtime(|runtime| runtime.block_on(runtime.recv_async(channel_id)))
        .map_err(|error| anyhow!("Receive operation failed: {}", error))?;
    runtime_list(
        vec![RuntimeVal::Bool(ok), val_to_runtime_val(&value, runtime.heap_mut())?],
        runtime.heap_mut(),
    )
}

fn chan_try_send32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 2, "chan::try_send")?;
    let values = args.as_slice();
    let channel_id = channel_id_arg(&values[0], &runtime.state.heap, "chan::try_send first argument")?;
    let value = runtime_val_to_val(&values[1], &runtime.state.heap)?;
    let sent = rt::with_runtime(|runtime| runtime.try_send(channel_id, value))
        .map_err(|error| anyhow!("Failed to send to channel: {}", error))?;
    Ok(RuntimeVal::Bool(sent))
}

fn chan_try_recv32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 1, "chan::try_recv")?;
    let channel_id = channel_id_arg(
        args.get(0).expect("arity checked"),
        &runtime.state.heap,
        "chan::try_recv first argument",
    )?;
    let payload = match rt::with_runtime(|runtime| runtime.try_recv(channel_id))? {
        Some((ok, value)) => vec![RuntimeVal::Bool(ok), val_to_runtime_val(&value, runtime.heap_mut())?],
        None => vec![RuntimeVal::Bool(false), RuntimeVal::Nil],
    };
    runtime_list(payload, runtime.heap_mut())
}

fn select_block32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    use rt::SelectOperation;

    expect_runtime_arity(args, 5, "select$block")?;
    let args = args.as_slice();
    let types = list_items(&args[0], runtime.heap_mut(), "select$block types")?;
    let channels = list_items(&args[1], runtime.heap_mut(), "select$block channels")?;
    let values = list_items(&args[2], runtime.heap_mut(), "select$block values")?;
    let guards = list_items(&args[3], runtime.heap_mut(), "select$block guards")?;
    let RuntimeVal::Bool(has_default) = args[4] else {
        return Err(anyhow!("select$block: has_default must be a Bool"));
    };
    let len = types.len();
    if channels.len() != len || values.len() != len || guards.len() != len {
        return Err(anyhow!("select$block: all lists must have equal length"));
    }

    let mut select = SelectOperation::new();
    for index in 0..len {
        if !matches!(guards[index], RuntimeVal::Bool(true)) {
            continue;
        }
        let RuntimeVal::Int(kind) = types[index] else {
            return Err(anyhow!("select$block: invalid arm entry types"));
        };
        let channel_id = channel_id_arg(&channels[index], &runtime.state.heap, "select$block channel")?;
        match kind {
            0 => select.add_recv(index, channel_id),
            1 => {
                let value = runtime_val_to_val(&values[index], &runtime.state.heap)?;
                select.add_send(index, channel_id, value);
            }
            _ => return Err(anyhow!("select$block: invalid arm entry types")),
        }
    }

    let result = rt::with_runtime(|runtime| runtime.block_on(select.execute(runtime, has_default)))?;
    if result.is_default {
        return runtime_list(
            vec![RuntimeVal::Bool(true), RuntimeVal::Int(-1), RuntimeVal::Nil],
            runtime.heap_mut(),
        );
    }

    let index = result
        .case_index
        .ok_or_else(|| anyhow!("select returned no case index"))? as i64;
    let payload = match result.recv_payload {
        Some((ok, value)) => runtime_list(
            vec![RuntimeVal::Bool(ok), val_to_runtime_val(&value, runtime.heap_mut())?],
            runtime.heap_mut(),
        )?,
        None => RuntimeVal::Nil,
    };
    runtime_list(
        vec![RuntimeVal::Bool(false), RuntimeVal::Int(index), payload],
        runtime.heap_mut(),
    )
}

fn format_variadic_runtime(args: &[RuntimeVal], heap: &HeapStore) -> Result<String> {
    if args.is_empty() {
        return Ok(String::new());
    }
    let Some(format) = runtime_string_maybe(&args[0], heap)? else {
        return join_runtime_display(args, heap);
    };
    let rest = &args[1..];
    let mut out = String::with_capacity(format.len() + rest.len() * 8);
    let chars: Vec<char> = format.chars().collect();
    let mut index = 0usize;
    let mut arg_index = 0usize;
    while index < chars.len() {
        if chars[index] == '{' && index + 1 < chars.len() && chars[index + 1] == '}' {
            if let Some(value) = rest.get(arg_index) {
                out.push_str(&runtime_display(value, heap)?);
                arg_index += 1;
            } else {
                out.push_str("{}");
            }
            index += 2;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    if arg_index < rest.len() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&join_runtime_display(&rest[arg_index..], heap)?);
    }
    Ok(out)
}

fn join_runtime_display(args: &[RuntimeVal], heap: &HeapStore) -> Result<String> {
    let mut out = String::new();
    for (index, value) in args.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&runtime_display(value, heap)?);
    }
    Ok(out)
}

fn runtime_display(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    Ok(runtime_val_to_val(value, heap)?.to_string())
}

fn runtime_string(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<str>> {
    runtime_string_maybe(value, heap)?.ok_or_else(|| anyhow!("{context} must be a string"))
}

fn runtime_string_maybe(value: &RuntimeVal, heap: &HeapStore) -> Result<Option<Arc<str>>> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(Some(Arc::<str>::from(value.as_str()))),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(Some(value.clone())),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

fn runtime_callable_arg(
    value: &RuntimeVal,
    runtime: &NativeRuntime32<'_>,
    context: &str,
) -> Result<lk_core::vm::RuntimeCallable32> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{context} must be a runtime callable"));
    };
    let callable = runtime
        .state
        .heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match callable {
        HeapValue::Callable(CallableValue::Runtime32(function)) => Ok(function.as_ref().clone()),
        HeapValue::Callable(CallableValue::Closure { .. }) => {
            let module = runtime
                .module
                .as_ref()
                .ok_or_else(|| anyhow!("{context} requires Module32 execution context"))?;
            runtime_value_to_callable32(
                value,
                &runtime.state.heap,
                &runtime.state.globals,
                Arc::new((*module).clone()),
            )
            .ok_or_else(|| anyhow!("{context} closure could not be materialized"))
        }
        _ => Err(anyhow!("{context} must be a runtime callable")),
    }
}

fn channel_id_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<u64> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{context} must be a Channel"));
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Channel(channel) => Ok(channel.id),
        other => Err(anyhow!("{context} must be a Channel, got {}", other.type_name())),
    }
}

fn list_items(value: &RuntimeVal, heap: &mut HeapStore, context: &str) -> Result<Vec<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{context} must be a List"));
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        .clone();
    match value {
        HeapValue::List(list) => Ok(list.materialize_mixed(heap)),
        other => Err(anyhow!("{context} must be a List, got {}", other.type_name())),
    }
}

fn runtime_list(values: Vec<RuntimeVal>, heap: &mut HeapStore) -> Result<RuntimeVal> {
    Ok(RuntimeVal::Obj(
        heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))),
    ))
}

fn expect_runtime_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(anyhow!(
            "{name}() expects exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        ))
    }
}

fn runtime_type_name(value: &RuntimeVal, heap: &HeapStore) -> &'static str {
    match value {
        RuntimeVal::Nil => "Nil",
        RuntimeVal::Bool(_) => "Bool",
        RuntimeVal::Int(_) => "Int",
        RuntimeVal::Float(_) => "Float",
        RuntimeVal::ShortStr(_) => "String",
        RuntimeVal::Obj(handle) => heap.get(*handle).map(HeapValue::type_name).unwrap_or("Obj"),
    }
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
