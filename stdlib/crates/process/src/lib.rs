use anyhow::{Result, anyhow, bail};
use lk_core::{
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedList, TypedMap},
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_bytes::runtime_bytes_value;
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::{process::Command, sync::Arc};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "process", docs = "Process execution and state helpers")]
pub struct ProcessModule;

#[lk_stdlib_common::stdlib_exports(module = "process")]
impl ProcessModule {
    #[stdlib_export(name = "id", params(), returns = Int)]
    fn id(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Int(std::process::id() as i64))
    }

    #[stdlib_export(name = "cwd", params(), returns = String?)]
    fn cwd(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match std::env::current_dir() {
            Ok(path) => Ok(runtime_string_value(&path.to_string_lossy(), runtime.heap_mut())),
            Err(_) => Ok(RuntimeVal::Nil),
        }
    }

    #[stdlib_export(name = "set_cwd", params(path: String), returns = Bool)]
    fn set_cwd(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "process.set_cwd path",
        )?;
        std::env::set_current_dir(path.as_ref()).map_err(|err| anyhow!("failed to set cwd '{}': {err}", path))?;
        Ok(RuntimeVal::Bool(true))
    }

    #[stdlib_export(name = "exit", params(code: Int), returns = Nil)]
    fn exit(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let code = int_arg(args.get(0).expect("checked arity"), "process.exit code")?;
        if code < i64::from(i32::MIN) || code > i64::from(i32::MAX) {
            bail!("process.exit code must fit in i32, got {code}");
        }
        let code = code as i32;
        std::process::exit(code);
    }

    #[stdlib_export(name = "status", params(cmd: String, ...args: String), returns = Int)]
    fn status(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (cmd, argv) = command_args(args, runtime, "process.status()")?;
        let status = Command::new(cmd.as_ref())
            .args(&argv)
            .status()
            .map_err(|err| anyhow!("failed to execute '{}': {err}", cmd))?;
        Ok(RuntimeVal::Int(status.code().unwrap_or(-1) as i64))
    }

    #[stdlib_export(name = "output", params(cmd: String, ...args: String), returns = Map)]
    fn output(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (cmd, argv) = command_args(args, runtime, "process.output()")?;
        let output = Command::new(cmd.as_ref())
            .args(&argv)
            .output()
            .map_err(|err| anyhow!("failed to execute '{}': {err}", cmd))?;
        output_map(output, runtime)
    }

    #[stdlib_export(name = "output_string", params(cmd: String, ...args: String), returns = String)]
    fn output_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (cmd, argv) = command_args(args, runtime, "process.output_string()")?;
        let output = Command::new(cmd.as_ref())
            .args(&argv)
            .output()
            .map_err(|err| anyhow!("failed to execute '{}': {err}", cmd))?;
        let stdout = String::from_utf8(output.stdout).map_err(|_| anyhow!("command stdout is not valid UTF-8"))?;
        Ok(runtime_string_value(&stdout, runtime.heap_mut()))
    }
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
