use anyhow::{Result, anyhow, bail};
use lk_core::{
    rt::{self, RuntimePayload},
    val::{HeapStore, ResourceHandle, RuntimeVal},
    vm::{NativeArgs, NativeRuntime},
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
};

use crate::{
    bytes::{runtime_bytes_or_string_arg, runtime_bytes_value},
    resource::{payload_int, payload_resource, resource_arg, resource_value, task_value},
    runtime_native::runtime_string_arg,
};

const MAX_READ_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "tcp", docs = "TCP networking helpers")]
pub struct NetTcpModule;

#[lk_stdlib_common::stdlib_exports(module = "net.tcp")]
impl NetTcpModule {
    #[stdlib_export(params(addr: String), returns = Resource)]
    fn connect(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let addr = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "tcp.connect addr")?;
        let stream = TcpStream::connect(addr.as_ref()).map_err(|err| anyhow!("tcp connect {addr}: {err}"))?;
        Ok(resource_value(
            "TcpStream",
            ResourceHandle::TcpStream(stream),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(params(addr: String), returns = Resource)]
    fn bind(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let addr = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "tcp.bind addr")?;
        let listener = TcpListener::bind(addr.as_ref()).map_err(|err| anyhow!("tcp bind {addr}: {err}"))?;
        Ok(resource_value(
            "TcpListener",
            ResourceHandle::TcpListener(listener),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(params(listener: Resource), returns = Resource)]
    fn accept(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let listener = listener_clone(args.get(0).expect("checked arity"), runtime.heap(), "tcp.accept()")?;
        let (stream, _) = listener.accept().map_err(|err| anyhow!("tcp accept: {err}"))?;
        Ok(resource_value(
            "TcpStream",
            ResourceHandle::TcpStream(stream),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(params(stream: Resource, max_bytes?: Int), returns = Bytes)]
    fn read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (stream, max) = read_args(args, runtime)?;
        let data = read_stream(stream, max)?;
        Ok(runtime_bytes_value(data, runtime.heap_mut()))
    }

    #[stdlib_export(params(stream: Resource, data: Bytes | String), returns = Int)]
    fn write(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let stream = stream_clone(&values[0], runtime.heap(), "tcp.write()")?;
        let data = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "tcp.write data")?;
        write_stream(stream, &data)
    }

    #[stdlib_export(params(resource: Resource), returns = Bool)]
    fn close(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "tcp.close()")?;
        Ok(RuntimeVal::Bool(crate::resource::close_resource(&resource)?))
    }

    #[stdlib_export(params(addr: String), returns = Task)]
    fn connect_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let addr = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "tcp.connect_task addr",
        )?
        .to_string();
        spawn_task(runtime, async move {
            let stream = TcpStream::connect(&addr).map_err(|err| anyhow!("tcp connect {addr}: {err}"))?;
            Ok(payload_resource("TcpStream", ResourceHandle::TcpStream(stream)))
        })
    }

    #[stdlib_export(params(listener: Resource), returns = Task)]
    fn accept_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let listener = listener_clone(args.get(0).expect("checked arity"), runtime.heap(), "tcp.accept_task()")?;
        spawn_task(runtime, async move {
            let (stream, _) = listener.accept().map_err(|err| anyhow!("tcp accept: {err}"))?;
            Ok(payload_resource("TcpStream", ResourceHandle::TcpStream(stream)))
        })
    }

    #[stdlib_export(params(stream: Resource, max_bytes?: Int), returns = Task)]
    fn read_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (stream, max) = read_args(args, runtime)?;
        spawn_task(
            runtime,
            async move { Ok(payload_resource_bytes(read_stream(stream, max)?)) },
        )
    }

    #[stdlib_export(params(stream: Resource, data: Bytes | String), returns = Task)]
    fn write_task(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let stream = stream_clone(&values[0], runtime.heap(), "tcp.write_task()")?;
        let data = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "tcp.write_task data")?.to_vec();
        spawn_task(runtime, async move {
            match write_stream(stream, &data)? {
                RuntimeVal::Int(written) => Ok(payload_int(written)),
                other => Err(anyhow!("tcp.write_task expected write count, got {:?}", other.kind())),
            }
        })
    }
}

fn read_args(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<(TcpStream, usize)> {
    if args.is_empty() || args.len() > 2 {
        bail!("tcp.read() expects 1 or 2 arguments: stream[, max_bytes]");
    }
    let values = args.as_slice();
    let stream = stream_clone(&values[0], runtime.heap(), "tcp.read()")?;
    let max = if let Some(value) = values.get(1) {
        usize_arg(value, "tcp.read max_bytes")?
    } else {
        4096
    };
    if max > MAX_READ_LIMIT {
        bail!("tcp.read max_bytes must be <= {MAX_READ_LIMIT}, got {max}");
    }
    Ok((stream, max))
}

fn stream_clone(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<TcpStream> {
    let resource = resource_arg(value, heap, context)?;
    let handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    match &*handle {
        ResourceHandle::TcpStream(stream) => stream
            .try_clone()
            .map_err(|err| anyhow!("{context} clone stream: {err}")),
        ResourceHandle::Closed => bail!("{context} resource is closed"),
        other => bail!("{context} expects TcpStream, got {}", resource_kind(other)),
    }
}

fn listener_clone(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<TcpListener> {
    let resource = resource_arg(value, heap, context)?;
    let handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    match &*handle {
        ResourceHandle::TcpListener(listener) => listener
            .try_clone()
            .map_err(|err| anyhow!("{context} clone listener: {err}")),
        ResourceHandle::Closed => bail!("{context} resource is closed"),
        other => bail!("{context} expects TcpListener, got {}", resource_kind(other)),
    }
}

fn read_stream(mut stream: TcpStream, max: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0u8; max];
    let read = stream.read(&mut buffer).map_err(|err| anyhow!("tcp read: {err}"))?;
    buffer.truncate(read);
    Ok(buffer)
}

fn write_stream(mut stream: TcpStream, data: &[u8]) -> Result<RuntimeVal> {
    stream.write_all(data).map_err(|err| anyhow!("tcp write: {err}"))?;
    Ok(RuntimeVal::Int(data.len() as i64))
}

fn spawn_task(
    runtime: &mut NativeRuntime<'_>,
    future: impl std::future::Future<Output = Result<RuntimePayload>> + Send + 'static,
) -> Result<RuntimeVal> {
    let task_id = rt::with_runtime(|rt| rt.spawn(future)).map_err(|err| anyhow!("failed to spawn task: {err}"))?;
    Ok(task_value(task_id, runtime.heap_mut()))
}

fn payload_resource_bytes(bytes: Vec<u8>) -> RuntimePayload {
    let mut heap = HeapStore::new();
    let value = runtime_bytes_value(bytes, &mut heap);
    RuntimePayload::new(value, heap)
}

fn usize_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    match value {
        RuntimeVal::Int(value) if *value >= 0 => Ok(*value as usize),
        other => bail!("{context} expects a non-negative integer, got {:?}", other.kind()),
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
