pub use lk_stdlib_bytes as bytes;
pub use lk_stdlib_chan as concurrency_chan;
pub use lk_stdlib_datetime as datetime;
pub use lk_stdlib_encoding as encoding;
pub use lk_stdlib_env as env;
pub use lk_stdlib_fs as fs;
pub use lk_stdlib_hash as hash;
pub use lk_stdlib_http as http;
pub use lk_stdlib_io as io;
pub use lk_stdlib_iter as iter;
pub use lk_stdlib_math as math;
pub use lk_stdlib_net as net;
pub use lk_stdlib_os as os;
pub use lk_stdlib_path as path;
pub use lk_stdlib_process as process;
pub use lk_stdlib_random as random;
pub use lk_stdlib_regex as regex;
pub use lk_stdlib_slice as slice;
pub use lk_stdlib_stream as stream;
pub use lk_stdlib_string as string;
pub use lk_stdlib_task as concurrency_task;
pub use lk_stdlib_time as time;
pub use lk_stdlib_uuid as uuid;
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
mod stdlib_modules_test;
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
        call_runtime_value_runtime, call_runtime_value_runtime_with_receiver, coroutine_status_runtime,
        create_coroutine_runtime, resume_coroutine_runtime,
    },
};
pub use lk_stdlib_common::metadata::{
    StdlibArity, StdlibCallableMetadata, StdlibCatalog, StdlibConstValue, StdlibExportKind, StdlibExportSpec,
    StdlibGlobalMetadata, StdlibGlobalSpec, StdlibModuleMetadata, StdlibModuleSpec, StdlibReturnKind,
    register_stdlib_global_metadata, register_stdlib_module_metadata, registered_stdlib_export_metadata,
    registered_stdlib_global_metadata, registered_stdlib_module_metadata,
};
use std::sync::{Arc, OnceLock};

use runtime_native::{runtime_display_value, runtime_string_value};

static STDLIB_CATALOG: OnceLock<StdlibCatalog> = OnceLock::new();

struct StdlibModuleEntry {
    name: &'static str,
    register: fn(&mut ModuleRegistry) -> Result<()>,
}

macro_rules! define_stdlib_modules {
    ($($name:literal => $register:path as $wrapper:ident),+ $(,)?) => {
        const STDLIB_MODULES: &[StdlibModuleEntry] = &[
            $(
                StdlibModuleEntry {
                    name: $name,
                    register: $register,
                },
            )+
        ];

        $(
            pub fn $wrapper(registry: &mut ModuleRegistry) -> Result<()> {
                $register(registry)
            }
        )+
    };
}

macro_rules! register_full_state_builtin {
    ($registry:expr, $name:ident => $function:ident / $arity:expr => $first:ident $(. $rest:ident)* : $return_kind:ident) => {
        register_runtime_builtin_full_state(
            $registry,
            stringify!($name),
            $function,
            $arity,
            Some(lk_stdlib_common::stdlib_global_metadata!(
                $name => $first $(. $rest)* : $return_kind
            )),
        );
    };
}

macro_rules! register_plain_builtin {
    ($registry:expr, $name:ident => $function:ident / $arity:expr) => {
        register_runtime_builtin($registry, stringify!($name), $function, $arity, None);
    };
    ($registry:expr, $name:ident => $function:ident / $arity:expr => $first:ident $(. $rest:ident)* : $return_kind:ident) => {
        register_runtime_builtin(
            $registry,
            stringify!($name),
            $function,
            $arity,
            Some(lk_stdlib_common::stdlib_global_metadata!(
                $name => $first $(. $rest)* : $return_kind
            )),
        );
    };
    ($registry:expr, $name:literal => $function:ident / $arity:expr) => {
        register_runtime_builtin($registry, $name, $function, $arity, None);
    };
}

