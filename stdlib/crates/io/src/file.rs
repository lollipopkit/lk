use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::{ResourceHandle, RuntimeVal},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use std::fs::OpenOptions;

use crate::std_io as io_std;
use crate::{
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
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "open" => open, 2,
                plain "create" => create, 1,
                plain "read" => io_std::read, lk_core::vm::NativeEntry::VARIADIC,
                plain "read_to_string" => io_std::read_to_string, 1,
                plain "read_line" => io_std::read_line, 1,
                plain "write" => io_std::write, 2,
                plain "writeln" => io_std::writeln_fn, 2,
                plain "write_all" => write_all, 2,
                plain "flush" => io_std::flush, 1,
                plain "close" => close, 1,
            ],
        ))
    }
}

fn open(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "file.open()")?;
    let values = args.as_slice();
    let path = runtime_string_arg(&values[0], runtime.heap(), "file.open path")?;
    let mode = runtime_string_arg(&values[1], runtime.heap(), "file.open mode")?;
    let file = open_with_mode(&path, &mode)?;
    Ok(resource_value("File", ResourceHandle::File(file), runtime.heap_mut()))
}

fn create(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "file.create()")?;
    let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.create path")?;
    let file =
        std::fs::File::create(path.as_ref()).map_err(|err| anyhow!("failed to create file '{}': {err}", path))?;
    Ok(resource_value("File", ResourceHandle::File(file), runtime.heap_mut()))
}

fn write_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    io_std::write(args, runtime)
}

fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "file.close()")?;
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
