use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    util::fast_map::fast_hash_map_new,
    val::{HeapStore, HeapValue, RuntimeVal, TypedList, TypedMap},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use lk_stdlib_bytes::{runtime_bytes_or_string_arg, runtime_bytes_value};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct FsModule;

impl FsModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for FsModule {
    fn name(&self) -> &str {
        "fs"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("read", read, 1),
                RuntimeNativeExport::plain("read_to_string", read_to_string, 1),
                RuntimeNativeExport::plain("write", write, 2),
                RuntimeNativeExport::plain("append", append, 2),
                RuntimeNativeExport::plain("exists", exists, 1),
                RuntimeNativeExport::plain("is_file", is_file, 1),
                RuntimeNativeExport::plain("is_dir", is_dir, 1),
                RuntimeNativeExport::plain("metadata", metadata, 1),
                RuntimeNativeExport::plain("read_dir", read_dir, 1),
                RuntimeNativeExport::plain("create_dir", create_dir, 1),
                RuntimeNativeExport::plain("create_dir_all", create_dir_all, 1),
                RuntimeNativeExport::plain("remove_file", remove_file, 1),
                RuntimeNativeExport::plain("remove_dir", remove_dir, 1),
                RuntimeNativeExport::plain("remove_dir_all", remove_dir_all, 1),
                RuntimeNativeExport::plain("rename", rename, 2),
                RuntimeNativeExport::plain("copy", copy, 2),
                RuntimeNativeExport::plain("canonicalize", canonicalize, 1),
                RuntimeNativeExport::plain("temp_dir", temp_dir, 0),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("fs", Box::new(FsModule::new()))
}

fn read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.read()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.read path")?;
    let data = std::fs::read(path.as_ref()).map_err(|err| anyhow!("failed to read file '{}': {err}", path))?;
    Ok(runtime_bytes_value(data, runtime.heap_mut()))
}

fn read_to_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.read_to_string()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.read_to_string path")?;
    let data =
        std::fs::read_to_string(path.as_ref()).map_err(|err| anyhow!("failed to read file '{}': {err}", path))?;
    Ok(runtime_string_value(&data, runtime.heap_mut()))
}

fn write(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "fs.write()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.write path")?;
    let data = runtime_bytes_or_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "fs.write data")?;
    std::fs::write(path.as_ref(), &data).map_err(|err| anyhow!("failed to write file '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn append(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    use std::io::Write;

    expect_arity(args, 2, "fs.append()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.append path")?;
    let data = runtime_bytes_or_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "fs.append data")?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_ref())
        .map_err(|err| anyhow!("failed to open file '{}': {err}", path))?;
    file.write_all(&data)
        .map_err(|err| anyhow!("failed to append file '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn exists(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.exists()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.exists path")?;
    Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).exists()))
}

fn is_file(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.is_file()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.is_file path")?;
    Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).is_file()))
}

fn is_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.is_dir()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.is_dir path")?;
    Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).is_dir()))
}

fn metadata(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.metadata()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.metadata path")?;
    let meta = std::fs::metadata(path.as_ref()).map_err(|err| anyhow!("failed to stat '{}': {err}", path))?;
    let mut map = fast_hash_map_new();
    map.insert(Arc::<str>::from("len"), RuntimeVal::Int(meta.len() as i64));
    map.insert(Arc::<str>::from("is_file"), RuntimeVal::Bool(meta.is_file()));
    map.insert(Arc::<str>::from("is_dir"), RuntimeVal::Bool(meta.is_dir()));
    map.insert(
        Arc::<str>::from("readonly"),
        RuntimeVal::Bool(meta.permissions().readonly()),
    );
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))),
    ))
}

fn read_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.read_dir()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.read_dir path")?;
    let mut entries = Vec::new();
    for entry in
        std::fs::read_dir(path.as_ref()).map_err(|err| anyhow!("failed to read directory '{}': {err}", path))?
    {
        let entry = entry.map_err(|err| anyhow!("failed to read directory entry '{}': {err}", path))?;
        if let Some(name) = entry.file_name().to_str() {
            entries.push(Arc::<str>::from(name));
        }
    }
    entries.sort();
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::List(TypedList::String(entries))),
    ))
}

fn create_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.create_dir()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.create_dir path")?;
    std::fs::create_dir(path.as_ref()).map_err(|err| anyhow!("failed to create directory '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn create_dir_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.create_dir_all()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.create_dir_all path")?;
    std::fs::create_dir_all(path.as_ref()).map_err(|err| anyhow!("failed to create directory '{}': {err}", path))?;
    Ok(RuntimeVal::Bool(true))
}

fn remove_file(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.remove_file()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.remove_file path")?;
    remove_path(path.as_ref(), |path| std::fs::remove_file(path))
}

fn remove_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.remove_dir()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.remove_dir path")?;
    remove_path(path.as_ref(), |path| std::fs::remove_dir(path))
}

fn remove_dir_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.remove_dir_all()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.remove_dir_all path")?;
    remove_path(path.as_ref(), |path| std::fs::remove_dir_all(path))
}

fn rename(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "fs.rename()")?;
    let from = path_arg(args.get(0).expect("checked arity"), runtime, "fs.rename from")?;
    let to = path_arg(args.get(1).expect("checked arity"), runtime, "fs.rename to")?;
    std::fs::rename(from.as_ref(), to.as_ref()).map_err(|err| anyhow!("failed to rename '{}': {err}", from))?;
    Ok(RuntimeVal::Bool(true))
}

fn copy(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "fs.copy()")?;
    let from = path_arg(args.get(0).expect("checked arity"), runtime, "fs.copy from")?;
    let to = path_arg(args.get(1).expect("checked arity"), runtime, "fs.copy to")?;
    let copied =
        std::fs::copy(from.as_ref(), to.as_ref()).map_err(|err| anyhow!("failed to copy '{}': {err}", from))?;
    Ok(RuntimeVal::Int(copied as i64))
}

fn canonicalize(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "fs.canonicalize()")?;
    let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.canonicalize path")?;
    let path =
        std::fs::canonicalize(path.as_ref()).map_err(|err| anyhow!("failed to canonicalize '{}': {err}", path))?;
    path_value(path, runtime.heap_mut())
}

fn temp_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "fs.temp_dir()")?;
    path_value(std::env::temp_dir(), runtime.heap_mut())
}

fn remove_path(path: &str, remove: impl FnOnce(&str) -> std::io::Result<()>) -> Result<RuntimeVal> {
    match remove(path) {
        Ok(()) => Ok(RuntimeVal::Bool(true)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(RuntimeVal::Bool(false)),
        Err(err) => Err(anyhow!("failed to remove '{}': {err}", path)),
    }
}

fn path_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, context: &str) -> Result<Arc<str>> {
    runtime_string_arg(value, runtime.heap(), context)
}

fn path_value(path: std::path::PathBuf, heap: &mut HeapStore) -> Result<RuntimeVal> {
    match path.into_os_string().into_string() {
        Ok(path) => Ok(runtime_string_value(&path, heap)),
        Err(_) => Ok(RuntimeVal::Nil),
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