define_stdlib_modules!(
    "io" => io::register as register_stdlib_module_io,
    "encoding" => encoding::register as register_stdlib_module_encoding,
    "bytes" => bytes::register as register_stdlib_module_bytes,
    "iter" => iter::register as register_stdlib_module_iter,
    "math" => math::register as register_stdlib_module_math,
    "string" => string::register as register_stdlib_module_string,
    "datetime" => datetime::register as register_stdlib_module_datetime,
    "os" => os::register as register_stdlib_module_os,
    "fs" => fs::register as register_stdlib_module_fs,
    "path" => path::register as register_stdlib_module_path,
    "env" => env::register as register_stdlib_module_env,
    "process" => process::register as register_stdlib_module_process,
    "hash" => hash::register as register_stdlib_module_hash,
    "regex" => regex::register as register_stdlib_module_regex,
    "random" => random::register as register_stdlib_module_random,
    "uuid" => uuid::register as register_stdlib_module_uuid,
    "http" => http::register as register_stdlib_module_http,
    "net" => net::register as register_stdlib_module_net,
    "slice" => slice::register as register_stdlib_module_slice,
    "stream" => stream::register as register_stdlib_module_stream,
    "task" => concurrency_task::register as register_stdlib_module_task,
    "chan" => concurrency_chan::register as register_stdlib_module_chan,
    "time" => time::register as register_stdlib_module_time,
);

pub fn stdlib_catalog() -> &'static StdlibCatalog {
    STDLIB_CATALOG.get_or_init(build_stdlib_catalog)
}

/// Register all stdlib modules with the given registry
pub fn register_stdlib_modules(registry: &mut ModuleRegistry) -> Result<()> {
    for entry in STDLIB_MODULES {
        (entry.register)(registry)?;
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
    if let Some(entry) = STDLIB_MODULES.iter().find(|entry| entry.name == name) {
        (entry.register)(registry)?;
    }
    Ok(())
}

fn stdlib_module_names() -> impl Iterator<Item = &'static str> {
    STDLIB_MODULES.iter().map(|entry| entry.name)
}

fn build_stdlib_catalog() -> StdlibCatalog {
    let mut registry = ModuleRegistry::new();
    register_stdlib_globals(&mut registry);
    register_stdlib_modules(&mut registry).expect("stdlib module registration should not fail");

    let mut modules = Vec::new();
    for name in stdlib_module_names() {
        let Ok(export) = registry.get_runtime_module(name) else {
            continue;
        };
        let exports = export
            .state_lock()
            .ok()
            .and_then(|state| catalog_exports_from_runtime(name, export.value(), state.heap()))
            .unwrap_or_default();
        let display = catalog_module_display(&exports);
        modules.push(StdlibModuleSpec {
            name: name.to_string(),
            detail: "stdlib module".to_string(),
            display,
            docs: registered_stdlib_module_metadata(name).and_then(|metadata| metadata.docs.map(str::to_string)),
            exports,
        });
    }
    modules.sort_by(|left, right| left.name.cmp(&right.name));

    let mut globals: Vec<_> = registry
        .get_all_runtime_builtins()
        .iter()
        .filter(|(name, _)| !name.contains("::") && !name.contains('$'))
        .filter_map(|(name, export)| {
            let arity = catalog_global_arity(export)?;
            let metadata = registered_stdlib_global_metadata(name);
            Some(StdlibGlobalSpec {
                name: name.to_string(),
                arity,
                detail: catalog_function_detail(name, arity),
                lowering_key: metadata.map(|metadata| metadata.lowering_key),
                return_kind: metadata.map(|metadata| metadata.return_kind),
                signature: metadata.and_then(|metadata| metadata.signature.map(str::to_string)),
                docs: metadata.and_then(|metadata| metadata.docs.map(str::to_string)),
            })
        })
        .collect();
    globals.sort_by(|left, right| left.name.cmp(&right.name));

    StdlibCatalog { modules, globals }
}

fn catalog_exports_from_runtime(path: &str, value: &RuntimeVal, heap: &HeapStore) -> Option<Vec<StdlibExportSpec>> {
    let RuntimeVal::Obj(handle) = value else {
        return None;
    };
    let HeapValue::Map(map) = heap.get(*handle)? else {
        return None;
    };
    let mut exports = Vec::new();
    for (key, value) in map.entries_iter() {
        let Some(name) = key.as_str().map(ToString::to_string) else {
            continue;
        };
        let child_path = format!("{path}.{name}");
        exports.push(catalog_export_from_runtime(&child_path, name, &value, heap));
    }
    exports.sort_by(|left, right| left.name.cmp(&right.name));
    Some(exports)
}

