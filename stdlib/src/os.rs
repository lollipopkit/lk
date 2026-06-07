use anyhow::{Result, anyhow, bail};
use lk_core::util::fast_map::{FastHashMap, fast_hash_map_new};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::{CallableValue, HeapStore, HeapValue, RuntimeVal, TypedList, TypedMap},
    vm::{
        Module, NativeArgs, NativeEntry, NativeFunction, NativeRuntime, PlainNativeFunction, RuntimeExport,
        RuntimeModuleState,
    },
};
use std::sync::{Arc, Mutex};

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

impl ModuleProvider for OsModule {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> &str {
        "Operating system interface"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        fn callable(heap: &mut HeapStore, f: PlainNativeFunction, arity: u16) -> RuntimeVal {
            RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
                name: Arc::<str>::from("os::<native>"),
                arity,
                function: NativeFunction::Plain(f),
            })))
        }
        fn key(s: &str) -> Arc<str> {
            Arc::<str>::from(s)
        }

        let mut heap = HeapStore::new();

        // Build os.env sub-namespace
        let mut env_entries: FastHashMap<Arc<str>, RuntimeVal> = fast_hash_map_new();
        env_entries.insert(key("get"), callable(&mut heap, env_get, NativeEntry::VARIADIC));
        env_entries.insert(key("set"), callable(&mut heap, env_set, 2));
        env_entries.insert(key("unset"), callable(&mut heap, env_unset, 1));
        let env_val = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(env_entries))));

        // Build outer module map
        let mut entries: FastHashMap<Arc<str>, RuntimeVal> = fast_hash_map_new();
        entries.insert(key("hostname"), callable(&mut heap, hostname, 0));
        entries.insert(key("arch"), callable(&mut heap, arch, 0));
        entries.insert(key("os"), callable(&mut heap, os, 0));
        entries.insert(key("exit"), callable(&mut heap, exit, NativeEntry::VARIADIC));
        entries.insert(key("exec"), callable(&mut heap, exec, NativeEntry::VARIADIC));
        entries.insert(key("clock"), callable(&mut heap, clock, 0));
        entries.insert(key("time"), callable(&mut heap, time, 0));
        entries.insert(key("epoch"), callable(&mut heap, epoch, 0));
        entries.insert(key("env_get"), callable(&mut heap, env_get, NativeEntry::VARIADIC));
        entries.insert(key("env_set"), callable(&mut heap, env_set, 2));
        entries.insert(key("env_unset"), callable(&mut heap, env_unset, 1));
        entries.insert(key("dir_list"), callable(&mut heap, dir_list, 1));
        entries.insert(key("dir_temp"), callable(&mut heap, dir_temp, 0));
        entries.insert(key("dir_current"), callable(&mut heap, dir_current, 0));
        entries.insert(key("file_read"), callable(&mut heap, file_read, 1));
        entries.insert(key("file_write"), callable(&mut heap, file_write, 2));
        entries.insert(key("file_append"), callable(&mut heap, file_append, 2));
        entries.insert(key("file_exists"), callable(&mut heap, file_exists, 1));
        entries.insert(key("file_size"), callable(&mut heap, file_size, 1));
        entries.insert(key("file_delete"), callable(&mut heap, file_delete, 1));
        entries.insert(key("mkdir"), callable(&mut heap, mkdir, 1));
        entries.insert(key("path_join"), callable(&mut heap, path_join, NativeEntry::VARIADIC));
        entries.insert(key("path_sep"), callable(&mut heap, path_sep, 0));
        entries.insert(key("env"), env_val);

        let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
        Ok(RuntimeExport::new(
            value,
            Arc::new(Mutex::new(RuntimeModuleState::new(heap, Vec::new()))),
            Arc::new(Module::default()),
        ))
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }
    bail!(
        "{name}() takes exactly {expected} argument{}",
        if expected == 1 { "" } else { "s" }
    )
}

fn no_args(args: NativeArgs<'_>, name: &str) -> Result<()> {
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

fn string_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, name: &str) -> Result<String> {
    Ok(runtime_string_arg(value, runtime.heap(), name)?.to_string())
}

fn optional_string_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, name: &str) -> Result<Option<String>> {
    if matches!(value, RuntimeVal::Nil) {
        return Ok(None);
    }
    Ok(Some(string_arg(value, runtime, name)?))
}

fn hostname(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "hostname")?;
    let hostname = std::env::var_os("HOSTNAME")
        .or_else(|| std::env::var_os("COMPUTERNAME"))
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "localhost".to_string());
    Ok(runtime_string_value(&hostname, runtime.heap_mut()))
}

fn arch(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "arch")?;
    Ok(runtime_string_value(std::env::consts::ARCH, runtime.heap_mut()))
}

