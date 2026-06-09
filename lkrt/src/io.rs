use crate::abi::{aborting, c_str, owned_c_string};
use std::{
    ffi::c_char,
    io::{Read, Write},
};

const MAX_STDIN_READ_BYTES: u64 = 1024 * 1024;

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
        0 => Err("io.std.flush unsupported for stdin".to_string()),
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
        let stdin = std::io::stdin();
        stdin
            .lock()
            .take(MAX_STDIN_READ_BYTES + 1)
            .read_to_string(&mut input)
            .map_err(|err| format!("stdin read failed: {err}"))?;
        if input.len() as u64 > MAX_STDIN_READ_BYTES {
            return Err(format!("stdin read exceeded {MAX_STDIN_READ_BYTES} byte limit"));
        }
        owned_c_string(input)
    })
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
