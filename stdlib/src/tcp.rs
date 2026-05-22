use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry},
    val::{RuntimeVal, Val},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32},
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
pub struct TcpModule {
    functions: HashMap<String, Val>,
}

impl Default for TcpModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        register_native(&mut functions, "connect", connect32, 2);
        register_native(&mut functions, "bind", bind32, 2);
        register_native(&mut functions, "close", close32, 1);
        register_native(&mut functions, "read", read32, lk_core::vm::NativeEntry32::VARIADIC);
        register_native(&mut functions, "write", write32, 2);
        register_native(&mut functions, "accept", accept32, 1);
        Self { functions }
    }
}

impl Module for TcpModule {
    fn name(&self) -> &str {
        "tcp"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn register_native(
    functions: &mut HashMap<String, Val>,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    functions.insert(
        name.to_string(),
        Val::runtime_native32(NativeFunction32::Plain(function), arity),
    );
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
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

fn connect32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "connect")?;
    let values = args.as_slice();
    let host = runtime_string_arg(&values[0], &runtime.state.heap, "connect host")?;
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

fn bind32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "bind")?;
    let values = args.as_slice();
    let host = runtime_string_arg(&values[0], &runtime.state.heap, "bind host")?;
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

fn accept32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

fn read32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

fn write32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "write")?;
    let values = args.as_slice();
    let conn_id = positive_id(&values[0], "Connection ID")?;
    let data = runtime_string_arg(&values[1], &runtime.state.heap, "write data")?;
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

fn close32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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
    use lk_core::{
        module::Module,
        val::{CallableValue, HeapStore, HeapValue},
        vm::RuntimeModuleState32,
    };

    fn tcp_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = TcpModule::new().exports();
        let value = exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?;
        let Val::Obj(object) = value else {
            bail!("{name} must be a heap callable");
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            bail!("{name} must be RuntimeNative32");
        };
        Ok((*arity, function.clone()))
    }

    fn call(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState32) -> Result<RuntimeVal> {
        let (_, function) = tcp_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative32");
        };
        let mut runtime = NativeRuntime32 {
            state,
            ctx: None,
            module: None,
        };
        function(NativeArgs32::new(args), &mut runtime)
    }

    #[test]
    fn tcp_exports_use_runtime_native32() -> Result<()> {
        for name in ["connect", "bind", "close", "read", "write", "accept"] {
            let (_, function) = tcp_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
        assert_eq!(tcp_native("read")?.0, lk_core::vm::NativeEntry32::VARIADIC);
        Ok(())
    }

    #[test]
    fn tcp_argument_validation_uses_runtime_values() {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let err = call("connect", &[], &mut state).expect_err("connect arity should fail");
        assert!(err.to_string().contains("requires 2 arguments"));
        let err = call("bind", &[], &mut state).expect_err("bind arity should fail");
        assert!(err.to_string().contains("requires 2 arguments"));
        let err = call("close", &[], &mut state).expect_err("close arity should fail");
        assert!(err.to_string().contains("requires 1 argument"));
    }

    #[test]
    fn tcp_close_unknown_id_returns_false() -> Result<()> {
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        assert_eq!(
            call("close", &[RuntimeVal::Int(i64::MAX)], &mut state)?,
            RuntimeVal::Bool(false)
        );
        Ok(())
    }
}
