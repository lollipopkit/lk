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
    rt::{self, RuntimePayload},
    val,
    val::{CallableValue, ChannelValue, HeapRef, HeapStore, HeapValue, RuntimeVal, TaskValue, Type, TypedList},
    vm::{
        NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, call_runtime_callable32_runtime,
        call_runtime_value32_runtime_with_receiver,
    },
};
use std::sync::Arc;

use runtime_native::runtime_display_value;

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
    register_runtime_builtin_full_state(registry, "print", print32, NativeEntry32::VARIADIC);
    register_runtime_builtin_full_state(registry, "println", println32, NativeEntry32::VARIADIC);
    register_runtime_builtin_full_state(registry, "panic", panic32, NativeEntry32::VARIADIC);
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
    registry.register_runtime_builtin(name, NativeFunction32::Plain(function), arity);
}

fn register_runtime_builtin_full_state(
    registry: &mut ModuleRegistry,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    registry.register_runtime_builtin(name, NativeFunction32::FullState(function), arity);
}

fn print32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    print!("{}", format_variadic_runtime(args.as_slice(), runtime)?);
    Ok(RuntimeVal::Nil)
}

fn println32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    println!("{}", format_variadic_runtime(args.as_slice(), runtime)?);
    Ok(RuntimeVal::Nil)
}

fn panic32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let mut msg = if args.is_empty() {
        "panic".to_string()
    } else {
        join_runtime_display(args.as_slice(), runtime)?
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
        .ctx()
        .map(lk_core::vm::VmContext::shallow_clone_shared_runtime)
        .unwrap_or_else(lk_core::vm::VmContext::new_without_core_vm_builtins);

    let fut: core::pin::Pin<Box<dyn core::future::Future<Output = Result<RuntimePayload>> + Send>> =
        Box::pin(async move {
            let mut heap = HeapStore::new();
            let result = call_runtime_callable32_runtime(function.as_ref(), &[], &mut heap, Some(&mut ctx))?;
            Ok(RuntimePayload::new(result, heap))
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
                runtime_type_name(other, runtime.heap())
            ));
        }
    };
    let inner_type = if values.len() == 2 {
        match &values[1] {
            RuntimeVal::Nil => val::Type::Nil,
            value => {
                let text = runtime_string(value, runtime.heap(), "chan() type")?;
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
    let channel_id = channel_id_arg(&values[0], runtime.heap(), "send first argument")?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = rt::with_runtime(|runtime| runtime.block_on(runtime.send_async(channel_id, value)))
        .map_err(|error| anyhow!("Send operation failed: {}", error))?;
    Ok(RuntimeVal::Bool(sent))
}

fn recv32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 1, "recv")?;
    let channel_id = channel_id_arg(
        args.get(0).expect("arity checked"),
        runtime.heap(),
        "recv first argument",
    )?;
    let (ok, value) = rt::with_runtime(|runtime| runtime.block_on(runtime.recv_async(channel_id)))
        .map_err(|error| anyhow!("Receive operation failed: {}", error))?;
    let value = value.into_value(runtime.heap_mut())?;
    runtime_list(vec![RuntimeVal::Bool(ok), value], runtime.heap_mut())
}

fn chan_try_send32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 2, "chan::try_send")?;
    let values = args.as_slice();
    let channel_id = channel_id_arg(&values[0], runtime.heap(), "chan::try_send first argument")?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = rt::with_runtime(|runtime| runtime.try_send(channel_id, value))
        .map_err(|error| anyhow!("Failed to send to channel: {}", error))?;
    Ok(RuntimeVal::Bool(sent))
}

fn chan_try_recv32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 1, "chan::try_recv")?;
    let channel_id = channel_id_arg(
        args.get(0).expect("arity checked"),
        runtime.heap(),
        "chan::try_recv first argument",
    )?;
    let payload = match rt::with_runtime(|runtime| runtime.try_recv(channel_id))? {
        Some((ok, value)) => vec![RuntimeVal::Bool(ok), value.into_value(runtime.heap_mut())?],
        None => vec![RuntimeVal::Bool(false), RuntimeVal::Nil],
    };
    runtime_list(payload, runtime.heap_mut())
}

