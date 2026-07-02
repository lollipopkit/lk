//! String helpers for AOT (constant string operations).
//!
//! Strings in the lowered subset are compile-time constants materialized as NUL-
//! terminated C strings (interned globals). These helpers implement the value-level
//! operations the MIR path lowers — currently ordered comparison, used to lower the
//! generic integer-compare opcodes when they dispatch on two string operands.

use std::cmp::Ordering;
use std::ffi::{CStr, CString, c_char};

use crate::state::runtime;

/// Returns an owned C string across the FFI boundary, registering it with the
/// runtime arena so it is reclaimed by `lkrt_string_free` (when the generated
/// code knows the value is dead) or by `lkrt_cleanup` at program exit.
pub(crate) fn arena_c_string(s: CString) -> *mut c_char {
    let ptr = s.into_raw();
    runtime().lock().expect("lkrt runtime poisoned").register_string(ptr);
    ptr
}

/// Byte-wise lexicographic comparison of two C strings, returning `-1`/`0`/`1`
/// (the sign of the ordering). The caller compares the result against `0` to
/// realize `==`/`!=`/`<`/`<=`/`>`/`>=`, matching the VM's string comparison
/// (exact equality for `==`, lexicographic otherwise).
///
/// # Safety
/// `a` and `b` must be valid NUL-terminated C strings, or null (treated as empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_cmp(a: *const c_char, b: *const c_char) -> i64 {
    // SAFETY: caller guarantees valid NUL-terminated strings (or null → empty).
    let a = if a.is_null() {
        b""
    } else {
        unsafe { CStr::from_ptr(a) }.to_bytes()
    };
    let b = if b.is_null() {
        b""
    } else {
        unsafe { CStr::from_ptr(b) }.to_bytes()
    };
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Concatenates two C strings into a freshly allocated NUL-terminated C string
/// (`a ++ b`), returned as an owned, arena-registered pointer (freed by
/// `lkrt_string_free` for known-dead intermediates, else by `lkrt_cleanup`).
///
/// # Safety
/// `a` and `b` must be valid NUL-terminated C strings, or null (treated as empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_concat(a: *const c_char, b: *const c_char) -> *mut c_char {
    // SAFETY: caller guarantees valid NUL-terminated strings (or null → empty).
    let a = if a.is_null() {
        b""
    } else {
        unsafe { CStr::from_ptr(a) }.to_bytes()
    };
    let b = if b.is_null() {
        b""
    } else {
        unsafe { CStr::from_ptr(b) }.to_bytes()
    };
    let mut bytes = Vec::with_capacity(a.len() + b.len());
    bytes.extend_from_slice(a);
    bytes.extend_from_slice(b);
    // Strings shouldn't contain interior NULs; fall back to empty defensively.
    arena_c_string(CString::new(bytes).unwrap_or_default())
}

/// Renders an `i64` as its decimal string (the VM's integer display), returned as a
/// freshly allocated, arena-registered C string.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_to_str(n: i64) -> *mut c_char {
    arena_c_string(CString::new(n.to_string()).unwrap_or_default())
}

/// Renders an `f64` as its display string. The VM formats floats with Rust's
/// `f64::to_string()` (see `runtime_value_display_string`), so this uses the same —
/// giving byte-identical output (`2.0 → "2"`, `1.0/3.0 → "0.3333333333333333"`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_f64_to_str(x: f64) -> *mut c_char {
    arena_c_string(CString::new(x.to_string()).unwrap_or_default())
}

/// Renders a bool as `"true"`/`"false"` (the VM's boolean display).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_bool_to_str(b: i64) -> *mut c_char {
    let s = if b != 0 { "true" } else { "false" };
    arena_c_string(CString::new(s).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn renders_scalars() {
        unsafe {
            let i = lkrt_i64_to_str(-42);
            assert_eq!(CStr::from_ptr(i).to_bytes(), b"-42");
            crate::lkrt_string_free(i);
            let t = lkrt_bool_to_str(1);
            assert_eq!(CStr::from_ptr(t).to_bytes(), b"true");
            crate::lkrt_string_free(t);
            let f = lkrt_bool_to_str(0);
            assert_eq!(CStr::from_ptr(f).to_bytes(), b"false");
            crate::lkrt_string_free(f);
        }
    }

    #[test]
    fn compares_c_strings() {
        let hi = CString::new("hi").unwrap();
        let hi2 = CString::new("hi").unwrap();
        let ho = CString::new("ho").unwrap();
        unsafe {
            assert_eq!(lkrt_str_cmp(hi.as_ptr(), hi2.as_ptr()), 0);
            assert_eq!(lkrt_str_cmp(hi.as_ptr(), ho.as_ptr()), -1);
            assert_eq!(lkrt_str_cmp(ho.as_ptr(), hi.as_ptr()), 1);
        }
    }

    #[test]
    fn concatenates_c_strings() {
        let foo = CString::new("foo").unwrap();
        let bar = CString::new("bar").unwrap();
        unsafe {
            let out = lkrt_str_concat(foo.as_ptr(), bar.as_ptr());
            assert_eq!(CStr::from_ptr(out).to_bytes(), b"foobar");
            // reclaim the arena-registered allocation in the test
            crate::lkrt_string_free(out);
        }
    }
}
