use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs32, NativeEntry32, NativeRuntime32, RuntimeExport32},
};

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct OsModule;

impl Default for OsModule {
    fn default() -> Self {
        Self::new()
    }
}

impl OsModule {
    pub fn new() -> Self {
        Self
    }
}

impl Module for OsModule {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> &str {
        "Operating system interface"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("hostname", hostname32, 0),
                RuntimeNativeExport32::plain("arch", arch32, 0),
                RuntimeNativeExport32::plain("os", os32, 0),
                RuntimeNativeExport32::plain("exit", exit32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("exec", exec32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("clock", clock32, 0),
                RuntimeNativeExport32::plain("time", time32, 0),
                RuntimeNativeExport32::plain("epoch", epoch32, 0),
                RuntimeNativeExport32::plain("env_get", env_get32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("env_set", env_set32, 2),
                RuntimeNativeExport32::plain("env_unset", env_unset32, 1),
                RuntimeNativeExport32::plain("dir_list", dir_list32, 1),
                RuntimeNativeExport32::plain("dir_temp", dir_temp32, 0),
                RuntimeNativeExport32::plain("dir_current", dir_current32, 0),
            ],
            &[],
        ))
    }
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }
    bail!(
        "{name}() takes exactly {expected} argument{}",
        if expected == 1 { "" } else { "s" }
    )
}

fn no_args(args: NativeArgs32<'_>, name: &str) -> Result<()> {
    if args.len() == 0 {
        Ok(())
    } else {
        bail!("{name}() takes no arguments")
    }
}

fn int_arg(value: &RuntimeVal, name: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        other => Err(anyhow!("{name} must be an integer, got {:?}", other.kind())),
    }
}

fn bool_arg(value: &RuntimeVal, name: &str) -> Result<bool> {
    match value {
        RuntimeVal::Bool(value) => Ok(*value),
        other => Err(anyhow!("{name} must be a boolean, got {:?}", other.kind())),
    }
}

fn string_arg(value: &RuntimeVal, runtime: &NativeRuntime32<'_>, name: &str) -> Result<String> {
    Ok(runtime_string_arg(value, runtime.heap(), name)?.to_string())
}

fn optional_string_arg(value: &RuntimeVal, runtime: &NativeRuntime32<'_>, name: &str) -> Result<Option<String>> {
    if matches!(value, RuntimeVal::Nil) {
        return Ok(None);
    }
    Ok(Some(string_arg(value, runtime, name)?))
}

fn hostname32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "hostname")?;
    let hostname = std::env::var_os("HOSTNAME")
        .or_else(|| std::env::var_os("COMPUTERNAME"))
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "localhost".to_string());
    Ok(runtime_string_value(&hostname, runtime.heap_mut()))
}

fn arch32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "arch")?;
    Ok(runtime_string_value(std::env::consts::ARCH, runtime.heap_mut()))
}

fn os32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "os")?;
    Ok(runtime_string_value(std::env::consts::OS, runtime.heap_mut()))
}

fn exit32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    if args.len() > 1 {
        bail!("exit() takes at most 1 argument: exit_code");
    }
    let code = if let Some(value) = args.get(0) {
        int_arg(value, "exit code")? as i32
    } else {
        0
    };
    std::process::exit(code);
}

fn clock32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "clock")?;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static START: AtomicU64 = AtomicU64::new(0);
    static INIT: AtomicBool = AtomicBool::new(false);
    if !INIT.swap(true, Ordering::SeqCst) {
        START.store(epoch_nanos(), Ordering::SeqCst);
    }
    let elapsed_secs = epoch_nanos().wrapping_sub(START.load(Ordering::SeqCst)) as f64 / 1_000_000_000.0;
    Ok(RuntimeVal::Float(elapsed_secs))
}

fn time32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "time")?;
    Ok(RuntimeVal::Int(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    ))
}

fn epoch32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "epoch")?;
    Ok(RuntimeVal::Int(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
    ))
}

