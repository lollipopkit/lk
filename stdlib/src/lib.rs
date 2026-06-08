pub use lk_stdlib_bytes as bytes;
pub use lk_stdlib_chan as concurrency_chan;
pub use lk_stdlib_datetime as datetime;
pub use lk_stdlib_io as io;
pub use lk_stdlib_iter as iter;
pub use lk_stdlib_json as json;
pub use lk_stdlib_math as math;
pub use lk_stdlib_net as net;
pub use lk_stdlib_os as os;
pub use lk_stdlib_slice as slice;
pub use lk_stdlib_stream as stream;
pub use lk_stdlib_string as string;
pub use lk_stdlib_task as concurrency_task;
pub use lk_stdlib_time as time;
pub use lk_stdlib_toml as toml;
pub use lk_stdlib_yaml as yaml;
mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[cfg(test)]
mod bytes_test;
#[cfg(test)]
mod datetime_test;
#[cfg(test)]
mod globals_test;
#[cfg(test)]
mod math_test;
#[cfg(test)]
mod os_test;
#[cfg(test)]
mod stdlib_runtime_test;
#[cfg(test)]
mod stream_test;
#[cfg(test)]
mod string_test;

use anyhow::{Result, anyhow};
use lk_core::{
    module::ModuleRegistry,
    rt::{self, RuntimePayload},
    val,
    val::{
        CallableValue, ChannelValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, TaskValue,
        Type, TypedList, TypedMap,
    },
    vm::{
        NativeArgs, NativeEntry, NativeFunction, NativeRuntime, call_runtime_callable_runtime,
        call_runtime_value_runtime_with_receiver,
    },
};
use std::sync::Arc;

use runtime_native::runtime_display_value;

/// Register all stdlib modules with the given registry
pub fn register_stdlib_modules(registry: &mut ModuleRegistry) -> Result<()> {
    for name in [
        "io", "json", "yaml", "toml", "bytes", "iter", "math", "string", "datetime", "os", "net", "slice", "stream",
        "task", "chan", "time",
    ] {
        register_stdlib_module_by_name(registry, name)?;
    }
    Ok(())
}

/// Register a selected subset of stdlib modules. Unknown names are ignored so
/// package modules can share the same use collection path and resolve later.
pub fn register_stdlib_modules_named(registry: &mut ModuleRegistry, names: &[String]) -> Result<()> {
    for name in names {
        register_stdlib_module_by_name(registry, name)?;
    }
    Ok(())
}

fn register_stdlib_module_by_name(registry: &mut ModuleRegistry, name: &str) -> Result<()> {
    match name {
        "io" => io::register(registry)?,
        "json" => json::register(registry)?,
        "yaml" => yaml::register(registry)?,
        "toml" => toml::register(registry)?,
        "bytes" => bytes::register(registry)?,
        "iter" => iter::register(registry)?,
        "math" => math::register(registry)?,
        "string" => string::register(registry)?,
        "datetime" => datetime::register(registry)?,
        "os" => os::register(registry)?,
        "net" => net::register(registry)?,
        "slice" => slice::register(registry)?,
        "stream" => stream::register(registry)?,
        "task" => concurrency_task::register(registry)?,
        "chan" => concurrency_chan::register(registry)?,
        "time" => time::register(registry)?,
        _ => {}
    }
    Ok(())
}

pub fn register_stdlib_module_io(registry: &mut ModuleRegistry) -> Result<()> {
    io::register(registry)
}

pub fn register_stdlib_module_json(registry: &mut ModuleRegistry) -> Result<()> {
    json::register(registry)
}

pub fn register_stdlib_module_yaml(registry: &mut ModuleRegistry) -> Result<()> {
    yaml::register(registry)
}

pub fn register_stdlib_module_toml(registry: &mut ModuleRegistry) -> Result<()> {
    toml::register(registry)
}

pub fn register_stdlib_module_bytes(registry: &mut ModuleRegistry) -> Result<()> {
    bytes::register(registry)
}

pub fn register_stdlib_module_iter(registry: &mut ModuleRegistry) -> Result<()> {
    iter::register(registry)
}

pub fn register_stdlib_module_math(registry: &mut ModuleRegistry) -> Result<()> {
    math::register(registry)
}

pub fn register_stdlib_module_string(registry: &mut ModuleRegistry) -> Result<()> {
    string::register(registry)
}

pub fn register_stdlib_module_datetime(registry: &mut ModuleRegistry) -> Result<()> {
    datetime::register(registry)
}

pub fn register_stdlib_module_os(registry: &mut ModuleRegistry) -> Result<()> {
    os::register(registry)
}

