use anyhow::{Result, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug, Default)]
pub struct UuidModule;

impl UuidModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for UuidModule {
    fn name(&self) -> &str {
        "uuid"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("v4", v4, 0),
                RuntimeNativeExport::plain("parse", parse, 1),
                RuntimeNativeExport::plain("is_valid", is_valid, 1),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("uuid", Box::new(UuidModule::new()))
}

fn v4(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "uuid.v4()")?;
    Ok(runtime_string_value(
        &uuid::Uuid::new_v4().to_string(),
        runtime.heap_mut(),
    ))
}

fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "uuid.parse()")?;
    let value = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "uuid.parse value")?;
    let parsed = uuid::Uuid::parse_str(value.as_ref()).map_err(|err| anyhow::anyhow!("invalid UUID: {err}"))?;
    Ok(runtime_string_value(&parsed.to_string(), runtime.heap_mut()))
}

fn is_valid(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "uuid.is_valid()")?;
    let value = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "uuid.is_valid value",
    )?;
    Ok(RuntimeVal::Bool(uuid::Uuid::parse_str(value.as_ref()).is_ok()))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
