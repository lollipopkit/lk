//! Typed native runtime support for LK LLVM AOT binaries.
//!
//! This crate is intentionally not the LK VM. It may provide low-level typed
//! helpers that LLVM-generated code links against, but it must not depend on the
//! parser, compiler, `ModuleArtifact`, `VmContext`, or the bytecode executor.

use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char},
    io::{Read, Write},
    net::TcpStream,
    sync::{Mutex, OnceLock},
};

/// Called by the CLI to make the Cargo dependency explicit.
pub fn link_anchor() -> u8 {
    0
}

/// Version string embedded in the static library for diagnostics.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_socket_addr(host: *const c_char, port: i64) -> *mut c_char {
    aborting(|| {
        if !(0..=65535).contains(&port) {
            return Err(format!("socket.addr port expects integer 0..65535, got {port}"));
        }
        owned_c_string(format!("{}:{port}", c_str(host, "socket.addr host")?))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_connect(addr: *const c_char) -> i64 {
    aborting(|| {
        let addr = c_str(addr, "tcp.connect addr")?;
        let stream = TcpStream::connect(addr.as_str()).map_err(|err| format!("tcp connect {addr}: {err}"))?;
        Ok(runtime().lock().expect("lkrt runtime poisoned").insert_stream(stream))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_read(stream: i64, max_bytes: i64) -> i64 {
    aborting(|| {
        let max = checked_read_len(max_bytes)?;
        let mut stream = runtime()
            .lock()
            .expect("lkrt runtime poisoned")
            .stream(stream)?
            .try_clone()
            .map_err(|err| format!("tcp.read clone stream: {err}"))?;
        let mut buffer = vec![0u8; max];
        let read = stream.read(&mut buffer).map_err(|err| format!("tcp read: {err}"))?;
        buffer.truncate(read);
        Ok(runtime().lock().expect("lkrt runtime poisoned").insert_bytes(buffer))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_write_str(stream: i64, data: *const c_char) -> i64 {
    aborting(|| {
        let data = c_str(data, "tcp.write data")?;
        write_stream(stream, data.as_bytes())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_write_bytes(stream: i64, data: i64) -> i64 {
    aborting(|| {
        let data = runtime().lock().expect("lkrt runtime poisoned").bytes(data)?.to_vec();
        write_stream(stream, &data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_close(stream: i64) -> i64 {
    aborting(|| {
        let closed = runtime()
            .lock()
            .expect("lkrt runtime poisoned")
            .remove_stream(stream)
            .is_some();
        Ok(i64::from(closed))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_bytes_to_string_utf8(bytes: i64) -> *mut c_char {
    aborting(|| {
        let bytes = runtime().lock().expect("lkrt runtime poisoned").bytes(bytes)?.to_vec();
        let value = std::str::from_utf8(&bytes).map_err(|err| format!("bytes are not valid UTF-8: {err}"))?;
        owned_c_string(value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_io_std_write(resource: i64, data: *const c_char, newline: i64) -> i64 {
    aborting(|| {
        let data = c_str(data, "io.std.write data")?;
        match resource {
            1 => write_std_stream(std::io::stdout().lock(), data.as_bytes(), newline != 0, "stdout"),
            2 => write_std_stream(std::io::stderr().lock(), data.as_bytes(), newline != 0, "stderr"),
            other => Err(format!("io.std.write unsupported resource handle {other}")),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_io_std_flush(resource: i64) -> i64 {
    aborting(|| match resource {
        0 => Ok(0),
        1 => std::io::stdout()
            .flush()
            .map(|_| 0)
            .map_err(|err| format!("stdout flush failed: {err}")),
        2 => std::io::stderr()
            .flush()
            .map(|_| 0)
            .map_err(|err| format!("stderr flush failed: {err}")),
        other => Err(format!("io.std.flush unsupported resource handle {other}")),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_io_std_read_to_string(resource: i64) -> *mut c_char {
    aborting(|| {
        if resource != 0 {
            return Err(format!("io.std.read_to_string expects stdin handle, got {resource}"));
        }
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|err| format!("stdin read failed: {err}"))?;
        owned_c_string(input)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_get_or(key: *const c_char, default: *const c_char) -> *mut c_char {
    aborting(|| {
        let key = c_str(key, "env.get_or key")?;
        let default = c_str(default, "env.get_or default")?;
        owned_c_string(std::env::var(key.as_str()).unwrap_or(default))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_exists(path: *const c_char) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.exists path")?;
        Ok(i64::from(std::path::Path::new(path.as_str()).exists()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_read_dir(path: *const c_char) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.read_dir path")?;
        std::fs::read_dir(path.as_str())
            .map_err(|err| format!("fs.read_dir {path}: {err}"))?
            .count();
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_process_cwd() -> *mut c_char {
    aborting(|| {
        let cwd = std::env::current_dir().map_err(|err| format!("process.cwd failed: {err}"))?;
        owned_c_string(cwd.to_string_lossy())
    })
}

fn runtime() -> &'static Mutex<RuntimeState> {
    static RUNTIME: OnceLock<Mutex<RuntimeState>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(RuntimeState::default()))
}

#[derive(Default)]
struct RuntimeState {
    next_handle: i64,
    streams: HashMap<i64, TcpStream>,
    bytes: HashMap<i64, Vec<u8>>,
}

impl RuntimeState {
    fn insert_stream(&mut self, stream: TcpStream) -> i64 {
        let handle = self.next_handle();
        self.streams.insert(handle, stream);
        handle
    }

    fn stream(&self, handle: i64) -> Result<&TcpStream, String> {
        self.streams
            .get(&handle)
            .ok_or_else(|| format!("tcp stream handle {handle} is closed or invalid"))
    }

    fn remove_stream(&mut self, handle: i64) -> Option<TcpStream> {
        self.streams.remove(&handle)
    }

    fn insert_bytes(&mut self, bytes: Vec<u8>) -> i64 {
        let handle = self.next_handle();
        self.bytes.insert(handle, bytes);
        handle
    }

    fn bytes(&self, handle: i64) -> Result<&[u8], String> {
        self.bytes
            .get(&handle)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("bytes handle {handle} is invalid"))
    }

    fn next_handle(&mut self) -> i64 {
        self.next_handle += 1;
        self.next_handle
    }
}

fn write_stream(handle: i64, data: &[u8]) -> Result<i64, String> {
    let mut stream = runtime()
        .lock()
        .expect("lkrt runtime poisoned")
        .stream(handle)?
        .try_clone()
        .map_err(|err| format!("tcp.write clone stream: {err}"))?;
    let written = stream.write(data).map_err(|err| format!("tcp write: {err}"))?;
    Ok(written as i64)
}

fn write_std_stream(mut stream: impl Write, data: &[u8], newline: bool, name: &str) -> Result<i64, String> {
    stream
        .write_all(data)
        .map_err(|err| format!("{name} write failed: {err}"))?;
    if newline {
        stream
            .write_all(b"\n")
            .map_err(|err| format!("{name} newline write failed: {err}"))?;
    }
    Ok(0)
}

fn checked_read_len(value: i64) -> Result<usize, String> {
    const MAX_READ_LIMIT: i64 = 1024 * 1024;
    if !(0..=MAX_READ_LIMIT).contains(&value) {
        return Err(format!("tcp.read max_bytes must be 0..={MAX_READ_LIMIT}, got {value}"));
    }
    Ok(value as usize)
}

fn c_str(ptr: *const c_char, context: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("{context} is null"));
    }
    // SAFETY: LLVM generated code passes NUL-terminated pointers produced by
    // LK string constants or lkrt-owned CString values. Null is checked above.
    let value = unsafe { CStr::from_ptr(ptr) };
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|err| format!("{context} is not valid UTF-8: {err}"))
}

fn owned_c_string(value: impl AsRef<str>) -> Result<*mut c_char, String> {
    CString::new(value.as_ref())
        .map(CString::into_raw)
        .map_err(|_| "string contains interior NUL byte".to_string())
}

fn aborting<T>(f: impl FnOnce() -> Result<T, String>) -> T {
    match f() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("lkrt error: {error}");
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn tcp_round_trip_over_handles() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr").to_string();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 4];
            stream.read_exact(&mut request).expect("read request");
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").expect("write response");
        });

        let addr = CString::new(addr).expect("addr cstring");
        let stream = lkrt_tcp_connect(addr.as_ptr());
        let request = CString::new("ping").expect("request cstring");
        assert_eq!(lkrt_tcp_write_str(stream, request.as_ptr()), 4);
        let bytes = lkrt_tcp_read(stream, 4);
        let response = lkrt_bytes_to_string_utf8(bytes);
        // SAFETY: lkrt_bytes_to_string_utf8 returns an owned NUL-terminated
        // CString pointer on success.
        let response = unsafe { CString::from_raw(response) };
        assert_eq!(response.to_str().expect("utf8"), "pong");
        assert_eq!(lkrt_tcp_close(stream), 1);
        assert_eq!(lkrt_tcp_close(stream), 0);
        server.join().expect("server thread");
    }
}