pub fn register_stdlib_module_net(registry: &mut ModuleRegistry) -> Result<()> {
    net::register(registry)
}

pub fn register_stdlib_module_stream(registry: &mut ModuleRegistry) -> Result<()> {
    stream::register(registry)
}

pub fn register_stdlib_module_task(registry: &mut ModuleRegistry) -> Result<()> {
    concurrency_task::register(registry)
}

pub fn register_stdlib_module_chan(registry: &mut ModuleRegistry) -> Result<()> {
    concurrency_chan::register(registry)
}

pub fn register_stdlib_module_time(registry: &mut ModuleRegistry) -> Result<()> {
    time::register(registry)
}

/// Register global builtin functions available without use
/// - print(fmt, ...args): print formatted text without newline; returns nil
/// - println(fmt, ...args): print formatted text with newline; returns nil
/// - panic([msg]): raise a runtime error with optional message and backtrace
/// - assert(cond[, msg]): panic unless cond is truthy
/// - assert_eq(actual, expected[, msg]): panic unless values are equal
/// - assert_ne(actual, expected[, msg]): panic unless values are not equal
pub fn register_stdlib_core_globals(registry: &mut ModuleRegistry) {
    register_runtime_builtin_full_state(registry, "print", print, NativeEntry::VARIADIC);
    register_runtime_builtin_full_state(registry, "println", println, NativeEntry::VARIADIC);
    register_runtime_builtin_full_state(registry, "panic", panic, NativeEntry::VARIADIC);
    register_runtime_builtin_full_state(registry, "assert", assert, NativeEntry::VARIADIC);
    register_runtime_builtin_full_state(registry, "assert_eq", assert_eq, NativeEntry::VARIADIC);
    register_runtime_builtin_full_state(registry, "assert_ne", assert_ne, NativeEntry::VARIADIC);
}

pub fn register_stdlib_concurrency_globals(registry: &mut ModuleRegistry) {
    register_runtime_builtin(registry, "spawn", spawn, 1);
    register_runtime_builtin(registry, "chan", chan, NativeEntry::VARIADIC);
    register_runtime_builtin(registry, "send", send, 2);
    register_runtime_builtin(registry, "recv", recv, 1);
    register_runtime_builtin(registry, "chan::try_send", chan_try_send, 2);
    register_runtime_builtin(registry, "chan::try_recv", chan_try_recv, 1);
    register_runtime_builtin(registry, "select$block", select_block, 5);
}

fn register_runtime_builtin(
    registry: &mut ModuleRegistry,
    name: &str,
    function: fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    registry.register_runtime_builtin(name, NativeFunction::Plain(function), arity);
}

fn register_runtime_builtin_full_state(
    registry: &mut ModuleRegistry,
    name: &str,
    function: fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    registry.register_runtime_builtin(name, NativeFunction::FullState(function), arity);
}

fn print(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    print!("{}", format_variadic_runtime(args.as_slice(), runtime)?);
    Ok(RuntimeVal::Nil)
}

fn println(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    println!("{}", format_variadic_runtime(args.as_slice(), runtime)?);
    Ok(RuntimeVal::Nil)
}

fn panic(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn assert(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, 1, 2, "assert")?;
    let values = args.as_slice();
    if assert_truthy(&values[0]) {
        return Ok(RuntimeVal::Nil);
    }
    let message = if let Some(message) = values.get(1) {
        format!("assertion failed: {}", runtime_display(message, runtime)?)
    } else {
        "assertion failed".to_string()
    };
    panic_runtime_message(message);
}

fn assert_eq(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, 2, 3, "assert_eq")?;
    let values = args.as_slice();
    if runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let actual = runtime_display(&values[0], runtime)?;
    let expected = runtime_display(&values[1], runtime)?;
    let mut message = format!("assertion failed: expected {expected}, got {actual}");
    if let Some(extra) = values.get(2) {
        message.push_str(" - ");
        message.push_str(&runtime_display(extra, runtime)?);
    }
    panic_runtime_message(message);
}

fn assert_ne(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, 2, 3, "assert_ne")?;
    let values = args.as_slice();
    if !runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let mut message = "assertion failed: values should not be equal".to_string();
    if let Some(extra) = values.get(2) {
        message.push_str(" - ");
        message.push_str(&runtime_display(extra, runtime)?);
    }
    panic_runtime_message(message);
}

fn expect_assert_args(args: NativeArgs<'_>, min: usize, max: usize, name: &str) -> Result<()> {
    if args.has_named() {
        return Err(anyhow!("{name}() does not accept named arguments"));
    }
    let len = args.len();
    if (min..=max).contains(&len) {
        Ok(())
    } else if min == max {
        Err(anyhow!("{name}() expects exactly {min} arguments"))
    } else {
        Err(anyhow!("{name}() expects {min} or {max} arguments"))
    }
}