fn catalog_export_from_runtime(path: &str, name: String, value: &RuntimeVal, heap: &HeapStore) -> StdlibExportSpec {
    match value {
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::Callable(CallableValue::RuntimeNative { name: _, arity, .. })) => {
                let arity = catalog_arity(*arity);
                let metadata = registered_stdlib_export_metadata(path);
                StdlibExportSpec {
                    name,
                    kind: StdlibExportKind::Function,
                    arity: Some(arity),
                    detail: catalog_function_detail(path, arity),
                    display: catalog_function_display(path.rsplit('.').next().unwrap_or(path), arity),
                    lowering_key: metadata.map(|metadata| metadata.lowering_key),
                    return_kind: metadata.map(|metadata| metadata.return_kind),
                    signature: metadata.and_then(|metadata| metadata.signature.map(str::to_string)),
                    docs: metadata.and_then(|metadata| metadata.docs.map(str::to_string)),
                    const_value: None,
                    children: Vec::new(),
                }
            }
            Some(HeapValue::Map(_)) => {
                let children = catalog_exports_from_runtime(path, value, heap).unwrap_or_default();
                let display = catalog_module_display(&children);
                StdlibExportSpec {
                    name,
                    kind: StdlibExportKind::Module,
                    arity: None,
                    detail: "stdlib namespace".to_string(),
                    display,
                    lowering_key: None,
                    return_kind: None,
                    signature: None,
                    docs: None,
                    const_value: None,
                    children,
                }
            }
            _ => catalog_value_export(path, name, value, heap),
        },
        _ => catalog_value_export(path, name, value, heap),
    }
}

fn catalog_value_export(path: &str, name: String, value: &RuntimeVal, heap: &HeapStore) -> StdlibExportSpec {
    let metadata = registered_stdlib_export_metadata(path);
    StdlibExportSpec {
        name,
        kind: StdlibExportKind::Value,
        arity: None,
        detail: "stdlib value".to_string(),
        display: runtime_display_value(value, heap).unwrap_or_else(|_| format!("<value {path}>")),
        lowering_key: metadata.map(|metadata| metadata.lowering_key),
        return_kind: metadata.map(|metadata| metadata.return_kind),
        signature: metadata.and_then(|metadata| metadata.signature.map(str::to_string)),
        docs: metadata.and_then(|metadata| metadata.docs.map(str::to_string)),
        const_value: catalog_const_value(value, heap),
        children: Vec::new(),
    }
}