fn select_block32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    use rt::SelectOperation;

    expect_runtime_arity(args, 5, "select$block")?;
    let args = args.as_slice();
    let types = list_handle_arg(&args[0], runtime.heap(), "select$block types")?;
    let channels = list_handle_arg(&args[1], runtime.heap(), "select$block channels")?;
    let values = list_handle_arg(&args[2], runtime.heap(), "select$block values")?;
    let guards = list_handle_arg(&args[3], runtime.heap(), "select$block guards")?;
    let RuntimeVal::Bool(has_default) = args[4] else {
        return Err(anyhow!("select$block: has_default must be a Bool"));
    };
    let len = typed_list_len(runtime.heap(), types, "select$block types")?;
    if typed_list_len(runtime.heap(), channels, "select$block channels")? != len
        || typed_list_len(runtime.heap(), values, "select$block values")? != len
        || typed_list_len(runtime.heap(), guards, "select$block guards")? != len
    {
        return Err(anyhow!("select$block: all lists must have equal length"));
    }

    let mut select = SelectOperation::new();
    for index in 0..len {
        if typed_list_bool_item(runtime.heap(), guards, index, "select$block guards")? != Some(true) {
            continue;
        }
        let kind = typed_list_int_item(runtime.heap(), types, index, "select$block types")?
            .ok_or_else(|| anyhow!("select$block: invalid arm entry types"))?;
        let channel = typed_list_item(runtime.heap_mut(), channels, index, "select$block channels")?
            .ok_or_else(|| anyhow!("select$block: missing channel arm"))?;
        let channel_id = channel_id_arg(&channel, runtime.heap(), "select$block channel")?;
        match kind {
            0 => select.add_recv(index, channel_id),
            1 => {
                let value = typed_list_item(runtime.heap_mut(), values, index, "select$block values")?
                    .ok_or_else(|| anyhow!("select$block: missing send value"))?;
                let value = RuntimePayload::copy_from_value(&value, runtime.heap())?;
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
            vec![RuntimeVal::Bool(ok), value.into_value(runtime.heap_mut())?],
            runtime.heap_mut(),
        )?,
        None => RuntimeVal::Nil,
    };
    runtime_list(
        vec![RuntimeVal::Bool(false), RuntimeVal::Int(index), payload],
        runtime.heap_mut(),
    )
}

fn format_variadic_runtime(args: &[RuntimeVal], runtime: &mut NativeRuntime32<'_>) -> Result<String> {
    if args.is_empty() {
        return Ok(String::new());
    }
    let Some(format) = runtime_string_maybe(&args[0], runtime.heap())? else {
        return join_runtime_display(args, runtime);
    };
    let rest = &args[1..];
    let mut out = String::with_capacity(format.len() + rest.len() * 8);
    let mut chars = format.chars().peekable();
    let mut arg_index = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if let Some(value) = rest.get(arg_index) {
                out.push_str(&runtime_display(value, runtime)?);
                arg_index += 1;
            } else {
                out.push_str("{}");
            }
        } else {
            out.push(ch);
        }
    }
    if arg_index < rest.len() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&join_runtime_display(&rest[arg_index..], runtime)?);
    }
    Ok(out)
}

fn join_runtime_display(args: &[RuntimeVal], runtime: &mut NativeRuntime32<'_>) -> Result<String> {
    let mut out = String::new();
    for (index, value) in args.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&runtime_display(value, runtime)?);
    }
    Ok(out)
}

fn runtime_display(value: &RuntimeVal, runtime: &mut NativeRuntime32<'_>) -> Result<String> {
    if let Some(value) = runtime_display_show(value, runtime)? {
        return Ok(value);
    }
    runtime_display_value(value, runtime.heap())
}

