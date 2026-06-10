use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{ResourceHandle, RuntimeVal},
    vm::{NativeArgs, NativeRuntime},
};
use std::io::{Read, Write};

use crate::{
    bytes::{runtime_bytes_or_string_arg, runtime_bytes_value},
    resource::{resource_arg, resource_value},
    runtime_native::runtime_string_value,
};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "std", docs = "Standard input and output resources")]
pub struct IoStdModule;

#[lk_stdlib_common::stdlib_exports(module = "io.std")]
impl IoStdModule {
    #[stdlib_export(params(), returns = Resource)]
    fn stdin(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(resource_value("Stdin", ResourceHandle::Stdin, runtime.heap_mut()))
    }

    #[stdlib_export(params(), returns = Resource)]
    fn stdout(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(resource_value("Stdout", ResourceHandle::Stdout, runtime.heap_mut()))
    }

    #[stdlib_export(params(), returns = Resource)]
    fn stderr(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(resource_value("Stderr", ResourceHandle::Stderr, runtime.heap_mut()))
    }

    #[stdlib_export(params(reader: Resource, max_bytes?: Int), returns = Bytes)]
    fn read_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        read(args, runtime)
    }

    #[stdlib_export(params(reader: Resource), returns = String)]
    fn read_to_string_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        read_to_string(args, runtime)
    }

    #[stdlib_export(params(reader: Resource), returns = String?)]
    fn read_line_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        read_line(args, runtime)
    }

    #[stdlib_export(params(writer: Resource, data: Bytes | String), returns = Int)]
    fn write_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        write(args, runtime)
    }

    #[stdlib_export(params(writer: Resource, data: Bytes | String), returns = Int)]
    fn writeln_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        writeln_fn(args, runtime)
    }

    #[stdlib_export(params(writer: Resource), returns = Bool)]
    fn flush_export(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        flush(args, runtime)
    }
}

pub fn read(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.is_empty() || args.len() > 2 {
        bail!("read() expects 1 or 2 arguments: reader[, max_bytes]");
    }
    let values = args.as_slice();
    let resource = resource_arg(&values[0], runtime.heap(), "read()")?;
    let max = if let Some(value) = values.get(1) {
        usize_arg(value, "read() max_bytes")?
    } else {
        4096
    };
    let mut handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    let mut buffer = vec![0u8; max];
    let read = match &mut *handle {
        ResourceHandle::File(file) => file
            .read(&mut buffer)
            .map_err(|err| anyhow!("file read error: {err}"))?,
        ResourceHandle::Stdin => std::io::stdin()
            .read(&mut buffer)
            .map_err(|err| anyhow!("stdin read error: {err}"))?,
        ResourceHandle::TcpStream(stream) => stream
            .read(&mut buffer)
            .map_err(|err| anyhow!("tcp read error: {err}"))?,
        ResourceHandle::Closed => bail!("read() resource is closed"),
        other => bail!("read() cannot read from {}", resource_kind(other)),
    };
    buffer.truncate(read);
    Ok(runtime_bytes_value(buffer, runtime.heap_mut()))
}

pub fn read_to_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "read_to_string()")?;
    let mut handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    let mut out = String::new();
    match &mut *handle {
        ResourceHandle::File(file) => {
            file.read_to_string(&mut out)
                .map_err(|err| anyhow!("file read error: {err}"))?;
        }
        ResourceHandle::Stdin => {
            std::io::stdin()
                .read_to_string(&mut out)
                .map_err(|err| anyhow!("stdin read error: {err}"))?;
        }
        ResourceHandle::TcpStream(stream) => {
            stream
                .read_to_string(&mut out)
                .map_err(|err| anyhow!("tcp read error: {err}"))?;
        }
        ResourceHandle::Closed => bail!("read_to_string() resource is closed"),
        other => bail!("read_to_string() cannot read from {}", resource_kind(other)),
    }
    Ok(runtime_string_value(&out, runtime.heap_mut()))
}

