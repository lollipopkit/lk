use anyhow::{Result, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};

use crate::{
    resource::{close_resource, resource_arg},
    runtime_native::{runtime_string_arg, runtime_string_value},
};

#[derive(Debug)]
pub struct NetSocketModule;

impl NetSocketModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NetSocketModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for NetSocketModule {
    fn name(&self) -> &str {
        "socket"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "addr" => addr, 2,
                plain "close" => close, 1,
            ],
        ))
    }
}

fn addr(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "socket.addr()")?;
    let values = args.as_slice();
    let host = runtime_string_arg(&values[0], runtime.heap(), "socket.addr host")?;
    let port = match &values[1] {
        RuntimeVal::Int(value) if *value >= 0 && *value <= 65535 => *value as u16,
        other => bail!("socket.addr port expects integer 0..65535, got {:?}", other.kind()),
    };
    Ok(runtime_string_value(&format!("{host}:{port}"), runtime.heap_mut()))
}

fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "socket.close()")?;
    let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "socket.close()")?;
    Ok(RuntimeVal::Bool(close_resource(&resource)?))
}
