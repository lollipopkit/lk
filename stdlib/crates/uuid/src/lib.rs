use anyhow::Result;
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
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
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "v4" => v4, 0,
                plain "parse" => parse, 1,
                plain "is_valid" => is_valid, 1,
            ],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("uuid", Box::new(UuidModule::new()))
}

fn v4(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 0, "uuid.v4()")?;
    Ok(runtime_string_value(
        &uuid::Uuid::new_v4().to_string(),
        runtime.heap_mut(),
    ))
}

fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "uuid.parse()")?;
    let value = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "uuid.parse value")?;
    let parsed = uuid::Uuid::parse_str(value.as_ref()).map_err(|err| anyhow::anyhow!("invalid UUID: {err}"))?;
    Ok(runtime_string_value(&parsed.to_string(), runtime.heap_mut()))
}

fn is_valid(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "uuid.is_valid()")?;
    let value = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "uuid.is_valid value",
    )?;
    Ok(RuntimeVal::Bool(uuid::Uuid::parse_str(value.as_ref()).is_ok()))
}