pub fn read_line(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "read_line()")?;
    let mut handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    let mut out = String::new();
    let read = match &mut *handle {
        ResourceHandle::File(file) => {
            read_line_unbuffered(file, &mut out).map_err(|err| anyhow!("file read error: {err}"))?
        }
        ResourceHandle::Stdin => read_line_unbuffered(&mut std::io::stdin().lock(), &mut out)
            .map_err(|err| anyhow!("stdin read error: {err}"))?,
        ResourceHandle::Closed => bail!("read_line() resource is closed"),
        other => bail!("read_line() cannot read from {}", resource_kind(other)),
    };
    if read == 0 {
        return Ok(RuntimeVal::Nil);
    }
    if out.ends_with('\n') {
        out.pop();
        if out.ends_with('\r') {
            out.pop();
        }
    }
    Ok(runtime_string_value(&out, runtime.heap_mut()))
}

pub fn write(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let resource = resource_arg(&values[0], runtime.heap(), "write()")?;
    let data = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "write() data")?;
    write_bytes(&resource, &data)
}

pub fn writeln_fn(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let resource = resource_arg(&values[0], runtime.heap(), "writeln()")?;
    let data = runtime_bytes_or_string_arg(&values[1], runtime.heap(), "writeln() data")?;
    let mut out = Vec::with_capacity(data.len() + 1);
    out.extend_from_slice(&data);
    out.push(b'\n');
    write_bytes(&resource, &out)
}

pub fn flush(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let resource = resource_arg(args.get(0).expect("checked arity"), runtime.heap(), "flush()")?;
    let mut handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    match &mut *handle {
        ResourceHandle::File(file) => file.flush().map_err(|err| anyhow!("file flush error: {err}"))?,
        ResourceHandle::Stdout => std::io::stdout()
            .flush()
            .map_err(|err| anyhow!("stdout flush error: {err}"))?,
        ResourceHandle::Stderr => std::io::stderr()
            .flush()
            .map_err(|err| anyhow!("stderr flush error: {err}"))?,
        ResourceHandle::TcpStream(stream) => stream.flush().map_err(|err| anyhow!("tcp flush error: {err}"))?,
        ResourceHandle::Closed => bail!("flush() resource is closed"),
        other => bail!("flush() cannot flush {}", resource_kind(other)),
    }
    Ok(RuntimeVal::Bool(true))
}

pub fn write_bytes(resource: &lk_core::val::ResourceValue, data: &[u8]) -> Result<RuntimeVal> {
    let mut handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    let written = match &mut *handle {
        ResourceHandle::File(file) => {
            file.write_all(data).map_err(|err| anyhow!("file write error: {err}"))?;
            data.len()
        }
        ResourceHandle::Stdout => {
            std::io::stdout()
                .write_all(data)
                .map_err(|err| anyhow!("stdout write error: {err}"))?;
            data.len()
        }
        ResourceHandle::Stderr => {
            std::io::stderr()
                .write_all(data)
                .map_err(|err| anyhow!("stderr write error: {err}"))?;
            data.len()
        }
        ResourceHandle::TcpStream(stream) => stream.write(data).map_err(|err| anyhow!("tcp write error: {err}"))?,
        ResourceHandle::Closed => bail!("write() resource is closed"),
        other => bail!("write() cannot write to {}", resource_kind(other)),
    };
    Ok(RuntimeVal::Int(written as i64))
}

fn read_line_unbuffered(reader: &mut impl Read, out: &mut String) -> std::io::Result<usize> {
    let mut bytes = Vec::new();
    let mut one = [0u8; 1];
    loop {
        let read = reader.read(&mut one)?;
        if read == 0 {
            break;
        }
        bytes.push(one[0]);
        if one[0] == b'\n' {
            break;
        }
    }
    let read = bytes.len();
    *out = String::from_utf8(bytes).map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    Ok(read)
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
