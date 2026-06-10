use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{ResourceHandle, RuntimeVal},
    vm::{NativeArgs, NativeRuntime},
};
use std::fs::OpenOptions;

use crate::std_io as io_std;
use crate::{
    resource::{close_resource, resource_arg, resource_value},
    runtime_native::runtime_string_arg,
};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "file", docs = "File resource helpers")]
pub struct IoFileModule;

#[lk_stdlib_common::stdlib_exports(module = "io.file")]
impl IoFileModule {
    #[stdlib_export(params(path: String, mode: String), returns = Resource)]
    fn open(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let path = runtime_string_arg(&values[0], runtime.heap(), "file.open path")?;
        let mode = runtime_string_arg(&values[1], runtime.heap(), "file.open mode")?;
        let file = open_with_mode(&path, &mode)?;
        Ok(resource_value("File", ResourceHandle::File(file), runtime.heap_mut()))
    }

    #[stdlib_export(params(path: String), returns = Resource)]
    fn create(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.create path")?;
        let file =
            std::fs::File::create(path.as_ref()).map_err(|err| anyhow!("failed to create file '{}': {err}", path))?;
        Ok(resource_value("File", ResourceHandle::File(file), runtime.heap_mut()))
    }

    #[stdlib_export(params(reader: Resource, max_bytes?: Int), returns = Bytes)]
    fn read_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::read(args, runtime)
    }

    #[stdlib_export(params(reader: Resource), returns = String)]
    fn read_to_string_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::read_to_string(args, runtime)
    }

    #[stdlib_export(params(reader: Resource), returns = String?)]
    fn read_line_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::read_line(args, runtime)
    }

    #[stdlib_export(params(writer: Resource, data: Bytes | String), returns = Int)]
    fn write_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::write(args, runtime)
    }

    #[stdlib_export(params(writer: Resource, data: Bytes | String), returns = Int)]
    fn writeln_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::writeln_fn(args, runtime)
    }

    #[stdlib_export(params(writer: Resource, data: Bytes | String), returns = Int)]
    fn write_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::write(args, runtime)
    }

    #[stdlib_export(params(writer: Resource), returns = Bool)]
    fn flush_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        io_std::flush(args, runtime)
    }

    #[stdlib_export(params(resource: Resource), returns = Bool)]
    fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "file.close()")?;
        Ok(RuntimeVal::Bool(close_resource(&resource)?))
    }
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