fn assert_truthy(value: &RuntimeVal) -> bool {
    !matches!(value, RuntimeVal::Nil | RuntimeVal::Bool(false))
}

fn panic_runtime_message(mut message: String) -> ! {
    let bt = std::backtrace::Backtrace::force_capture();
    message.push_str("\nBacktrace:\n");
    message.push_str(&format!("{}", bt));
    panic!("{}", message);
}

fn spawn(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 1, "spawn")?;
    let function = runtime_callable_arg(args.get(0).expect("arity checked"), runtime, "spawn argument")?;
    let mut ctx = runtime
        .ctx()
        .map(lk_core::vm::VmContext::shallow_clone_shared_runtime)
        .unwrap_or_else(lk_core::vm::VmContext::new_without_core_vm_builtins);

    let fut: core::pin::Pin<Box<dyn core::future::Future<Output = Result<RuntimePayload>> + Send>> =
        Box::pin(async move {
            let mut heap = HeapStore::new();
            let result = call_runtime_callable_runtime(function.as_ref(), &[], &mut heap, Some(&mut ctx))?;
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

fn chan(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 2, "send")?;
    let values = args.as_slice();
    let channel_id = channel_id_arg(&values[0], runtime.heap(), "send first argument")?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = rt::with_runtime(|runtime| runtime.block_on(runtime.send_async(channel_id, value)))
        .map_err(|error| anyhow!("Send operation failed: {}", error))?;
    Ok(RuntimeVal::Bool(sent))
}

fn recv(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn chan_try_send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 2, "chan::try_send")?;
    let values = args.as_slice();
    let channel_id = channel_id_arg(&values[0], runtime.heap(), "chan::try_send first argument")?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = rt::with_runtime(|runtime| runtime.try_send(channel_id, value))
        .map_err(|error| anyhow!("Failed to send to channel: {}", error))?;
    Ok(RuntimeVal::Bool(sent))
}