fn runtime_display_show(value: &RuntimeVal, runtime: &mut NativeRuntime32<'_>) -> Result<Option<String>> {
    let Some(receiver_type) = runtime_display_receiver_type(value, runtime.heap()) else {
        return Ok(None);
    };
    let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
        return Ok(None);
    };
    let Some(ctx) = ctx else {
        return Ok(None);
    };
    let Some(method) = ctx
        .type_checker()
        .as_ref()
        .and_then(|tc| tc.registry().get_method(&receiver_type, "show").cloned())
    else {
        return Ok(None);
    };
    let result = call_runtime_value32_runtime_with_receiver(method, value, &[], state, module, Some(ctx))?;
    runtime_string_maybe(&result, state.heap()).map(|value| value.map(|value| value.to_string()))
}

fn runtime_display_receiver_type(value: &RuntimeVal, heap: &HeapStore) -> Option<Type> {
    let RuntimeVal::Obj(handle) = value else {
        return None;
    };
    let Some(HeapValue::Object(object)) = heap.get(*handle) else {
        return None;
    };
    Some(Type::Named(object.type_name.to_string()))
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
) -> Result<Arc<lk_core::vm::RuntimeCallable32>> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{context} must be a runtime callable"));
    };
    let callable = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match callable {
        HeapValue::Callable(CallableValue::Runtime32(function)) => Ok(Arc::clone(function)),
        HeapValue::Callable(CallableValue::Closure { .. }) => {
            Err(anyhow!("{context} closure requires active RuntimeModuleState32"))
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

fn list_handle_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<HeapRef> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{context} must be a List"));
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(_) => Ok(*handle),
        other => Err(anyhow!("{context} must be a List, got {}", other.type_name())),
    }
}

fn typed_list_ref<'a>(heap: &'a HeapStore, handle: HeapRef, context: &str) -> Result<&'a TypedList> {
    match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(list) => Ok(list),
        other => Err(anyhow!("{context} must be a List, got {}", other.type_name())),
    }
}

fn typed_list_len(heap: &HeapStore, handle: HeapRef, context: &str) -> Result<usize> {
    Ok(typed_list_ref(heap, handle, context)?.len())
}

fn typed_list_int_item(heap: &HeapStore, handle: HeapRef, index: usize, context: &str) -> Result<Option<i64>> {
    let list = typed_list_ref(heap, handle, context)?;
    Ok(match list {
        TypedList::Mixed(values) => match values.get(index) {
            Some(RuntimeVal::Int(value)) => Some(*value),
            _ => None,
        },
        TypedList::Int(values) => values.get(index).copied(),
        _ => None,
    })
}

fn typed_list_bool_item(heap: &HeapStore, handle: HeapRef, index: usize, context: &str) -> Result<Option<bool>> {
    let list = typed_list_ref(heap, handle, context)?;
    Ok(match list {
        TypedList::Mixed(values) => match values.get(index) {
            Some(RuntimeVal::Bool(value)) => Some(*value),
            _ => None,
        },
        TypedList::Bool(values) => values.get(index).copied(),
        _ => None,
    })
}

fn typed_list_item(heap: &mut HeapStore, handle: HeapRef, index: usize, context: &str) -> Result<Option<RuntimeVal>> {
    enum Item {
        Value(RuntimeVal),
        String(Arc<str>),
    }

    let item = {
        let list = typed_list_ref(heap, handle, context)?;
        match list {
            TypedList::Mixed(values) => values.get(index).cloned().map(Item::Value),
            TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int).map(Item::Value),
            TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float).map(Item::Value),
            TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool).map(Item::Value),
            TypedList::String(values) => values.get(index).cloned().map(Item::String),
        }
    };
    Ok(match item {
        Some(Item::Value(value)) => Some(value),
        Some(Item::String(value)) => {
            if let Some(short) = val::ShortStr::new(&value) {
                Some(RuntimeVal::ShortStr(short))
            } else {
                Some(RuntimeVal::Obj(heap.alloc(HeapValue::String(value))))
            }
        }
        None => None,
    })
}

fn runtime_list(values: Vec<RuntimeVal>, heap: &mut HeapStore) -> Result<RuntimeVal> {
    Ok(RuntimeVal::Obj(
        heap.alloc(HeapValue::List(typed_list_from_values(values, heap))),
    ))
}