fn catalog_module_display(exports: &[StdlibExportSpec]) -> String {
    format!(
        "{{{}}}",
        exports
            .iter()
            .map(|export| format!("{}: {}", export.name, export.display))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn catalog_arity(arity: u16) -> StdlibArity {
    if arity == NativeEntry::VARIADIC {
        StdlibArity::Variadic
    } else {
        StdlibArity::Fixed(arity)
    }
}

fn catalog_global_arity(export: &lk_core::vm::RuntimeExport) -> Option<StdlibArity> {
    let state = export.state_lock().ok()?;
    let RuntimeVal::Obj(handle) = export.value() else {
        return None;
    };
    let HeapValue::Callable(CallableValue::RuntimeNative { arity, .. }) = state.heap().get(*handle)? else {
        return None;
    };
    Some(catalog_arity(*arity))
}

fn catalog_function_detail(name: &str, arity: StdlibArity) -> String {
    match arity {
        StdlibArity::Fixed(value) => format!("{name}({value} args)"),
        StdlibArity::Variadic => format!("{name}(...)"),
    }
}

fn catalog_function_display(name: &str, arity: StdlibArity) -> String {
    match arity {
        StdlibArity::Fixed(value) => format!("<native fn {name}({value} args)>"),
        StdlibArity::Variadic => format!("<native fn {name}(...)>"),
    }
}

fn catalog_const_value(value: &RuntimeVal, heap: &HeapStore) -> Option<StdlibConstValue> {
    match value {
        RuntimeVal::Nil => Some(StdlibConstValue::Nil),
        RuntimeVal::Bool(value) => Some(StdlibConstValue::Bool(*value)),
        RuntimeVal::Int(value) => Some(StdlibConstValue::Int(*value)),
        RuntimeVal::Float(value) => Some(StdlibConstValue::Float(*value)),
        RuntimeVal::ShortStr(value) => Some(StdlibConstValue::String(value.as_str().to_string())),
        RuntimeVal::Obj(handle) => match heap.get(*handle)? {
            HeapValue::String(value) => Some(StdlibConstValue::String(value.to_string())),
            _ => None,
        },
    }
}

/// Register global builtin functions available without use
/// - print(fmt, ...args): print formatted text without newline; returns nil
/// - println(fmt, ...args): print formatted text with newline; returns nil
/// - panic([msg]): raise a runtime error with optional message and backtrace
/// - assert(cond[, msg]): panic unless cond is truthy
/// - assert_eq(actual, expected[, msg]): panic unless values are equal
/// - assert_ne(actual, expected[, msg]): panic unless values are not equal
pub fn register_stdlib_core_globals(registry: &mut ModuleRegistry) {
    register_full_state_builtin!(registry, print => print / NativeEntry::VARIADIC => core.print: Nil);
    register_full_state_builtin!(registry, println => println / NativeEntry::VARIADIC => core.println: Nil);
    register_full_state_builtin!(registry, panic => panic / NativeEntry::VARIADIC => core.panic: Nil);
    register_full_state_builtin!(registry, error => error / NativeEntry::VARIADIC => core.error: Nil);
    register_full_state_builtin!(registry, pcall => pcall / NativeEntry::VARIADIC => core.pcall: RuntimeValue);
    register_full_state_builtin!(registry, assert => assert / NativeEntry::VARIADIC => core.assert: Nil);
    register_full_state_builtin!(registry, assert_eq => assert_eq / NativeEntry::VARIADIC => core.assert_eq: Nil);
    register_full_state_builtin!(registry, assert_ne => assert_ne / NativeEntry::VARIADIC => core.assert_ne: Nil);
    register_plain_builtin!(registry, coroutine_create => coroutine_create / 1);
    register_full_state_builtin!(registry, coroutine_resume => coroutine_resume / NativeEntry::VARIADIC => core.coroutine_resume: RuntimeValue);
    register_plain_builtin!(registry, coroutine_status => coroutine_status / 1);
}

pub fn register_stdlib_concurrency_globals(registry: &mut ModuleRegistry) {
    register_plain_builtin!(registry, spawn => spawn / 1);
    register_plain_builtin!(registry, chan => chan / NativeEntry::VARIADIC => core.chan: RuntimeValue);
    register_plain_builtin!(registry, send => send / 2 => core.send: Nil);
    register_plain_builtin!(registry, recv => recv / 1 => core.recv: RuntimeValue);
    register_plain_builtin!(registry, "chan::try_send" => chan_try_send / 2);
    register_plain_builtin!(registry, "chan::try_recv" => chan_try_recv / 1);
    register_plain_builtin!(registry, "select$block" => select_block / 5);
}

fn register_runtime_builtin(
    registry: &mut ModuleRegistry,
    name: &'static str,
    function: fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>,
    arity: u16,
    metadata: Option<StdlibGlobalMetadata>,
) {
    register_global_metadata(name, metadata);
    registry.register_runtime_builtin(name, NativeFunction::Plain(function), arity);
}

fn register_runtime_builtin_full_state(
    registry: &mut ModuleRegistry,
    name: &'static str,
    function: fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>,
    arity: u16,
    metadata: Option<StdlibGlobalMetadata>,
) {
    register_global_metadata(name, metadata);
    registry.register_runtime_builtin(name, NativeFunction::FullState(function), arity);
}

fn register_global_metadata(name: &'static str, metadata: Option<StdlibGlobalMetadata>) {
    let Some(metadata) = metadata else {
        return;
    };
    debug_assert_eq!(metadata.name, name);
    register_stdlib_global_metadata(metadata).expect("stdlib global metadata should be consistent");
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

/// `error(value...)` — raise a recoverable error. Unlike `panic`, it propagates
/// as a catchable error (caught by `pcall`) rather than aborting; uncaught, it
/// fails the program. A single non-heap value (Int/Float/Bool/ShortStr/Nil) is
/// carried first-class so `pcall` returns it as-is (M2.2, primitives); otherwise
/// a stringified message is raised (heap-object first-class values need GC
/// rooting across unwinding — deferred).
fn error(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if let [value] = args.as_slice() {
        let value = *value;
        // Capture the display up-front: an uncaught heap error can't be rendered
        // later (the heap is gone once execution unwinds out) (plan M2.2).
        let rendered = join_runtime_display(args.as_slice(), runtime)?;
        // A heap object must be pinned as a GC root so it survives collection at
        // the native-call safepoints hit while the error unwinds to its `pcall`
        // (plan M2.2). Primitives are Copy and need no pinning. If full VM state
        // is unavailable we can't pin, so fall back to a stringified message.
        let carry_first_class = if matches!(value, RuntimeVal::Obj(_)) {
            match runtime.state_ctx_module_mut() {
                Some((state, _, _)) => {
                    state.set_pending_raise_root(Some(value));
                    true
                }
                None => false,
            }
        } else {
            true
        };
        if carry_first_class {
            return Err(anyhow!(lk_core::vm::LkRaisedValue {
                value,
                rendered: Arc::<str>::from(rendered.as_str()),
            }));
        }
        return Err(anyhow!("{rendered}"));
    }
    let msg = if args.is_empty() {
        "error".to_string()
    } else {
        join_runtime_display(args.as_slice(), runtime)?
    };
    Err(anyhow!("{msg}"))
}

/// `pcall(f, args...) -> [ok, result_or_error]` — a protected call. Invokes `f`
/// with `args`; on success returns `[true, result]`, on any raised error returns
/// `[false, message]` instead of propagating. This is the recoverable-error
/// primitive (plan M2.1); it catches both `error(...)` and other runtime errors.
fn pcall(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some((&callee, call_args)) = values.split_first() else {
        return Err(anyhow!("pcall expects at least 1 argument: the function to call"));
    };
    let call_args = call_args.to_vec();
    let outcome = {
        let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
            return Err(anyhow!("pcall requires full VM state"));
        };
        call_runtime_value_runtime(callee, &call_args, state, module, ctx)
    };
    if outcome.is_err()
        && let Some((state, ctx, _)) = runtime.state_ctx_module_mut()
    {
        // The error is caught here: release the GC-root pin on any first-class
        // heap error value now that it's about to be handed back (plan M2.2).
        // The value stays valid — the following `pcall` allocations use the raw
        // heap (no collection) — but it no longer needs to survive as a stray
        // root once execution resumes normally.
        state.set_pending_raise_root(None);
        // Discard the traceback frames the errored call accumulated — a later
        // uncaught error should report a clean call stack (plan M2.2). try/catch
        // desugars to pcall, so this also covers caught language errors.
        if let Some(ctx) = ctx {
            ctx.truncate_call_stack(0);
        }
    }
    let (ok, value) = match outcome {
        Ok(result) => (true, result),
        Err(err) => {
            // The call machinery wraps errors with context, so inspect the
            // deepest cause. A first-class primitive error value round-trips as
            // itself (M2.2); otherwise the message string is returned.
            let root = err.root_cause();
            if let Some(raised) = root.downcast_ref::<lk_core::vm::LkRaisedValue>() {
                (false, raised.value)
            } else {
                let message = root.to_string();
                let handle = runtime
                    .heap_mut()
                    .alloc(HeapValue::String(Arc::<str>::from(message.as_str())));
                (false, RuntimeVal::Obj(handle))
            }
        }
    };
    let list = runtime
        .heap_mut()
        .alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Bool(ok), value])));
    Ok(RuntimeVal::Obj(list))
}

