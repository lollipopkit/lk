//! The `io` ABI surface for targets without an OS.
//!
//! `io.rs` needs `std::io`, so bare metal gets this instead. The functions have
//! to exist rather than be omitted: codegen declares them for any program that
//! might print, and a missing symbol fails the link even when nothing calls it.
//!
//! Where the bytes go is the board's business, so the sink is installed by the
//! binary — the same shape `stdlib/bare`'s `set_output` uses for the
//! interpreter. Until one is installed, output is discarded rather than being
//! an error: a program that logs should still run headless.

use core::ffi::{CStr, c_char};

/// Where `println`/`print` output goes. A plain `fn` pointer so the slot is
/// `const`-initialisable and needs no allocation before `main`.
type OutputSink = fn(&str);

static OUTPUT: spin::Mutex<Option<OutputSink>> = spin::Mutex::new(None);

/// Installs the console. Call once during board bring-up.
pub fn set_output(sink: OutputSink) {
    *OUTPUT.lock() = Some(sink);
}

/// `lkrt_io_std_write(resource, data, newline)` — the ABI entry codegen emits
/// for `print`/`println`.
///
/// # Safety
///
/// `data` must be a valid NUL-terminated C string, as codegen guarantees.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_io_std_write(_resource: i64, data: *const c_char, newline: i64) -> i64 {
    // The sink is copied out and the guard dropped before calling it: a sink
    // that itself logs would otherwise deadlock on a spin mutex.
    let Some(sink) = *OUTPUT.lock() else {
        return 0;
    };
    if !data.is_null() {
        // SAFETY: codegen passes a NUL-terminated string it owns.
        let text = unsafe { CStr::from_ptr(data) };
        if let Ok(text) = text.to_str() {
            sink(text);
        }
    }
    if newline != 0 {
        sink("\n");
    }
    0
}

/// No buffering to flush: the sink writes synchronously.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_io_std_flush(_resource: i64) -> i64 {
    0
}

/// There is no standard input on a bare-metal board. Returning null is the
/// "read nothing" answer the ABI already defines.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_io_std_read_to_string(_resource: i64) -> *mut c_char {
    core::ptr::null_mut()
}