fn chan_try_recv(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn select_block(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn format_variadic_runtime(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<String> {
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

fn join_runtime_display(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<String> {
    let mut out = String::new();
    for (index, value) in args.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&runtime_display(value, runtime)?);
    }
    Ok(out)
}

fn runtime_display(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<String> {
    if let Some(value) = runtime_display_show(value, runtime)? {
        return Ok(value);
    }
    runtime_display_value(value, runtime.heap())
}

fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> Result<bool> {
    Ok(match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left == right,
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => *left as f64 == *right,
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => *left == *right as f64,
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) if left == right => true,
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) => {
            let left = heap
                .get(*left)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", left.index()))?;
            let right = heap
                .get(*right)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", right.index()))?;
            heap_values_equal(left, right, heap)?
        }
        _ => match (
            runtime_value_to_string(left, heap)?,
            runtime_value_to_string(right, heap)?,
        ) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
    })
}

fn heap_values_equal(left: &HeapValue, right: &HeapValue, heap: &HeapStore) -> Result<bool> {
    Ok(match (left, right) {
        (HeapValue::String(left), HeapValue::String(right)) => left == right,
        (HeapValue::List(left), HeapValue::List(right)) => typed_lists_equal(left, right, heap)?,
        (HeapValue::Map(left), HeapValue::Map(right)) => typed_maps_equal(left, right, heap)?,
        (HeapValue::Set(left), HeapValue::Set(right)) => runtime_sets_equal(left, right),
        _ => false,
    })
}

fn runtime_sets_equal(left: &RuntimeSet, right: &RuntimeSet) -> bool {
    left.len() == right.len() && left.entries().all(|key| right.contains(key))
}

fn runtime_value_to_string(value: &RuntimeVal, heap: &HeapStore) -> Result<Option<Arc<str>>> {
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

fn typed_lists_equal(left: &TypedList, right: &TypedList, heap: &HeapStore) -> Result<bool> {
    if left.len() != right.len() {
        return Ok(false);
    }
    match (left, right) {
        (TypedList::Int(left), TypedList::Int(right)) => return Ok(left == right),
        (TypedList::Float(left), TypedList::Float(right)) => return Ok(left == right),
        (TypedList::Bool(left), TypedList::Bool(right)) => return Ok(left == right),
        (TypedList::String(left), TypedList::String(right)) => return Ok(left == right),
        _ => {}
    }
    for index in 0..left.len() {
        if !typed_list_items_equal(left, index, right, index, heap)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn typed_list_items_equal(
    left: &TypedList,
    left_index: usize,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> Result<bool> {
    match (left, right) {
        (TypedList::Mixed(left), TypedList::Mixed(right)) => {
            runtime_values_equal(&left[left_index], &right[right_index], heap)
        }
        (TypedList::Mixed(left), TypedList::String(right)) => {
            runtime_value_equals_string(&left[left_index], &right[right_index], heap)
        }
        (TypedList::String(left), TypedList::Mixed(right)) => {
            runtime_value_equals_string(&right[right_index], &left[left_index], heap)
        }
        (TypedList::Int(left), _) => {
            typed_list_runtime_item_equal(RuntimeVal::Int(left[left_index]), right, right_index, heap)
        }
        (TypedList::Float(left), _) => {
            typed_list_runtime_item_equal(RuntimeVal::Float(left[left_index]), right, right_index, heap)
        }
        (TypedList::Bool(left), _) => {
            typed_list_runtime_item_equal(RuntimeVal::Bool(left[left_index]), right, right_index, heap)
        }
        (TypedList::String(left), _) => typed_list_string_item_equal(&left[left_index], right, right_index, heap),
        (TypedList::Mixed(left), _) => {
            typed_list_runtime_item_equal(left[left_index].clone(), right, right_index, heap)
        }
    }
}

fn typed_list_runtime_item_equal(
    value: RuntimeVal,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> Result<bool> {
    match right {
        TypedList::Mixed(right) => runtime_values_equal(&value, &right[right_index], heap),
        TypedList::Int(right) => runtime_values_equal(&value, &RuntimeVal::Int(right[right_index]), heap),
        TypedList::Float(right) => runtime_values_equal(&value, &RuntimeVal::Float(right[right_index]), heap),
        TypedList::Bool(right) => runtime_values_equal(&value, &RuntimeVal::Bool(right[right_index]), heap),
        TypedList::String(right) => runtime_value_equals_string(&value, &right[right_index], heap),
    }
}

fn typed_list_string_item_equal(
    left: &Arc<str>,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> Result<bool> {
    match right {
        TypedList::Mixed(right) => runtime_value_equals_string(&right[right_index], left, heap),
        TypedList::String(right) => Ok(left == &right[right_index]),
        _ => Ok(false),
    }
}

fn runtime_value_equals_string(value: &RuntimeVal, expected: &str, heap: &HeapStore) -> Result<bool> {
    Ok(match value {
        RuntimeVal::ShortStr(value) => value.as_str() == expected,
        RuntimeVal::Obj(handle) => matches!(
            heap.get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            HeapValue::String(value) if value.as_ref() == expected
        ),
        _ => false,
    })
}

fn typed_maps_equal(left: &TypedMap, right: &TypedMap, heap: &HeapStore) -> Result<bool> {
    if left.len() != right.len() {
        return Ok(false);
    }
    match left {
        TypedMap::Mixed(entries) => {
            for (key, value) in entries {
                if !typed_map_value_equal(right, key, value, heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringMixed(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, value, heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringInt(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, &RuntimeVal::Int(*value), heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringFloat(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, &RuntimeVal::Float(*value), heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringBool(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, &RuntimeVal::Bool(*value), heap)? {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn typed_map_value_equal(
    right: &TypedMap,
    key: &RuntimeMapKey,
    left_value: &RuntimeVal,
    heap: &HeapStore,
) -> Result<bool> {
    let Some(right_value) = right.get(key) else {
        return Ok(false);
    };
    runtime_values_equal(left_value, &right_value, heap)
}

fn runtime_display_show(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<Option<String>> {
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
    let result = call_runtime_value_runtime_with_receiver(method, value, &[], state, module, Some(ctx))?;
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
    runtime: &NativeRuntime<'_>,
    context: &str,
) -> Result<Arc<lk_core::vm::RuntimeCallable>> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{context} must be a runtime callable"));
    };
    let callable = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match callable {
        HeapValue::Callable(CallableValue::Runtime(function)) => Ok(Arc::clone(function)),
        HeapValue::Callable(CallableValue::Closure { .. }) => {
            Err(anyhow!("{context} closure requires active RuntimeModuleState"))
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

fn expect_runtime_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
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

#[cfg(test)]
mod runtime_registration_tests {
    use super::*;
    use lk_core::{val::Type, vm::RuntimeModuleState};

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
        let mut state = RuntimeModuleState::default();
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
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = select_block(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("select$block should return list object");
        };
        assert!(matches!(runtime.heap().get(handle), Some(HeapValue::List(_))));
        assert_eq!(runtime.heap().len(), 6);
        Ok(())
    }
}
