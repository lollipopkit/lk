use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedList, TypedMap},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use lk_stdlib_bytes::runtime_bytes_value;
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::{process::Command, sync::Arc};

#[derive(Debug, Default)]
pub struct ProcessModule;

impl ProcessModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for ProcessModule {
    fn name(&self) -> &str {
        "process"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("id", id, 0),
                RuntimeNativeExport::plain("cwd", cwd, 0),
                RuntimeNativeExport::plain("set_cwd", set_cwd, 1),
                RuntimeNativeExport::plain("exit", exit, 1),
                RuntimeNativeExport::plain("status", status, lk_core::vm::NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("output", output, lk_core::vm::NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("output_string", output_string, lk_core::vm::NativeEntry::VARIADIC),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("process", Box::new(ProcessModule::new()))
}

fn id(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "process.id()")?;
    Ok(RuntimeVal::Int(std::process::id() as i64))
}

fn cwd(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "process.cwd()")?;
    match std::env::current_dir() {
        Ok(path) => Ok(runtime_string_value(&path.to_string_lossy(), runtime.heap_mut())),
        Err(_) => Ok(RuntimeVal::Nil),
    }
}

fn set_cwd(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "process.set_cwd()")?;
    let path = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "process.set_cwd path",
    )?;
    std::env::set_current_dir(path.as_ref()).map_err(|err| anyhow!("failed to set cwd '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn exit(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "process.exit()")?;
    let code = int_arg(args.get(0).expect("checked arity"), "process.exit code")? as i32;
    std::process::exit(code);
}

fn status(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (cmd, argv) = command_args(args, runtime, "process.status()")?;
    let status = Command::new(cmd.as_ref())
        .args(&argv)
        .status()
        .map_err(|err| anyhow!("failed to execute '{}': {err}", cmd))?;
    Ok(RuntimeVal::Int(status.code().unwrap_or(-1) as i64))
}

fn output(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (cmd, argv) = command_args(args, runtime, "process.output()")?;
    let output = Command::new(cmd.as_ref())
        .args(&argv)
        .output()
        .map_err(|err| anyhow!("failed to execute '{}': {err}", cmd))?;
    output_map(output, runtime)
}

fn output_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (cmd, argv) = command_args(args, runtime, "process.output_string()")?;
    let output = Command::new(cmd.as_ref())
        .args(&argv)
        .output()
        .map_err(|err| anyhow!("failed to execute '{}': {err}", cmd))?;
    let stdout = String::from_utf8(output.stdout).map_err(|_| anyhow!("command stdout is not valid UTF-8"))?;
    Ok(runtime_string_value(&stdout, runtime.heap_mut()))
}

fn output_map(output: std::process::Output, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let mut map = fast_hash_map_new();
    map.insert(
        Arc::<str>::from("status"),
        RuntimeVal::Int(output.status.code().unwrap_or(-1) as i64),
    );
    map.insert(Arc::<str>::from("success"), RuntimeVal::Bool(output.status.success()));
    map.insert(
        Arc::<str>::from("stdout"),
        runtime_bytes_value(output.stdout, runtime.heap_mut()),
    );
    map.insert(
        Arc::<str>::from("stderr"),
        runtime_bytes_value(output.stderr, runtime.heap_mut()),
    );
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))),
    ))
}

fn command_args(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<(Arc<str>, Vec<String>)> {
    if args.is_empty() || args.len() > 2 {
        bail!("{name} expects 1 or 2 arguments: cmd[, args]");
    }
    let cmd = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "process command")?;
    let argv = if let Some(value) = args.get(1) {
        string_list_arg(value, runtime, "process args")?
    } else {
        Vec::new()
    };
    Ok((cmd, argv))
}

fn string_list_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, context: &str) -> Result<Vec<String>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a list");
    };
    let value = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::List(list) = value else {
        bail!("{context} must be a list, got {}", value.type_name());
    };
    match list {
        TypedList::String(values) => Ok(values.iter().map(ToString::to_string).collect()),
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| Ok(runtime_string_arg(value, runtime.heap(), context)?.to_string()))
            .collect(),
        _ => bail!("{context} must contain only strings"),
    }
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        other => bail!("{context} expects Int, got {:?}", other.kind()),
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
