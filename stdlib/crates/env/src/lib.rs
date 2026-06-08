use anyhow::{Result, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedMap},
    vm::{NativeArgs, RuntimeExport},
};
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
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("get", get, 1),
                RuntimeNativeExport::plain("get_or", get_or, 2),
                RuntimeNativeExport::plain("has", has, 1),
                RuntimeNativeExport::plain("vars", vars, 0),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("env", Box::new(EnvModule::new()))
}

fn get(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "env.get()")?;
    let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.get key")?;
    Ok(
        match std::env::var_os(key.as_ref()).and_then(|value| value.into_string().ok()) {
            Some(value) => runtime_string_value(&value, runtime.heap_mut()),
            None => RuntimeVal::Nil,
        },
    )
}

fn get_or(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "env.get_or()")?;
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
    expect_arity(args, 1, "env.has()")?;
    let key = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "env.has key")?;
    Ok(RuntimeVal::Bool(std::env::var_os(key.as_ref()).is_some()))
}

fn vars(args: NativeArgs<'_>, runtime: &mut lk_core::vm::NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "env.vars()")?;
    let mut map = fast_hash_map_new();
    for (key, value) in std::env::vars() {
        map.insert(Arc::<str>::from(key), runtime_string_value(&value, runtime.heap_mut()));
    }
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))),
    ))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
