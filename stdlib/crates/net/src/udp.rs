use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    rt::{self, RuntimePayload},
    util::fast_map::fast_hash_map_new,
    val::{HeapStore, HeapValue, ResourceHandle, RuntimeMapKey, RuntimeVal, TypedMap},
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use std::{net::UdpSocket, sync::Arc};

use crate::{
    bytes::{runtime_bytes_or_string_arg, runtime_bytes_value},
    resource::{payload_int, resource_arg, resource_value, task_value},
    runtime_native::{runtime_string_arg, runtime_string_value},
};

const MAX_DATAGRAM_READ_LIMIT: usize = 65_535;

#[derive(Debug)]
pub struct NetUdpModule;

impl NetUdpModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NetUdpModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for NetUdpModule {
    fn name(&self) -> &str {
        "udp"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("bind", bind, 1),
                RuntimeNativeExport::plain("recv_from", recv_from, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("send_to", send_to, 3),
                RuntimeNativeExport::plain("recv_from_task", recv_from_task, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("send_to_task", send_to_task, 3),
            ],
            &[],
        ))
    }
}

fn bind(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "udp.bind()")?;
    let addr = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "udp.bind addr")?;
    let socket = UdpSocket::bind(addr.as_ref()).map_err(|err| anyhow!("udp bind {addr}: {err}"))?;
    Ok(resource_value(
        "UdpSocket",
        ResourceHandle::UdpSocket(socket),
        runtime.heap_mut(),
    ))
}

fn recv_from(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (socket, max) = recv_args(args, runtime)?;
    let (data, addr) = recv_socket(socket, max)?;
    Ok(recv_result_value(data, addr, runtime.heap_mut()))
}

fn send_to(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "udp.send_to()")?;
    let values = args.as_slice();
    let socket = socket_clone(&values[0], runtime.heap(), "udp.send_to()")?;
    let data = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "udp.send_to data")?;
    let addr = runtime_string_arg(&values[2], runtime.heap(), "udp.send_to addr")?;
    send_socket(socket, &data, &addr)
}

fn recv_from_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (socket, max) = recv_args(args, runtime)?;
    spawn_task(runtime, async move {
        let (data, addr) = recv_socket(socket, max)?;
        Ok(payload_recv_result(data, addr))
    })
}

fn send_to_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "udp.send_to_task()")?;
    let values = args.as_slice();
    let socket = socket_clone(&values[0], runtime.heap(), "udp.send_to_task()")?;
    let data = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "udp.send_to_task data")?;
    let addr = runtime_string_arg(&values[2], runtime.heap(), "udp.send_to_task addr")?.to_string();
    spawn_task(runtime, async move {
        let RuntimeVal::Int(sent) = send_socket(socket, &data, &addr)? else {
            unreachable!("send_socket returns int")
        };
        Ok(payload_int(sent))
    })
}

fn recv_args(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<(UdpSocket, usize)> {
    if args.is_empty() || args.len() > 2 {
        bail!("udp.recv_from() expects 1 or 2 arguments: socket[, max_bytes]");
    }
    let values = args.as_slice();
    let socket = socket_clone(&values[0], runtime.heap(), "udp.recv_from()")?;
    let max = if let Some(value) = values.get(1) {
        usize_arg(value, "udp.recv_from max_bytes")?
    } else {
        4096
    };
    if max > MAX_DATAGRAM_READ_LIMIT {
        bail!("udp.recv_from max_bytes must be <= {MAX_DATAGRAM_READ_LIMIT}, got {max}");
    }
    Ok((socket, max))
}

fn socket_clone(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<UdpSocket> {
    let resource = resource_arg(value, heap, context)?;
    let handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    match &*handle {
        ResourceHandle::UdpSocket(socket) => socket
            .try_clone()
            .map_err(|err| anyhow!("{context} clone socket: {err}")),
        ResourceHandle::Closed => bail!("{context} resource is closed"),
        other => bail!("{context} expects UdpSocket, got {}", resource_kind(other)),
    }
}

fn recv_socket(socket: UdpSocket, max: usize) -> Result<(Vec<u8>, String)> {
    let mut buffer = vec![0u8; max];
    let (read, addr) = socket
        .recv_from(&mut buffer)
        .map_err(|err| anyhow!("udp recv_from: {err}"))?;
    buffer.truncate(read);
    Ok((buffer, addr.to_string()))
}

fn send_socket(socket: UdpSocket, data: &[u8], addr: &str) -> Result<RuntimeVal> {
    let sent = socket
        .send_to(data, addr)
        .map_err(|err| anyhow!("udp send_to {addr}: {err}"))?;
    Ok(RuntimeVal::Int(sent as i64))
}

fn spawn_task(
    runtime: &mut NativeRuntime<'_>,
    future: impl std::future::Future<Output = Result<RuntimePayload>> + Send + 'static,
) -> Result<RuntimeVal> {
    let task_id = rt::with_runtime(|rt| rt.spawn(future)).map_err(|err| anyhow!("failed to spawn task: {err}"))?;
    Ok(task_value(task_id, runtime.heap_mut()))
}

fn recv_result_value(data: Vec<u8>, addr: String, heap: &mut HeapStore) -> RuntimeVal {
    let data = runtime_bytes_value(data, heap);
    let addr = runtime_string_value(&addr, heap);
    let mut fields = fast_hash_map_new();
    fields.insert(RuntimeMapKey::String(Arc::<str>::from("data")), data);
    fields.insert(RuntimeMapKey::String(Arc::<str>::from("addr")), addr);
    RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::Mixed(fields))))
}

fn payload_recv_result(data: Vec<u8>, addr: String) -> RuntimePayload {
    let mut heap = HeapStore::new();
    let value = recv_result_value(data, addr, &mut heap);
    RuntimePayload::new(value, heap)
}

fn usize_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    match value {
        RuntimeVal::Int(value) if *value >= 0 => Ok(*value as usize),
        other => bail!("{context} expects a non-negative integer, got {:?}", other.kind()),
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}

fn resource_kind(handle: &ResourceHandle) -> &'static str {
    match handle {
        ResourceHandle::File(_) => "File",
        ResourceHandle::Stdin => "Stdin",
        ResourceHandle::Stdout => "Stdout",
        ResourceHandle::Stderr => "Stderr",
        ResourceHandle::TcpStream(_) => "TcpStream",
        ResourceHandle::TcpListener(_) => "TcpListener",
        ResourceHandle::UdpSocket(_) => "UdpSocket",
        ResourceHandle::Closed => "Closed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::val::{HeapValue, RuntimeVal};

    #[test]
    fn recv_result_value_contains_data_and_addr_fields() {
        let mut heap = HeapStore::new();
        let value = recv_result_value(vec![1, 2, 3], "127.0.0.1:9999".to_string(), &mut heap);
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected result map");
        };
        let Some(HeapValue::Map(map)) = heap.get(handle) else {
            panic!("expected result map");
        };

        assert!(matches!(map.get_str("data"), Some(RuntimeVal::Obj(_))));
        let Some(addr) = map.get_str("addr") else {
            panic!("expected addr field");
        };
        assert!(matches!(addr, RuntimeVal::ShortStr(_) | RuntimeVal::Obj(_)));
    }
}
