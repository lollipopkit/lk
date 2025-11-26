use anyhow::{Result, anyhow};
use lkr_core::{module::Module, val::Val, vm::VmContext};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener as StdTcpListener, TcpStream};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

/// Global registry to keep track of TCP connections and listeners by ID
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

    fn get_global() -> Arc<Mutex<TcpRegistry>> {
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

        // Connection management
        functions.insert("connect".to_string(), Val::RustFunction(Self::connect));
        functions.insert("bind".to_string(), Val::RustFunction(Self::bind));
        functions.insert("close".to_string(), Val::RustFunction(Self::close));
        functions.insert("read".to_string(), Val::RustFunction(Self::read));
        functions.insert("write".to_string(), Val::RustFunction(Self::write));
        functions.insert("accept".to_string(), Val::RustFunction(Self::accept));

        TcpModule { functions }
    }

    /// Connect to a TCP server: tcp.connect(host, port) -> connection_id
    fn connect(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("connect requires 2 arguments: host, port"));
        }

        let host = match &args[0] {
            Val::Str(s) => s.as_ref(),
            _ => return Err(anyhow!("Host must be a string")),
        };

        let port = match &args[1] {
            Val::Int(i) if *i > 0 && *i <= 65535 => *i as u16,
            _ => return Err(anyhow!("Port must be a valid integer between 1 and 65535")),
        };

        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr).map_err(|e| anyhow!("Failed to connect to {}: {}", addr, e))?;

        let registry = TcpRegistry::get_global();
        let mut registry = registry.lock().unwrap();
        let id = registry.next_id;
        registry.next_id += 1;
        registry.connections.insert(id, stream);

        Ok(Val::Int(id as i64))
    }

    /// Bind a TCP listener: tcp.bind(host, port) -> listener_id
    fn bind(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("bind requires 2 arguments: host, port"));
        }

        let host = match &args[0] {
            Val::Str(s) => s.as_ref(),
            _ => return Err(anyhow!("Host must be a string")),
        };

        let port = match &args[1] {
            Val::Int(i) if *i > 0 && *i <= 65535 => *i as u16,
            _ => return Err(anyhow!("Port must be a valid integer between 1 and 65535")),
        };

        let addr = format!("{}:{}", host, port);
        let listener = StdTcpListener::bind(&addr).map_err(|e| anyhow!("Failed to bind to {}: {}", addr, e))?;

        let registry = TcpRegistry::get_global();
        let mut registry = registry.lock().unwrap();
        let id = registry.next_id;
        registry.next_id += 1;
        registry.listeners.insert(id, listener);

        Ok(Val::Int(id as i64))
    }

    /// Accept a connection from a listener: tcp.accept(listener_id) -> connection_id
    fn accept(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("accept requires 1 argument: listener_id"));
        }

        let listener_id = match &args[0] {
            Val::Int(i) if *i > 0 => *i as u64,
            _ => return Err(anyhow!("Listener ID must be a positive integer")),
        };

        let registry = TcpRegistry::get_global();
        let mut registry = registry.lock().unwrap();

        let listener = registry
            .listeners
            .get(&listener_id)
            .ok_or_else(|| anyhow!("Invalid listener ID: {}", listener_id))?;

        // This is a blocking accept - in a real implementation you might want to make this configurable
        let (stream, _) = listener
            .accept()
            .map_err(|e| anyhow!("Failed to accept connection: {}", e))?;

        let id = registry.next_id;
        registry.next_id += 1;
        registry.connections.insert(id, stream);

        Ok(Val::Int(id as i64))
    }

    /// Read data from a connection: tcp.read(connection_id, [max_bytes]) -> string
    fn read(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.is_empty() || args.len() > 2 {
            return Err(anyhow!("read requires 1-2 arguments: connection_id, [max_bytes]"));
        }

        let conn_id = match &args[0] {
            Val::Int(i) if *i > 0 => *i as u64,
            _ => return Err(anyhow!("Connection ID must be a positive integer")),
        };

        let max_bytes = if args.len() > 1 {
            match &args[1] {
                Val::Int(i) if *i > 0 => *i as usize,
                _ => return Err(anyhow!("max_bytes must be a positive integer")),
            }
        } else {
            4096
        };

        let registry = TcpRegistry::get_global();
        let mut registry = registry.lock().unwrap();

        let stream = registry
            .connections
            .get_mut(&conn_id)
            .ok_or_else(|| anyhow!("Invalid connection ID: {}", conn_id))?;

        let mut buffer = vec![0u8; max_bytes];
        let bytes_read = stream
            .read(&mut buffer)
            .map_err(|e| anyhow!("Failed to read from connection: {}", e))?;

        buffer.truncate(bytes_read);
        let data = String::from_utf8(buffer).map_err(|_| anyhow!("Data is not valid UTF-8"))?;

        Ok(Val::Str(data.into()))
    }

    /// Write data to a connection: tcp.write(connection_id, data) -> bytes_written
    fn write(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("write requires 2 arguments: connection_id, data"));
        }

        let conn_id = match &args[0] {
            Val::Int(i) if *i > 0 => *i as u64,
            _ => return Err(anyhow!("Connection ID must be a positive integer")),
        };

        let data_string;
        let data = match &args[1] {
            Val::Str(s) => s.as_bytes(),
            v => {
                data_string = v.to_string();
                data_string.as_bytes()
            }
        };

        let registry = TcpRegistry::get_global();
        let mut registry = registry.lock().unwrap();

        let stream = registry
            .connections
            .get_mut(&conn_id)
            .ok_or_else(|| anyhow!("Invalid connection ID: {}", conn_id))?;

        let bytes_written = stream
            .write(data)
            .map_err(|e| anyhow!("Failed to write to connection: {}", e))?;

        Ok(Val::Int(bytes_written as i64))
    }

    /// Close a connection or listener: tcp.close(id) -> bool
    fn close(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("close requires 1 argument: id"));
        }

        let id = match &args[0] {
            Val::Int(i) if *i > 0 => *i as u64,
            _ => return Err(anyhow!("ID must be a positive integer")),
        };

        let registry = TcpRegistry::get_global();
        let mut registry = registry.lock().unwrap();

        let closed = registry.connections.remove(&id).is_some() || registry.listeners.remove(&id).is_some();

        Ok(Val::Bool(closed))
    }
}

impl Module for TcpModule {
    fn name(&self) -> &str {
        "tcp"
    }

    fn register(&self, _registry: &mut lkr_core::module::ModuleRegistry) -> Result<()> {
        // Don't register functions globally - they should be accessed via module.function()
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_module_functions() {
        let module = TcpModule::new();
        let exports = module.exports();
        assert!(exports.contains_key("connect"));
        assert!(exports.contains_key("bind"));
        assert!(exports.contains_key("close"));
        assert!(exports.contains_key("read"));
        assert!(exports.contains_key("write"));
        assert!(exports.contains_key("accept"));
    }
}
