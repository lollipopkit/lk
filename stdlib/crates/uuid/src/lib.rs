use anyhow::Result;
use lk_core::{
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "uuid", docs = "UUID generation and validation helpers")]
pub struct UuidModule;

#[lk_stdlib_common::stdlib_exports(module = "uuid")]
impl UuidModule {
    #[stdlib_export(name = "v4", params(), returns = String)]
    fn v4(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(runtime_string_value(
            &uuid::Uuid::new_v4().to_string(),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(name = "parse", params(value: String), returns = String)]
    fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "uuid.parse value")?;
        let parsed = uuid::Uuid::parse_str(value.as_ref()).map_err(|err| anyhow::anyhow!("invalid UUID: {err}"))?;
        Ok(runtime_string_value(&parsed.to_string(), runtime.heap_mut()))
    }

    #[stdlib_export(name = "is_valid", params(value: String), returns = Bool)]
    fn is_valid(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "uuid.is_valid value",
        )?;
        Ok(RuntimeVal::Bool(uuid::Uuid::parse_str(value.as_ref()).is_ok()))
    }
}
