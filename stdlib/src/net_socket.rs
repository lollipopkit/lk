use anyhow::{Result, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
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
        "net/socket"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("addr", addr, 2),
                RuntimeNativeExport::plain("close", close, 1),
            ],
            &[],
        ))
    }
}

fn addr(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "socket.addr()")?;
    let values = args.as_slice();
    let host = runtime_string_arg(&values[0], runtime.heap(), "socket.addr host")?;
    let port = match &values[1] {
        RuntimeVal::Int(value) if *value >= 0 && *value <= 65535 => *value as u16,
        other => bail!("socket.addr port expects integer 0..65535, got {:?}", other.kind()),
    };
    Ok(runtime_string_value(&format!("{host}:{port}"), runtime.heap_mut()))
}

fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "socket.close()")?;
    let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "socket.close()")?;
    Ok(RuntimeVal::Bool(close_resource(&resource)?))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
