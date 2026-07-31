//! Native `process`: the current process, and child processes.
//!
//! Every member reports with the stdlib module's own sentence, because a caught
//! error's message is program output. The one that is *not* an error path is
//! `output`, whose four-key map is built through the VM's two-stage
//! construction so it iterates — and therefore prints — the same way.

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_void};

use std::process::Command;

use crate::abi::{c_str, owned_c_string, raising};

/// The `args` list, whose elements are the same `*const c_char` a native
/// `List<String>` holds.
///
/// # Safety
/// `handle` must be a live string-list handle, or null for "no arguments".
unsafe fn argv(handle: *mut c_void) -> Vec<String> {
    if handle.is_null() {
        return Vec::new();
    }
    // SAFETY: a live string-list handle, as the ABI declares.
    let values: &Vec<*const c_char> = unsafe { &*(handle as *mut Vec<*const c_char>) };
    values
        .iter()
        .map(|&ptr| {
            if ptr.is_null() {
                String::new()
            } else {
                // SAFETY: list elements are NUL-terminated LK strings.
                unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
            }
        })
        .collect()
}

fn run(cmd: &str, args: &[String]) -> Result<std::process::Output, String> {
    Command::new(cmd)
        .args(args)
        .output()
        .map_err(|err| alloc::format!("failed to execute '{cmd}': {err}"))
}

/// `process.id()`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_process_id() -> i64 {
    std::process::id() as i64
}

/// `process.set_cwd(path)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_process_set_cwd(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "process.set_cwd path")?;
        std::env::set_current_dir(path.as_str()).map_err(|err| alloc::format!("failed to set cwd '{path}': {err}"))?;
        Ok(1)
    })
}

/// `process.exit(code)` — does not return.
///
/// Nothing is flushed here on purpose, and it took measuring to be sure of
/// that: generated code prints through **C stdio** (`printf`), and
/// `std::process::exit` calls libc `exit`, which flushes those streams — so an
/// unterminated `print("partial")` still reaches the terminal. The Rust-side
/// stream that `io.std.write` uses flushes at every write (see `io.rs`). A
/// belt-and-braces `stdout().flush()` here would have been flushing the stream
/// that was never the one at risk.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_process_exit(code: i64) {
    if code < i64::from(i32::MIN) || code > i64::from(i32::MAX) {
        crate::panic::raise_str(&alloc::format!("process.exit code must fit in i32, got {code}"));
    }
    std::process::exit(code as i32);
}

/// The one-argument spellings — `process.status("true")` with no argument list.
///
/// # Safety
/// `cmd` must be a NUL-terminated LK string.
///
/// Separate entry points rather than a null handle materialised at the call
/// site: the module-call lowering passes exactly the arguments a row declares,
/// and inventing a null pointer for a missing one is the kind of thing that
/// works until a row's parameter is not a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_process_status_noargs(cmd: *const c_char) -> i64 {
    // SAFETY: a null handle is "no arguments", which `argv` handles.
    unsafe { lkrt_process_status(cmd, core::ptr::null_mut()) }
}

/// # Safety
/// `cmd` must be a NUL-terminated LK string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_process_output_string_noargs(cmd: *const c_char) -> *mut c_char {
    // SAFETY: see above.
    unsafe { lkrt_process_output_string(cmd, core::ptr::null_mut()) }
}

/// # Safety
/// `cmd` must be a NUL-terminated LK string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_process_output_noargs(cmd: *const c_char) -> *mut c_void {
    // SAFETY: see above.
    unsafe { lkrt_process_output(cmd, core::ptr::null_mut()) }
}

/// `process.status(cmd[, args])` — the exit code, or -1 when a signal killed
/// the child (the stdlib's `code().unwrap_or(-1)`).
///
/// # Safety
/// `args` must be a live string-list handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_process_status(cmd: *const c_char, args: *mut c_void) -> i64 {
    raising(|| {
        let cmd = c_str(cmd, "process command")?;
        // SAFETY: forwarded from the ABI, which declares the same contract.
        let argv = unsafe { argv(args) };
        let output = run(cmd.as_str(), &argv)?;
        Ok(i64::from(output.status.code().unwrap_or(-1)))
    })
}

/// `process.output_string(cmd[, args])` — the child's stdout as text.
///
/// # Safety
/// `args` must be a live string-list handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_process_output_string(cmd: *const c_char, args: *mut c_void) -> *mut c_char {
    raising(|| {
        let cmd = c_str(cmd, "process command")?;
        // SAFETY: forwarded from the ABI, which declares the same contract.
        let argv = unsafe { argv(args) };
        let output = run(cmd.as_str(), &argv)?;
        let stdout = String::from_utf8(output.stdout).map_err(|_| "command stdout is not valid UTF-8".to_string())?;
        owned_c_string(stdout)
    })
}

/// `process.output(cmd[, args])` — `status`, `success`, `stdout`, `stderr`, in
/// the stdlib module's insertion order, with the two streams as `Bytes`.
///
/// # Safety
/// `args` must be a live string-list handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_process_output(cmd: *const c_char, args: *mut c_void) -> *mut c_void {
    raising(|| {
        let cmd = c_str(cmd, "process command")?;
        // SAFETY: forwarded from the ABI, which declares the same contract.
        let argv = unsafe { argv(args) };
        let output = run(cmd.as_str(), &argv)?;
        let pairs = alloc::vec![
            (
                String::from("status"),
                crate::lkdyn::lkrt_dyn_from_i64(i64::from(output.status.code().unwrap_or(-1)))
            ),
            (
                String::from("success"),
                crate::lkdyn::lkrt_dyn_from_bool(i64::from(output.status.success()))
            ),
            (
                String::from("stdout"),
                crate::lkdyn::lkrt_dyn_from_bytes(crate::lkbytes::bytes_handle(output.stdout))
            ),
            (
                String::from("stderr"),
                crate::lkdyn::lkrt_dyn_from_bytes(crate::lkbytes::bytes_handle(output.stderr))
            ),
        ];
        Ok(crate::vm_mirror::str_dyn_map_mirrored(pairs))
    })
}
