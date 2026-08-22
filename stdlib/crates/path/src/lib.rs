use anyhow::{Result, bail};
use lk_core::{
    val::{HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::{path::Path, sync::Arc};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "path", docs = "Path manipulation helpers")]
pub struct PathModule;

#[lk_stdlib_common::stdlib_exports]
impl PathModule {
    #[stdlib_export(name = "join", params(first: String, ...rest: String), returns = String, docs = "Joins path components using the platform path separator.")]
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

    #[stdlib_export(name = "parent", params(path: String), returns = String?)]
    fn parent(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        path_part(args, runtime, "path.parent()", |path| {
            path.parent().map(|value| value.to_string_lossy().to_string())
        })
    }

    #[stdlib_export(name = "file_name", params(path: String), returns = String?)]
    fn file_name(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        path_part(args, runtime, "path.file_name()", |path| {
            path.file_name().map(|value| value.to_string_lossy().to_string())
        })
    }

    #[stdlib_export(name = "file_stem", params(path: String), returns = String?)]
    fn file_stem(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        path_part(args, runtime, "path.file_stem()", |path| {
            path.file_stem().map(|value| value.to_string_lossy().to_string())
        })
    }

    #[stdlib_export(name = "extension", params(path: String), returns = String?)]
    fn extension(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        path_part(args, runtime, "path.extension()", |path| {
            path.extension().map(|value| value.to_string_lossy().to_string())
        })
    }

    #[stdlib_export(name = "with_extension", params(path: String, ext: String), returns = String)]
    fn with_extension(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.with_extension path")?;
        let ext = string_arg(args.get(1).expect("checked arity"), runtime, "path.with_extension ext")?;
        let path = Path::new(path.as_ref()).with_extension(ext.as_ref());
        Ok(runtime_string_value(&path.to_string_lossy(), runtime.heap_mut()))
    }

    #[stdlib_export(name = "is_absolute", params(path: String), returns = Bool)]
    fn is_absolute(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.is_absolute path")?;
        Ok(RuntimeVal::Bool(Path::new(path.as_ref()).is_absolute()))
    }

    #[stdlib_export(name = "normalize", params(path: String), returns = String)]
    fn normalize(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.normalize path")?;
        let path = Path::new(path.as_ref());
        // A `..` that cannot be popped is kept only when the path is
        // *relative*, where it still means something: `a/../../b` really does
        // go up past where it started, so `../b` is the answer. Above a root it
        // means nothing — `/..` is `/` on every filesystem — and keeping it
        // produced `/../a`, a path that normalizes to itself forever.
        let rooted = path.has_root();
        let mut out = std::path::PathBuf::new();
        for component in path.components() {
            use std::path::Component;
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    // Only a *named* component is what `..` cancels. Popping
                    // whatever was last meant one `..` ate another:
                    // `normalize("../..")` answered the empty string, so two
                    // levels up became none at all.
                    if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                        out.pop();
                    } else if !rooted {
                        out.push(component.as_os_str());
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        Ok(runtime_string_value(&out.to_string_lossy(), runtime.heap_mut()))
    }

    #[stdlib_export(name = "components", params(path: String), returns = List)]
    fn components(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let path = string_arg(args.get(0).expect("checked arity"), runtime, "path.components path")?;
        let values = Path::new(path.as_ref())
            .components()
            .map(|component| Arc::<str>::from(component.as_os_str().to_string_lossy().as_ref()))
            .collect();
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(values))),
        ))
    }

    #[stdlib_export(name = "sep", params(), returns = String)]
    fn sep(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(runtime_string_value(std::path::MAIN_SEPARATOR_STR, runtime.heap_mut()))
    }

    #[stdlib_export(name = "delimiter", params(), returns = String)]
    fn delimiter(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(runtime_string_value(
            if cfg!(windows) { ";" } else { ":" },
            runtime.heap_mut(),
        ))
    }
}

fn path_part(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
    name: &str,
    f: impl FnOnce(&Path) -> Option<String>,
) -> Result<RuntimeVal> {
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
