use anyhow::{Result, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::{HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::{path::Path, sync::Arc};

#[derive(Debug, Default)]
pub struct PathModule;

impl PathModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for PathModule {
    fn name(&self) -> &str {
        "path"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("join", join, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("parent", parent, 1),
                RuntimeNativeExport::plain("file_name", file_name, 1),
                RuntimeNativeExport::plain("file_stem", file_stem, 1),
                RuntimeNativeExport::plain("extension", extension, 1),
                RuntimeNativeExport::plain("with_extension", with_extension, 2),
                RuntimeNativeExport::plain("is_absolute", is_absolute, 1),
                RuntimeNativeExport::plain("normalize", normalize, 1),
                RuntimeNativeExport::plain("components", components, 1),
                RuntimeNativeExport::plain("sep", sep, 0),
                RuntimeNativeExport::plain("delimiter", delimiter, 0),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("path", Box::new(PathModule::new()))
}

fn join(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.is_empty() {
        bail!("path.join() requires at least 1 argument");
    }
    let mut path = std::path::PathBuf::new();
    for value in args.as_slice() {
        let component = string_arg(value, runtime, "path.join component")?;
        path.push(component.as_ref());
    }
    Ok(runtime_string_value(&path.to_string_lossy(), runtime.heap_mut()))
}

fn parent(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    path_part(args, runtime, "path.parent()", |path| {
        path.parent().map(|value| value.to_string_lossy().to_string())
    })
}

fn file_name(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    path_part(args, runtime, "path.file_name()", |path| {
        path.file_name().map(|value| value.to_string_lossy().to_string())
    })
}

fn file_stem(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    path_part(args, runtime, "path.file_stem()", |path| {
        path.file_stem().map(|value| value.to_string_lossy().to_string())
    })
}

fn extension(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    path_part(args, runtime, "path.extension()", |path| {
        path.extension().map(|value| value.to_string_lossy().to_string())
    })
}

fn with_extension(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "path.with_extension()")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.with_extension path")?;
    let ext = string_arg(args.get(1).expect("checked arity"), runtime, "path.with_extension ext")?;
    let path = Path::new(path.as_ref()).with_extension(ext.as_ref());
    Ok(runtime_string_value(&path.to_string_lossy(), runtime.heap_mut()))
}

fn is_absolute(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "path.is_absolute()")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.is_absolute path")?;
    Ok(RuntimeVal::Bool(Path::new(path.as_ref()).is_absolute()))
}

fn normalize(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "path.normalize()")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.normalize path")?;
    let mut out = std::path::PathBuf::new();
    for component in Path::new(path.as_ref()).components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(runtime_string_value(&out.to_string_lossy(), runtime.heap_mut()))
}

fn components(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "path.components()")?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.components path")?;
    let values = Path::new(path.as_ref())
        .components()
        .map(|component| Arc::<str>::from(component.as_os_str().to_string_lossy().as_ref()))
        .collect();
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::List(TypedList::String(values))),
    ))
}

fn sep(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "path.sep()")?;
    Ok(runtime_string_value(std::path::MAIN_SEPARATOR_STR, runtime.heap_mut()))
}

fn delimiter(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "path.delimiter()")?;
    Ok(runtime_string_value(
        if cfg!(windows) { ";" } else { ":" },
        runtime.heap_mut(),
    ))
}

fn path_part(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
    name: &str,
    f: impl FnOnce(&Path) -> Option<String>,
) -> Result<RuntimeVal> {
    expect_arity(args, 1, name)?;
    let path = string_arg(args.get(0).expect("checked arity"), runtime, name)?;
    let path = Path::new(path.as_ref());
    Ok(match f(path) {
        Some(value) => runtime_string_value(&value, runtime.heap_mut()),
        None => RuntimeVal::Nil,
    })
}

fn string_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, context: &str) -> Result<Arc<str>> {
    runtime_string_arg(value, runtime.heap(), context)
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
