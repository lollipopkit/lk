//! Native `uuid`: the same `uuid` crate the stdlib module uses, so the text,
//! the accepted input forms, and the parse-error wording are one rule.
//!
//! That last part is why sharing matters here more than usual: `uuid.parse`
//! raises `invalid UUID: {err}` where `{err}` is the crate's own `Display`
//! (`invalid character: found `n` at 0`), and a caught error's message *is*
//! program output. A hand-written parser would have had to reproduce that
//! sentence.
//!
//! `std`-only: `v4` draws from the OS entropy source, which bare metal does not
//! have. The other two members would work without it, but splitting the module
//! across the `std` line for two functions the no_std profile has no way to
//! reach anyway is not worth the cfg.

use alloc::string::ToString as _;
use core::ffi::{CStr, c_char};

fn view(text: *const c_char) -> &'static str {
    if text.is_null() {
        return "";
    }
    // SAFETY: LK strings reaching the ABI are NUL-terminated and outlive the call.
    unsafe { CStr::from_ptr(text) }.to_str().unwrap_or("")
}

fn out(text: alloc::string::String) -> *mut c_char {
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(text).unwrap_or_default())
}

/// `uuid.v4()`.
///
/// Not `Pure` in the ABI schema, and it must never become so: two calls in one
/// block are two different UUIDs, and CSE would merge them.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_uuid_v4() -> *mut c_char {
    out(uuid::Uuid::new_v4().to_string())
}

/// `uuid.parse(text)` — canonical lowercase hyphenated form, or a raise
/// carrying the crate's own reason.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_uuid_parse(text: *const c_char) -> *mut c_char {
    match uuid::Uuid::parse_str(view(text)) {
        Ok(parsed) => out(parsed.to_string()),
        Err(error) => crate::panic::raise_str(&alloc::format!("invalid UUID: {error}")),
    }
}

/// `uuid.is_valid(text)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_uuid_is_valid(text: *const c_char) -> i64 {
    i64::from(uuid::Uuid::parse_str(view(text)).is_ok())
}