pub(crate) fn typed_list_from_values(values: Vec<RuntimeVal>, heap: &HeapStore) -> TypedList {
    if values.is_empty() {
        return TypedList::Mixed(values);
    }

    let mut ints = Vec::with_capacity(values.len());
    let mut floats = Vec::with_capacity(values.len());
    let mut bools = Vec::with_capacity(values.len());
    let mut strings = Vec::with_capacity(values.len());
    for value in &values {
        match value {
            RuntimeVal::Int(value) if floats.is_empty() && bools.is_empty() && strings.is_empty() => {
                ints.push(*value);
            }
            RuntimeVal::Float(value) if ints.is_empty() && bools.is_empty() && strings.is_empty() => {
                floats.push(*value);
            }
            RuntimeVal::Bool(value) if ints.is_empty() && floats.is_empty() && strings.is_empty() => {
                bools.push(*value);
            }
            RuntimeVal::ShortStr(value) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                strings.push(Arc::<str>::from(value.as_str()));
            }
            RuntimeVal::Obj(handle) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                let Some(HeapValue::String(value)) = heap.get(*handle) else {
                    return TypedList::Mixed(values);
                };
                strings.push(value.clone());
            }
            _ => return TypedList::Mixed(values),
        }
    }

    if !ints.is_empty() {
        TypedList::Int(ints)
    } else if !floats.is_empty() {
        TypedList::Float(floats)
    } else if !bools.is_empty() {
        TypedList::Bool(bools)
    } else {
        TypedList::String(strings)
    }
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

/// Returns a mapping of stdlib module names to their .lk source file paths.
/// These are LK-language stdlib modules that complement the Rust-native ones.
pub fn stdlib_lk_modules() -> Vec<(&'static str, &'static str)> {
    vec![
        ("alg", "alg"),
        ("collections", "collections"),
        ("func", "func"),
        ("assert", "assert"),
        ("assert_", "assert"),
        ("math_ext", "math_ext"),
    ]
}

/// Register LK-source stdlib modules on a resolver.
/// Must be called after Rust-native stdlib modules are registered
/// (native modules take priority).
pub fn register_stdlib_lk_modules(resolver: &mut lk_core::stmt::ModuleResolver) -> Result<()> {
    let lk_dir = lk_dir_path();
    for (name, sub) in stdlib_lk_modules() {
        // Only register if no Rust-native module with this name exists
        if resolver.resolve_runtime_module(name).is_err() {
            let mod_path = lk_dir.join(sub).join("mod.lk");
            if mod_path.exists() {
                resolver.register_package_module(name, mod_path);
            }
        }
    }
    Ok(())
}

/// Return the directory containing the .lk stdlib source files.
fn lk_dir_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lk")
}

#[cfg(test)]
mod runtime_registration_tests {
    use super::*;
    use lk_core::{val::Type, vm::RuntimeModuleState32};

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

        assert!(registry.get_runtime_builtin("println").is_some());
        assert!(registry.get_runtime_builtin("spawn").is_none());
        assert!(registry.get_runtime_builtin("select$block").is_none());
    }

    #[test]
    fn select_block_reads_typed_control_lists_without_materializing_inactive_values() -> Result<()> {
        let mut state = RuntimeModuleState32::default();
        let channel_id = rt::with_runtime(|runtime| runtime.create_channel(Some(1)))?;
        let channel = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Channel(Arc::new(ChannelValue {
            id: channel_id,
            capacity: Some(1),
            inner_type: Type::Nil,
        }))));
        let types = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(TypedList::Int(vec![1]))));
        let channels = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(TypedList::Mixed(vec![channel]))));
        let values =
            RuntimeVal::Obj(
                state
                    .heap_mut()
                    .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                        "long-select-send-value",
                    )]))),
            );
        let guards = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(TypedList::Bool(vec![false]))));
        let args = [types, channels, values, guards, RuntimeVal::Bool(true)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = select_block32(NativeArgs32::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("select$block should return list object");
        };
        assert!(matches!(runtime.heap().get(handle), Some(HeapValue::List(_))));
        assert_eq!(runtime.heap().len(), 6);
        Ok(())
    }
}
