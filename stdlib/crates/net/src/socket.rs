use anyhow::{Result, bail};
use lk_core::{
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime},
};

use crate::{
    resource::{close_resource, resource_arg},
    runtime_native::{runtime_string_arg, runtime_string_value},
};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "socket", docs = "Socket address helpers")]
pub struct NetSocketModule;

#[lk_stdlib_common::stdlib_exports(module = "net.socket")]
impl NetSocketModule {
    #[stdlib_export(params(host: String, port: Int), returns = String)]
    fn addr(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let host = runtime_string_arg(&values[0], runtime.heap(), "socket.addr host")?;
        let port = match &values[1] {
            RuntimeVal::Int(value) if *value >= 0 && *value <= 65535 => *value as u16,
            other => bail!("socket.addr port expects integer 0..65535, got {:?}", other.kind()),
        };
        Ok(runtime_string_value(&format!("{host}:{port}"), runtime.heap_mut()))
    }

    #[stdlib_export(params(resource: Resource), returns = Bool)]
    fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "socket.close()")?;
        Ok(RuntimeVal::Bool(close_resource(&resource)?))
    }
}
