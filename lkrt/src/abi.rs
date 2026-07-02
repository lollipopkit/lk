use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
};

use crate::state::runtime;

pub(crate) use lk_aot_abi::ABI_VERSION;
pub(crate) const LKRT_STATUS_OK: i64 = 0;
pub(crate) const LKRT_STATUS_ERR: i64 = -1;

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_abi_version() -> i64 {
    ABI_VERSION
}

/// Called at the start of a native binary's `main` with the ABI version the code
/// was generated against. If the linked `lkrt` reports a different version the
/// binary and runtime disagree on the calling/representation contract, so we
/// abort with a clear message rather than execute with a mismatched ABI (this is
/// a link/configuration error, never a reason to fall back to the VM).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_abi_check(expected: i64) {
    if expected != ABI_VERSION {
        eprintln!("lkrt ABI mismatch: binary built for ABI v{expected}, linked lkrt is v{ABI_VERSION}");
        std::process::abort();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_last_error() -> *mut c_char {
    let error = LAST_ERROR.with(|slot| slot.borrow().clone().unwrap_or_default());
    owned_c_string_lossy(error)
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_error_clear() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cleanup() {
    runtime().lock().expect("lkrt runtime poisoned").cleanup();
}

/// Frees an arena-registered string returned by an lkrt function. Unregistered
/// or null pointers are ignored, so double-frees through this entry point are
/// harmless.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by an lkrt function
/// (`CString::into_raw`-based) that has not been freed by other means.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    if !runtime().lock().expect("lkrt runtime poisoned").unregister_string(ptr) {
        return;
    }
    // SAFETY: The pointer must come from an lkrt function that returned a
    // CString through CString::into_raw. Null was handled above.
    unsafe {
        drop(CString::from_raw(ptr));
    }
}

pub(crate) fn c_str(ptr: *const c_char, context: &str) -> Result<String, String> {
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

pub(crate) fn owned_c_string(value: impl AsRef<str>) -> Result<*mut c_char, String> {
    let ptr = CString::new(value.as_ref())
        .map(CString::into_raw)
        .map_err(|_| "string contains interior NUL byte".to_string())?;
    runtime().lock().expect("lkrt runtime poisoned").register_string(ptr);
    Ok(ptr)
}

pub(crate) fn aborting<T>(f: impl FnOnce() -> Result<T, String>) -> T {
    match f() {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error.clone());
            eprintln!("lkrt error: {error}");
            std::process::abort();
        }
    }
}

pub(crate) fn set_last_error(error: impl Into<String>) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(error.into());
    });
}

pub(crate) fn status(f: impl FnOnce() -> Result<(), String>) -> i64 {
    match f() {
        Ok(()) => {
            lkrt_error_clear();
            LKRT_STATUS_OK
        }
        Err(error) => {
            set_last_error(error);
            LKRT_STATUS_ERR
        }
    }
}

pub(crate) fn write_out<T>(out: *mut T, value: T, context: &str) -> Result<(), String> {
    if out.is_null() {
        return Err(format!("{context} out pointer is null"));
    }
    // SAFETY: The caller provides a valid out pointer for the C ABI result.
    unsafe {
        *out = value;
    }
    Ok(())
}

fn owned_c_string_lossy(value: impl AsRef<str>) -> *mut c_char {
    let sanitized = value.as_ref().replace('\0', "\\0");
    let ptr = CString::new(sanitized)
        .expect("sanitized lkrt error string has no interior NUL")
        .into_raw();
    runtime().lock().expect("lkrt runtime poisoned").register_string(ptr);
    ptr
}
