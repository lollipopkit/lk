use anyhow::{Result, anyhow};
use lk_core::{
    util::fast_map::fast_hash_map_new,
    val::{HeapStore, HeapValue, RuntimeVal, TypedList, TypedMap},
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_bytes::{runtime_bytes_or_string_arg, runtime_bytes_value};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::sync::Arc;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "fs", docs = "Filesystem helpers")]
pub struct FsModule;

#[lk_stdlib_common::stdlib_exports(module = "fs")]
impl FsModule {
    #[stdlib_export(params(path: String), returns = Bytes, docs = "Reads a file as bytes.")]
    fn read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.read path")?;
        let data = std::fs::read(path.as_ref()).map_err(|err| anyhow!("failed to read file '{}': {err}", path))?;
        Ok(runtime_bytes_value(data, runtime.heap_mut()))
    }

    #[stdlib_export(params(path: String), returns = String, docs = "Reads a UTF-8 text file.")]
    fn read_to_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.read_to_string path")?;
        let data =
            std::fs::read_to_string(path.as_ref()).map_err(|err| anyhow!("failed to read file '{}': {err}", path))?;
        Ok(runtime_string_value(&data, runtime.heap_mut()))
    }

    #[stdlib_export(params(path: String, data: Bytes | String), returns = Bool, docs = "Writes bytes or text to a file.")]
    fn write(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.write path")?;
        let data = runtime_bytes_or_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "fs.write data")?;
        std::fs::write(path.as_ref(), &data).map_err(|err| anyhow!("failed to write file '{}': {err}", path))?;
        Ok(RuntimeVal::Bool(true))
    }

    #[stdlib_export(params(path: String, data: Bytes | String), returns = Bool)]
    fn append(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        use std::io::Write;
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

    #[stdlib_export(params(path: String), returns = Bool)]
    fn exists(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.exists path")?;
        Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).exists()))
    }

    #[stdlib_export(params(path: String), returns = Bool)]
    fn is_file(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.is_file path")?;
        Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).is_file()))
    }

    #[stdlib_export(params(path: String), returns = Bool)]
    fn is_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.is_dir path")?;
        Ok(RuntimeVal::Bool(std::path::Path::new(path.as_ref()).is_dir()))
    }

    #[stdlib_export(name = "metadata", params(path: String), returns = Map)]
    fn metadata(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    #[stdlib_export(params(path: String), returns = List[String])]
    fn read_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    #[stdlib_export(params(path: String), returns = Bool)]
    fn create_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.create_dir path")?;
        std::fs::create_dir(path.as_ref()).map_err(|err| anyhow!("failed to create directory '{}': {err}", path))?;
        Ok(RuntimeVal::Bool(true))
    }

    #[stdlib_export(params(path: String), returns = Bool)]
    fn create_dir_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.create_dir_all path")?;
        std::fs::create_dir_all(path.as_ref())
            .map_err(|err| anyhow!("failed to create directory '{}': {err}", path))?;
        Ok(RuntimeVal::Bool(true))
    }

    #[stdlib_export(params(path: String), returns = Bool)]
    fn remove_file(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.remove_file path")?;
        remove_path(path.as_ref(), |path| std::fs::remove_file(path))
    }

    #[stdlib_export(params(path: String), returns = Bool)]
    fn remove_dir(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.remove_dir path")?;
        remove_path(path.as_ref(), |path| std::fs::remove_dir(path))
    }

    #[stdlib_export(params(path: String), returns = Bool)]
    fn remove_dir_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.remove_dir_all path")?;
        remove_path(path.as_ref(), |path| std::fs::remove_dir_all(path))
    }

    #[stdlib_export(params(from: String, to: String), returns = Bool)]
    fn rename(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let from = path_arg(args.get(0).expect("checked arity"), runtime, "fs.rename from")?;
        let to = path_arg(args.get(1).expect("checked arity"), runtime, "fs.rename to")?;
        std::fs::rename(from.as_ref(), to.as_ref()).map_err(|err| anyhow!("failed to rename '{}': {err}", from))?;
        Ok(RuntimeVal::Bool(true))
    }

    #[stdlib_export(params(from: String, to: String), returns = Bool)]
    fn copy(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let from = path_arg(args.get(0).expect("checked arity"), runtime, "fs.copy from")?;
        let to = path_arg(args.get(1).expect("checked arity"), runtime, "fs.copy to")?;
        let copied =
            std::fs::copy(from.as_ref(), to.as_ref()).map_err(|err| anyhow!("failed to copy '{}': {err}", from))?;
        Ok(RuntimeVal::Int(copied as i64))
    }

    #[stdlib_export(params(path: String), returns = String)]
    fn canonicalize(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = path_arg(args.get(0).expect("checked arity"), runtime, "fs.canonicalize path")?;
        let path =
            std::fs::canonicalize(path.as_ref()).map_err(|err| anyhow!("failed to canonicalize '{}': {err}", path))?;
        path_value(path, runtime.heap_mut())
    }

    #[stdlib_export(params(), returns = String)]
    fn temp_dir(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        path_value(std::env::temp_dir(), runtime.heap_mut())
    }
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