fn os(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "os")?;
    Ok(runtime_string_value(std::env::consts::OS, runtime.heap_mut()))
}

fn exit(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn clock(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn time(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "time")?;
    Ok(RuntimeVal::Int(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    ))
}

fn epoch(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn env_get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn env_set(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "env.set")?;
    let _ = string_arg(args.get(0).expect("checked arity"), runtime, "env.set variable_name")?;
    let _ = string_arg(args.get(1).expect("checked arity"), runtime, "env.set value")?;
    bail!("env.set() is disabled: mutating process environment is unsafe in the VM runtime")
}

fn env_unset(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "env.unset")?;
    let _ = string_arg(args.get(0).expect("checked arity"), runtime, "env.unset variable_name")?;
    bail!("env.unset() is disabled: mutating process environment is unsafe in the VM runtime")
}

fn dir_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn dir_temp(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "dir.temp")?;
    Ok(match std::env::temp_dir().into_os_string().into_string() {
        Ok(path) => runtime_string_value(&path, runtime.heap_mut()),
        Err(_) => RuntimeVal::Nil,
    })
}

fn dir_current(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "dir.current")?;
    Ok(match std::env::current_dir() {
        Ok(path) => match path.into_os_string().into_string() {
            Ok(path) => runtime_string_value(&path, runtime.heap_mut()),
            Err(_) => RuntimeVal::Nil,
        },
        Err(_) => RuntimeVal::Nil,
    })
}

fn exec(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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
        let mut lines = Vec::new();
        for line in stdout.lines() {
            lines.push(std::sync::Arc::<str>::from(line.trim_end_matches('\r')));
        }
        return Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(lines))),
        ));
    }
    Ok(runtime_string_value(&stdout, runtime.heap_mut()))
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
        TypedList::String(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(value.to_string());
            }
            Ok(out)
        }
        TypedList::Mixed(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(runtime_string_arg(value, runtime.heap(), context)?.to_string());
            }
            Ok(out)
        }
        _ => bail!("{context} must contain only strings"),
    }
}

// ── File system operations ──────────────────────────────────

fn file_read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file_read")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "file_read path")?;
    let content = std::fs::read_to_string(&path).map_err(|err| anyhow!("failed to read file '{}': {}", path, err))?;
    Ok(runtime_string_value(&content, runtime.heap_mut()))
}

fn file_write(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "file_write")?;
    let values = args.as_slice();
    let path = string_arg(values.get(0).expect("checked arity"), runtime, "file_write path")?;
    let content = string_arg(values.get(1).expect("checked arity"), runtime, "file_write content")?;
    std::fs::write(&path, content.as_bytes()).map_err(|err| anyhow!("failed to write file '{}': {}", path, err))?;
    Ok(RuntimeVal::Bool(true))
}

fn file_append(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "file_append")?;
    let values = args.as_slice();
    let path = string_arg(values.get(0).expect("checked arity"), runtime, "file_append path")?;
    let content = string_arg(values.get(1).expect("checked arity"), runtime, "file_append content")?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| anyhow!("failed to open file '{}': {}", path, err))?;
    file.write_all(content.as_bytes())
        .map_err(|err| anyhow!("failed to append to file '{}': {}", path, err))?;
    Ok(RuntimeVal::Bool(true))
}

fn file_exists(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file_exists")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "file_exists path")?;
    Ok(RuntimeVal::Bool(std::path::Path::new(&path).exists()))
}

fn file_size(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file_size")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "file_size path")?;
    let metadata = std::fs::metadata(&path).map_err(|err| anyhow!("failed to get metadata for '{}': {}", path, err))?;
    Ok(RuntimeVal::Int(metadata.len() as i64))
}

fn file_delete(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file_delete")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "file_delete path")?;
    let result = std::fs::remove_file(&path);
    Ok(RuntimeVal::Bool(result.is_ok()))
}

fn mkdir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "mkdir")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "mkdir path")?;
    std::fs::create_dir_all(&path).map_err(|err| anyhow!("failed to create directory '{}': {}", path, err))?;
    Ok(RuntimeVal::Bool(true))
}

fn path_join(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.is_empty() {
        bail!("path_join() requires at least 1 argument");
    }
    let values = args.as_slice();
    let mut path = std::path::PathBuf::new();
    for value in values {
        let component = string_arg(value, runtime, "path_join component")?;
        path.push(component.as_ref() as &std::path::Path);
    }
    let result = path.to_string_lossy();
    Ok(runtime_string_value(&result, runtime.heap_mut()))
}

fn path_sep(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(_args, "path_sep")?;
    Ok(runtime_string_value(std::path::MAIN_SEPARATOR_STR, runtime.heap_mut()))
}
