use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    rt::{self, RuntimePayload},
    val::{HeapStore, ResourceHandle, RuntimeVal},
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use std::net::UdpSocket;

use crate::{
    resource::{payload_int, payload_string, resource_arg, resource_value, task_value},
    runtime_native::{runtime_string_arg, runtime_string_value},
};

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
        "net/udp"
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
    let (data, _addr) = recv_socket(socket, max)?;
    Ok(runtime_string_value(&data, runtime.heap_mut()))
}

fn send_to(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "udp.send_to()")?;
    let values = args.as_slice();
    let socket = socket_clone(&values[0], runtime.heap(), "udp.send_to()")?;
    let data = runtime_string_arg(&values[1], runtime.heap(), "udp.send_to data")?;
    let addr = runtime_string_arg(&values[2], runtime.heap(), "udp.send_to addr")?;
    send_socket(socket, data.as_bytes(), &addr)
}

fn recv_from_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (socket, max) = recv_args(args, runtime)?;
    spawn_task(runtime, async move {
        let (data, _addr) = recv_socket(socket, max)?;
        Ok(payload_string(data))
    })
}

fn send_to_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "udp.send_to_task()")?;
    let values = args.as_slice();
    let socket = socket_clone(&values[0], runtime.heap(), "udp.send_to_task()")?;
    let data = runtime_string_arg(&values[1], runtime.heap(), "udp.send_to_task data")?.to_string();
    let addr = runtime_string_arg(&values[2], runtime.heap(), "udp.send_to_task addr")?.to_string();
    spawn_task(runtime, async move {
        let RuntimeVal::Int(sent) = send_socket(socket, data.as_bytes(), &addr)? else {
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

fn recv_socket(socket: UdpSocket, max: usize) -> Result<(String, String)> {
    let mut buffer = vec![0u8; max];
    let (read, addr) = socket
        .recv_from(&mut buffer)
        .map_err(|err| anyhow!("udp recv_from: {err}"))?;
    buffer.truncate(read);
    let data = String::from_utf8(buffer).map_err(|_| anyhow!("udp data is not valid UTF-8"))?;
    Ok((data, addr.to_string()))
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
