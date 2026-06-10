use anyhow::Result;
use lk_core::{
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedMap},
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::sync::Arc;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "env", docs = "Environment variable helpers")]
pub struct EnvModule;

#[lk_stdlib_common::stdlib_exports]
impl EnvModule {
    #[stdlib_export(name = "get", params(key: String), returns = String?, docs = "Returns an environment variable, or nil if it is not set.")]
    fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.get key")?;
        Ok(
            match std::env::var_os(key.as_ref()).and_then(|value| value.into_string().ok()) {
                Some(value) => runtime_string_value(&value, runtime.heap_mut()),
                None => RuntimeVal::Nil,
            },
        )
    }

    #[stdlib_export(name = "get_or", params(key: String, default: String), returns = String, docs = "Returns an environment variable, or the provided default value.")]
    fn get_or(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.get_or key")?;
        let default = runtime_string_arg(
            args.get(1).expect("checked arity"),
            runtime.heap(),
            "env.get_or default",
        )?;
        let value = std::env::var_os(key.as_ref())
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| default.to_string());
        Ok(runtime_string_value(&value, runtime.heap_mut()))
    }

    #[stdlib_export(name = "has", params(key: String), returns = Bool, docs = "Returns true when the environment variable is set.")]
    fn has(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.has key")?;
        Ok(RuntimeVal::Bool(std::env::var_os(key.as_ref()).is_some()))
    }

    #[stdlib_export(name = "vars", params(), returns = Map, docs = "Returns all environment variables as a map.")]
    fn vars(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let mut map = fast_hash_map_new();
        for (key, value) in std::env::vars_os() {
            let key = key.to_string_lossy();
            let value = value.to_string_lossy();
            map.insert(
                Arc::<str>::from(key.as_ref()),
                runtime_string_value(&value, runtime.heap_mut()),
            );
        }
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))),
        ))
    }
}