fn epoch_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn env_get32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    if args.len() != 1 && args.len() != 2 {
        bail!("env.get() takes 1 or 2 arguments: variable_name [, default_value]");
    }
    let name = string_arg(args.get(0).expect("checked arity"), runtime, "env.get variable_name")?;
    let default = if let Some(value) = args.get(1) {
        optional_string_arg(value, runtime, "env.get default_value")?
    } else {
        None
    };
    match std::env::var_os(&name).and_then(|value| value.into_string().ok()) {
        Some(value) => Ok(runtime_string_value(&value, runtime.heap_mut())),
        None => match default {
            Some(value) => Ok(runtime_string_value(&value, runtime.heap_mut())),
            None => Ok(RuntimeVal::Nil),
        },
    }
}

fn env_set32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "env.set")?;
    let _ = string_arg(args.get(0).expect("checked arity"), runtime, "env.set variable_name")?;
    let _ = string_arg(args.get(1).expect("checked arity"), runtime, "env.set value")?;
    bail!("env.set() is disabled: mutating process environment is unsafe in the VM runtime")
}

fn env_unset32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "env.unset")?;
    let _ = string_arg(args.get(0).expect("checked arity"), runtime, "env.unset variable_name")?;
    bail!("env.unset() is disabled: mutating process environment is unsafe in the VM runtime")
}

fn dir_list32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "dir.list")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "dir.list path")?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&path).map_err(|err| anyhow!("failed to read directory: {err}"))? {
        let Ok(entry) = entry else {
            continue;
        };
        if let Some(name) = entry.file_name().to_str() {
            entries.push(std::sync::Arc::<str>::from(name));
        }
    }
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::List(TypedList::String(entries))),
    ))
}

fn dir_temp32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "dir.temp")?;
    Ok(match std::env::temp_dir().into_os_string().into_string() {
        Ok(path) => runtime_string_value(&path, runtime.heap_mut()),
        Err(_) => RuntimeVal::Nil,
    })
}

fn dir_current32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    no_args(args, "dir.current")?;
    Ok(match std::env::current_dir() {
        Ok(path) => match path.into_os_string().into_string() {
            Ok(path) => runtime_string_value(&path, runtime.heap_mut()),
            Err(_) => RuntimeVal::Nil,
        },
        Err(_) => RuntimeVal::Nil,
    })
}

fn exec32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    use std::process::Command;

    if args.is_empty() || args.len() > 3 {
        bail!("exec() expects 1-3 arguments: cmd[, args_list][, stream_bool]");
    }
    let cmd = string_arg(args.get(0).expect("checked arity"), runtime, "exec cmd")?;
    let mut argv = Vec::new();
    let mut stream = false;

    if let Some(second) = args.get(1) {
        match second {
            RuntimeVal::Bool(_) => stream = bool_arg(second, "exec stream")?,
            _ => argv = string_list_arg(second, runtime, "exec args_list")?,
        }
    }
    if let Some(third) = args.get(2) {
        stream = bool_arg(third, "exec stream")?;
    }

    let output = Command::new(&cmd)
        .args(&argv)
        .output()
        .map_err(|err| anyhow!("failed to execute '{cmd}': {err}"))?;
    let stdout = String::from_utf8(output.stdout).map_err(|_| anyhow!("command stdout is not valid UTF-8"))?;
    if stream {
        let lines = stdout
            .lines()
            .map(|line| std::sync::Arc::<str>::from(line.trim_end_matches('\r')))
            .collect::<Vec<_>>();
        return Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(lines))),
        ));
    }
    Ok(runtime_string_value(&stdout, runtime.heap_mut()))
}

fn string_list_arg(value: &RuntimeVal, runtime: &NativeRuntime32<'_>, context: &str) -> Result<Vec<String>> {
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
            .map(|value| runtime_string_arg(value, runtime.heap(), context).map(|value| value.to_string()))
            .collect(),
        _ => bail!("{context} must contain only strings"),
    }
}
