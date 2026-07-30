use crate::{
    abi::{c_str, owned_c_string, raising},
    state::{HandleKind, with_runtime},
};
use std::{
    ffi::c_char,
    io::{Read, Write},
    net::TcpStream,
};

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_socket_addr(host: *const c_char, port: i64) -> *mut c_char {
    raising(|| {
        if !(0..=65535).contains(&port) {
            return Err(format!("socket.addr port expects integer 0..65535, got {port}"));
        }
        let host = c_str(host, "socket.addr host")?;
        owned_c_string(socket_addr_text(&host, port))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_connect(addr: *const c_char) -> i64 {
    raising(|| {
        let addr = c_str(addr, "tcp.connect addr")?;
        let stream = TcpStream::connect(addr.as_str()).map_err(|err| format!("tcp connect {addr}: {err}"))?;
        Ok(with_runtime(|rt| rt.insert_stream(stream)))
    })
}

/// `tcp.read(stream, max)` — the bytes read, as a `Bytes` **value**.
///
/// It used to answer the one-shot host handle (`insert_bytes`, read once with
/// `take_bytes`). That made `tcp.read` produce something that was not the
/// language's `Bytes`: you could hand it to `bytes.to_string_utf8` exactly once
/// and to nothing else. A `Bytes` is an ordinary value, so this hands back the
/// arena handle every other producer does.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_read(stream: i64, max_bytes: i64) -> *mut core::ffi::c_void {
    raising(|| {
        let max = checked_read_len(max_bytes)?;
        let mut stream = with_runtime(|rt| {
            rt.stream(stream)?
                .try_clone()
                .map_err(|err| format!("tcp.read clone stream: {err}"))
        })?;
        let mut buffer = vec![0u8; max];
        let read = stream.read(&mut buffer).map_err(|err| format!("tcp read: {err}"))?;
        buffer.truncate(read);
        Ok(crate::lkbytes::bytes_handle(buffer))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_write_str(stream: i64, data: *const c_char) -> i64 {
    raising(|| {
        let data = c_str(data, "tcp.write data")?;
        write_stream(stream, data.as_bytes())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_write_bytes(stream: i64, data: *mut core::ffi::c_void) -> i64 {
    raising(|| write_stream(stream, crate::lkbytes::bytes_slice(data)))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_tcp_close(stream: i64) -> i64 {
    raising(|| with_runtime(|rt| rt.close_kind(stream, HandleKind::TcpStream)).map(i64::from))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_handle_close(handle: i64) -> i64 {
    i64::from(with_runtime(|rt| rt.close_any(handle)))
}

fn write_stream(handle: i64, data: &[u8]) -> Result<i64, String> {
    let mut stream = with_runtime(|rt| {
        rt.stream(handle)?
            .try_clone()
            .map_err(|err| format!("tcp.write clone stream: {err}"))
    })?;
    stream.write_all(data).map_err(|err| format!("tcp write: {err}"))?;
    Ok(data.len() as i64)
}

fn socket_addr_text(host: &str, port: i64) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn checked_read_len(value: i64) -> Result<usize, String> {
    const MAX_READ_LIMIT: i64 = 1024 * 1024;
    if !(0..=MAX_READ_LIMIT).contains(&value) {
        return Err(format!("tcp.read max_bytes must be 0..={MAX_READ_LIMIT}, got {value}"));
    }
    Ok(value as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lkrt_last_error, lkrt_string_free};
    use std::{ffi::CString, net::TcpListener, thread};

    #[test]
    fn tcp_round_trip_over_typed_handles() {
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
        // A `Bytes` **value** now, not the one-shot host handle: read it twice
        // to say so.
        let bytes = lkrt_tcp_read(stream, 4);
        assert_eq!(unsafe { crate::lkbytes::lkrt_lkbytes_len(bytes) }, 4);
        let response = unsafe { crate::lkbytes::lkrt_lkbytes_utf8(bytes) };
        // SAFETY: response is an lkrt-owned NUL-terminated CString pointer.
        let response_text = unsafe { core::ffi::CStr::from_ptr(response) }
            .to_str()
            .expect("utf8")
            .to_owned();
        // SAFETY: the pointer came from an lkrt owned-string return.
        unsafe { lkrt_string_free(response) };
        assert_eq!(response_text, "pong");
        assert_eq!(unsafe { crate::lkbytes::lkrt_lkbytes_len(bytes) }, 4);
        assert_eq!(lkrt_tcp_close(stream), 1);
        assert_eq!(lkrt_tcp_close(stream), 0);
        server.join().expect("server thread");
    }

    #[test]
    fn last_error_is_owned_string() {
        let error = lkrt_last_error();
        assert!(!error.is_null());
        // SAFETY: the pointer came from an lkrt owned-string return.
        unsafe { lkrt_string_free(error) };
    }

    #[test]
    fn socket_addr_brackets_ipv6_literals() {
        let host = CString::new("::1").expect("host");
        let addr = lkrt_socket_addr(host.as_ptr(), 8080);
        assert!(!addr.is_null());
        // SAFETY: addr is an lkrt-owned NUL-terminated CString pointer.
        let addr_text = unsafe { core::ffi::CStr::from_ptr(addr) }
            .to_str()
            .expect("utf8")
            .to_owned();
        // SAFETY: the pointer came from an lkrt owned-string return.
        unsafe { lkrt_string_free(addr) };
        assert_eq!(addr_text, "[::1]:8080");
    }
}