/// `coroutine_create(fn) -> Coroutine`: wraps a plain LK function as a
/// stackless coroutine (plan: coroutines/`yield`, post-M2.5). Does not run
/// any of the function's code — that happens on the first `coroutine_resume`.
fn coroutine_create(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let Some(&callee) = args.as_slice().first() else {
        return Err(anyhow!("coroutine_create expects 1 argument: the function"));
    };
    create_coroutine_runtime(callee, runtime.heap_mut())
}

/// `coroutine_resume(co, ...args) -> [ok, value]` — mirrors `pcall`'s
/// `[ok, value]` convention. `args` seed the entry function's parameters on
/// the first resume; on later resumes only `args[0]` (or `Nil`) is delivered
/// as the paused `yield` expression's result.
fn coroutine_resume(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some((&coroutine, resume_args)) = values.split_first() else {
        return Err(anyhow!("coroutine_resume expects at least 1 argument: the coroutine"));
    };
    let resume_args = resume_args.to_vec();
    let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
        return Err(anyhow!("coroutine_resume requires full VM state"));
    };
    resume_coroutine_runtime(coroutine, &resume_args, &[], state, module, ctx)
}

/// `coroutine_status(co) -> "suspended" | "running" | "dead"`.
fn coroutine_status(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let Some(&coroutine) = args.as_slice().first() else {
        return Err(anyhow!("coroutine_status expects 1 argument: the coroutine"));
    };
    let status = coroutine_status_runtime(coroutine, runtime.heap())?;
    Ok(runtime_string_value(status, runtime.heap_mut()))
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
    Err(anyhow!("{message}"))
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
    Err(anyhow!("{message}"))
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
    Err(anyhow!("{message}"))
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

    let task_id = runtime
        .async_runtime()
        .with(|runtime| runtime.spawn(fut))
        .map_err(|error| anyhow!("Failed to spawn task: {}", error))?;
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
    let channel_id = runtime
        .async_runtime()
        .with(|runtime| runtime.create_channel(cap_opt))
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
    let sent = runtime
        .async_runtime()
        .with(|runtime| runtime.block_on(runtime.send_async(channel_id, value)))
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
    let (ok, value) = runtime
        .async_runtime()
        .with(|runtime| runtime.block_on(runtime.recv_async(channel_id)))
        .map_err(|error| anyhow!("Receive operation failed: {}", error))?;
    let value = value.into_value(runtime.heap_mut())?;
    runtime_list(vec![RuntimeVal::Bool(ok), value], runtime.heap_mut())
}

