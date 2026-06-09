use anyhow::Result;
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedMap},
    vm::{NativeArgs, RuntimeExport},
};
use lk_stdlib_common::metadata::StdlibModuleMetadata;
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct EnvModule;

impl EnvModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for EnvModule {
    fn name(&self) -> &str {
        "env"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "get" => get, 1,
                plain "get_or" => get_or, 2,
                plain "has" => has, 1,
                plain "vars" => vars, 0,
            ],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    lk_stdlib_common::metadata::register_stdlib_module_metadata(metadata())?;
    registry.register_module("env", Box::new(EnvModule::new()))
}

pub fn metadata() -> StdlibModuleMetadata {
    lk_stdlib_common::stdlib_module_metadata!(env, [get_or => String])
}

fn get(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "env.get()")?;
    let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.get key")?;
    Ok(
        match std::env::var_os(key.as_ref()).and_then(|value| value.into_string().ok()) {
            Some(value) => runtime_string_value(&value, runtime.heap_mut()),
            None => RuntimeVal::Nil,
        },
    )
}

fn get_or(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "env.get_or()")?;
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

fn has(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "env.has()")?;
    let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.has key")?;
    Ok(RuntimeVal::Bool(std::env::var_os(key.as_ref()).is_some()))
}

fn vars(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 0, "env.vars()")?;
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
