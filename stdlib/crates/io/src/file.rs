use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::{ResourceHandle, RuntimeVal},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use std::{fs::OpenOptions, io::Write};

use crate::std_io as io_std;
use crate::{
    bytes::{runtime_bytes_or_string_arg, runtime_bytes_value},
    resource::{close_resource, resource_arg, resource_value},
    runtime_native::runtime_string_arg,
};

#[derive(Debug)]
pub struct IoFileModule;

impl IoFileModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IoFileModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for IoFileModule {
    fn name(&self) -> &str {
        "file"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("open", open, 2),
                RuntimeNativeExport::plain("create", create, 1),
                RuntimeNativeExport::plain("read", read, 1),
                RuntimeNativeExport::plain("write", write_path, 2),
                RuntimeNativeExport::plain("append", append, 2),
                RuntimeNativeExport::plain("exists", exists, 1),
                RuntimeNativeExport::plain("size", size, 1),
                RuntimeNativeExport::plain("remove", remove, 1),
                RuntimeNativeExport::plain("read_to_string", read_to_string, 1),
                RuntimeNativeExport::plain("write_all", write_all, 2),
                RuntimeNativeExport::plain("flush", io_std::flush, 1),
                RuntimeNativeExport::plain("close", close, 1),
            ],
            &[],
        ))
    }
}

fn open(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "file.open()")?;
    let values = args.as_slice();
    let path = runtime_string_arg(&values[0], runtime.heap(), "file.open path")?;
    let mode = runtime_string_arg(&values[1], runtime.heap(), "file.open mode")?;
    let file = open_with_mode(&path, &mode)?;
    Ok(resource_value("File", ResourceHandle::File(file), runtime.heap_mut()))
}

fn create(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.create()")?;
    let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.create path")?;
    let file =
        std::fs::File::create(path.as_ref()).map_err(|err| anyhow!("failed to create file '{}': {err}", path))?;
    Ok(resource_value("File", ResourceHandle::File(file), runtime.heap_mut()))
}

fn read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.read()")?;
    let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.read path")?;
    let content = std::fs::read(path.as_ref()).map_err(|err| anyhow!("failed to read file '{}': {err}", path))?;
    Ok(runtime_bytes_value(content, runtime.heap_mut()))
}

fn read_to_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.read_to_string()")?;
    match runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "file.read_to_string path",
    ) {
        Ok(path) => {
            let content = std::fs::read_to_string(path.as_ref())
                .map_err(|err| anyhow!("failed to read file '{}': {err}", path))?;
            Ok(crate::runtime_native::runtime_string_value(
                &content,
                runtime.heap_mut(),
            ))
        }
        Err(_) => io_std::read_to_string(args, runtime),
    }
}

fn write_path(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "file.write()")?;
    let values = args.as_slice();
    let path = runtime_string_arg(&values[0], runtime.heap(), "file.write path")?;
    let content = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "file.write content")?;
    std::fs::write(path.as_ref(), &content).map_err(|err| anyhow!("failed to write file '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn append(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "file.append()")?;
    let values = args.as_slice();
    let path = runtime_string_arg(&values[0], runtime.heap(), "file.append path")?;
    let content = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "file.append content")?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_ref())
        .map_err(|err| anyhow!("failed to open file '{}': {err}", path))?;
    file.write_all(&content)
        .map_err(|err| anyhow!("failed to append file '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn exists(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.exists()")?;
    let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.exists path")?;
    Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).exists()))
}

fn size(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.size()")?;
    let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.size path")?;
    let len = std::fs::metadata(path.as_ref())
        .map_err(|err| anyhow!("failed to stat file '{}': {err}", path))?
        .len();
    Ok(RuntimeVal::Int(len as i64))
}

fn remove(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.remove()")?;
    let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.remove path")?;
    match std::fs::remove_file(path.as_ref()) {
        Ok(()) => Ok(RuntimeVal::Bool(true)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(RuntimeVal::Bool(false)),
        Err(err) => Err(anyhow!("failed to remove file '{}': {err}", path)),
    }
}

fn write_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    io_std::write(args, runtime)
}

fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "file.close()")?;
    let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.close()")?;
    Ok(RuntimeVal::Bool(close_resource(&resource)?))
}

fn open_with_mode(path: &str, mode: &str) -> Result<std::fs::File> {
    let mut options = OpenOptions::new();
    match mode {
        "r" | "read" => {
            options.read(true);
        }
        "w" | "write" => {
            options.create(true).write(true).truncate(true);
        }
        "a" | "append" => {
            options.create(true).append(true);
        }
        "rw" | "read_write" => {
            options.create(true).read(true).write(true);
        }
        "r+" => {
            options.read(true).write(true);
        }
        "w+" => {
            options.create(true).read(true).write(true).truncate(true);
        }
        "a+" => {
            options.create(true).read(true).append(true);
        }
        other => bail!("unsupported file mode '{other}'"),
    }
    options
        .open(path)
        .map_err(|err| anyhow!("failed to open file '{path}': {err}"))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