fn chan_try_send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_runtime_arity(args, 2, "chan::try_send")?;
    let values = args.as_slice();
    let channel_id = channel_id_arg(&values[0], runtime.heap(), "chan::try_send first argument")?;
    let value = RuntimePayload::copy_from_value(&values[1], runtime.heap())?;
    let sent = runtime
        .async_runtime()
        .with(|runtime| runtime.try_send(channel_id, value))
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
    let payload = match runtime.async_runtime().with(|runtime| runtime.try_recv(channel_id))? {
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

    let result = runtime
        .async_runtime()
        .with(|runtime| runtime.block_on(select.execute(runtime, has_default)))?;
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
        (TypedList::Mixed(left), _) => typed_list_runtime_item_equal(left[left_index], right, right_index, heap),
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
        register_stdlib_modules_named(&mut registry, &["math".to_string()]).expect("register math again");

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
        let mut ctx = lk_core::vm::VmContext::new_without_core_vm_builtins();
        let mut state = RuntimeModuleState::default();
        let channel_id = ctx.async_runtime().with(|runtime| runtime.create_channel(Some(1)))?;
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
        let mut runtime = NativeRuntime::new(&mut state, Some(&mut ctx), None);

        let result = select_block(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("select$block should return list object");
        };
        assert!(matches!(runtime.heap().get(handle), Some(HeapValue::List(_))));
        assert_eq!(runtime.heap().len(), 6);
        Ok(())
    }
}
