use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener as StdTcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

static TCP_REGISTRY: OnceLock<Arc<Mutex<TcpRegistry>>> = OnceLock::new();

#[derive(Debug)]
struct TcpRegistry {
    connections: HashMap<u64, TcpStream>,
    listeners: HashMap<u64, StdTcpListener>,
    next_id: u64,
}

impl TcpRegistry {
    fn new() -> Self {
        Self {
            connections: HashMap::new(),
            listeners: HashMap::new(),
            next_id: 1,
        }
    }

    fn global() -> Arc<Mutex<TcpRegistry>> {
        TCP_REGISTRY
            .get_or_init(|| Arc::new(Mutex::new(TcpRegistry::new())))
            .clone()
    }
}

#[derive(Debug)]
pub struct TcpModule;

impl Default for TcpModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for TcpModule {
    fn name(&self) -> &str {
        "tcp"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("connect", connect, 2),
                RuntimeNativeExport::plain("bind", bind, 2),
                RuntimeNativeExport::plain("close", close, 1),
                RuntimeNativeExport::plain("read", read, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("write", write, 2),
                RuntimeNativeExport::plain("accept", accept, 1),
            ],
            &[],
        ))
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }
    bail!(
        "{name} requires {expected} argument{}",
        if expected == 1 { "" } else { "s" }
    )
}

fn positive_id(value: &RuntimeVal, name: &str) -> Result<u64> {
    match value {
        RuntimeVal::Int(value) if *value > 0 => Ok(*value as u64),
        other => Err(anyhow!("{name} must be a positive integer, got {:?}", other.kind())),
    }
}

fn port_arg(value: &RuntimeVal, name: &str) -> Result<u16> {
    match value {
        RuntimeVal::Int(value) if *value > 0 && *value <= 65535 => Ok(*value as u16),
        other => Err(anyhow!(
            "{name} must be a valid integer between 1 and 65535, got {:?}",
            other.kind()
        )),
    }
}

fn connect(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "connect")?;
    let values = args.as_slice();
    let host = runtime_string_arg(&values[0], runtime.heap(), "connect host")?;
    let port = port_arg(&values[1], "Port")?;
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(&addr).map_err(|err| anyhow!("Failed to connect to {addr}: {err}"))?;
    let registry = TcpRegistry::global();
    let mut registry = registry.lock().map_err(|_| anyhow!("TCP registry poisoned"))?;
    let id = registry.next_id;
    registry.next_id += 1;
    registry.connections.insert(id, stream);
    Ok(RuntimeVal::Int(id as i64))
}

fn bind(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "bind")?;
    let values = args.as_slice();
    let host = runtime_string_arg(&values[0], runtime.heap(), "bind host")?;
    let port = port_arg(&values[1], "Port")?;
    let addr = format!("{}:{}", host, port);
    let listener = StdTcpListener::bind(&addr).map_err(|err| anyhow!("Failed to bind to {addr}: {err}"))?;
    let registry = TcpRegistry::global();
    let mut registry = registry.lock().map_err(|_| anyhow!("TCP registry poisoned"))?;
    let id = registry.next_id;
    registry.next_id += 1;
    registry.listeners.insert(id, listener);
    Ok(RuntimeVal::Int(id as i64))
}

fn accept(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "accept")?;
    let listener_id = positive_id(args.get(0).expect("checked arity"), "Listener ID")?;
    let registry = TcpRegistry::global();
    let mut registry = registry.lock().map_err(|_| anyhow!("TCP registry poisoned"))?;
    let listener = registry
        .listeners
        .get(&listener_id)
        .ok_or_else(|| anyhow!("Invalid listener ID: {listener_id}"))?;
    let (stream, _) = listener
        .accept()
        .map_err(|err| anyhow!("Failed to accept connection: {err}"))?;
    let id = registry.next_id;
    registry.next_id += 1;
    registry.connections.insert(id, stream);
    Ok(RuntimeVal::Int(id as i64))
}

fn read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.is_empty() || args.len() > 2 {
        bail!("read requires 1-2 arguments: connection_id, [max_bytes]");
    }
    let values = args.as_slice();
    let conn_id = positive_id(&values[0], "Connection ID")?;
    let max_bytes = if let Some(value) = values.get(1) {
        positive_id(value, "max_bytes")? as usize
    } else {
        4096
    };
    let registry = TcpRegistry::global();
    let mut registry = registry.lock().map_err(|_| anyhow!("TCP registry poisoned"))?;
    let stream = registry
        .connections
        .get_mut(&conn_id)
        .ok_or_else(|| anyhow!("Invalid connection ID: {conn_id}"))?;
    let mut buffer = vec![0u8; max_bytes];
    let bytes_read = stream
        .read(&mut buffer)
        .map_err(|err| anyhow!("Failed to read from connection: {err}"))?;
    buffer.truncate(bytes_read);
    let data = String::from_utf8(buffer).map_err(|_| anyhow!("Data is not valid UTF-8"))?;
    Ok(runtime_string_value(&data, runtime.heap_mut()))
}

fn write(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "write")?;
    let values = args.as_slice();
    let conn_id = positive_id(&values[0], "Connection ID")?;
    let data = runtime_string_arg(&values[1], runtime.heap(), "write data")?;
    let registry = TcpRegistry::global();
    let mut registry = registry.lock().map_err(|_| anyhow!("TCP registry poisoned"))?;
    let stream = registry
        .connections
        .get_mut(&conn_id)
        .ok_or_else(|| anyhow!("Invalid connection ID: {conn_id}"))?;
    let bytes_written = stream
        .write(data.as_bytes())
        .map_err(|err| anyhow!("Failed to write to connection: {err}"))?;
    Ok(RuntimeVal::Int(bytes_written as i64))
}

fn close(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "close")?;
    let id = positive_id(args.get(0).expect("checked arity"), "ID")?;
    let registry = TcpRegistry::global();
    let mut registry = registry.lock().map_err(|_| anyhow!("TCP registry poisoned"))?;
    let closed = registry.connections.remove(&id).is_some() || registry.listeners.remove(&id).is_some();
    Ok(RuntimeVal::Bool(closed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::vm::{NativeFunction, RuntimeModuleState};

    fn tcp_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&TcpModule::new(), name)
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState) -> Result<RuntimeVal> {
        let (_, function) = tcp_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    #[test]
    fn tcp_exports_use_runtime_native() -> Result<()> {
        for name in ["connect", "bind", "close", "read", "write", "accept"] {
            let (_, function) = tcp_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        assert_eq!(tcp_native("read")?.0, lk_core::vm::NativeEntry::VARIADIC);
        Ok(())
    }

    #[test]
    fn tcp_argument_validation_uses_runtime_values() {
        let mut state = RuntimeModuleState::default();
        let err = call("connect", &[], &mut state).expect_err("connect arity should fail");
        assert!(err.to_string().contains("requires 2 arguments"));
        let err = call("bind", &[], &mut state).expect_err("bind arity should fail");
        assert!(err.to_string().contains("requires 2 arguments"));
        let err = call("close", &[], &mut state).expect_err("close arity should fail");
        assert!(err.to_string().contains("requires 1 argument"));
    }

    #[test]
    fn tcp_close_unknown_id_returns_false() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        assert_eq!(
            call("close", &[RuntimeVal::Int(i64::MAX)], &mut state)?,
            RuntimeVal::Bool(false)
        );
        Ok(())
    }
}
