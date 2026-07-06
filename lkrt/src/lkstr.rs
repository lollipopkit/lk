//! String helpers for AOT (constant string operations).
//!
//! Strings in the lowered subset are compile-time constants materialized as NUL-
//! terminated C strings (interned globals). These helpers implement the value-level
//! operations the MIR path lowers — currently ordered comparison, used to lower the
//! generic integer-compare opcodes when they dispatch on two string operands.

use std::cmp::Ordering;
use std::ffi::{CStr, CString, c_char};

use crate::state::with_runtime;

/// Returns an owned C string across the FFI boundary, registering it with the
/// runtime arena so it is reclaimed by `lkrt_string_free` (when the generated
/// code knows the value is dead) or by `lkrt_cleanup` at program exit.
pub(crate) fn arena_c_string(s: CString) -> *mut c_char {
    let ptr = s.into_raw();
    with_runtime(|rt| rt.register_string(ptr));
    ptr
}

/// `s.starts_with(prefix)` — byte-prefix test (Rust `str::starts_with`, the
/// VM's exact semantics). Null pointers count as empty strings.
///
/// # Safety
/// Both pointers must be null or NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_starts_with(s: *const std::ffi::c_char, prefix: *const std::ffi::c_char) -> i64 {
    let bytes = |p: *const std::ffi::c_char| {
        if p.is_null() {
            &[][..]
        } else {
            // SAFETY: non-null pointers are NUL-terminated per the ABI.
            unsafe { std::ffi::CStr::from_ptr(p) }.to_bytes()
        }
    };
    i64::from(bytes(s).starts_with(bytes(prefix)))
}

/// `s.len()` with the VM's exact semantics: the number of Unicode scalar
/// values (`chars().count()`), not bytes.
///
/// # Safety
/// `s` must be null or a NUL-terminated string pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_char_len(s: *const std::ffi::c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    // SAFETY: non-null pointers are NUL-terminated per the ABI.
    let text = unsafe { std::ffi::CStr::from_ptr(s) };
    match text.to_str() {
        Ok(text) => text.chars().count() as i64,
        Err(_) => text.to_bytes().len() as i64,
    }
}

/// `s.contains(needle)` — byte-substring test (Rust `str::contains`, the VM's
/// exact semantics). Null pointers count as empty strings.
///
/// # Safety
/// Both pointers must be null or NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_contains(s: *const c_char, needle: *const c_char) -> i64 {
    let bytes = |p: *const c_char| {
        if p.is_null() {
            &[][..]
        } else {
            // SAFETY: non-null pointers are NUL-terminated per the ABI.
            unsafe { CStr::from_ptr(p) }.to_bytes()
        }
    };
    let (s, needle) = (bytes(s), bytes(needle));
    i64::from(s.windows(needle.len().max(1)).any(|w| w == needle) || needle.is_empty())
}

/// Char-based range slice (`s[1..3]`), exactly the VM's
/// `slice_string_general`: negative indices count from the tail, everything
/// clamps (never a failure), and indices are *chars*, consistent with `s[i]`
/// and `s.len()`.
///
/// # Safety
/// `s` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_slice_chars(s: *const c_char, start: i64, end: i64) -> *mut c_char {
    let text = if s.is_null() {
        ""
    } else {
        // SAFETY: non-null pointers are NUL-terminated per the ABI.
        unsafe { CStr::from_ptr(s) }.to_str().unwrap_or("")
    };
    let s_len = text.chars().count() as i64;
    let start = if start < 0 {
        (s_len + start).max(0)
    } else {
        start.min(s_len)
    } as usize;
    let end = if end < 0 { (s_len + end).max(0) } else { end.min(s_len) } as usize;
    let start = start.min(end);
    let sliced: String = text.chars().skip(start).take(end - start).collect();
    arena_c_string(CString::new(sliced).unwrap_or_default())
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
    let mut bytes = Vec::with_capacity(a.len() + b.len() + 1);
    bytes.extend_from_slice(a);
    bytes.extend_from_slice(b);
    // SAFETY: both inputs come from NUL-terminated C strings, so the collected
    // bytes cannot contain an interior NUL.
    arena_c_string(unsafe { CString::from_vec_unchecked(bytes) })
}

/// `prefix ++ decimal(suffix)` in a single allocation: the composite string-int
/// map-key shape (`m["n${i}"]`) the lowering proves via `GetIndexStrI` /
/// `SetIndexStrI` facts. Equivalent to `concat(prefix, from_i64(suffix))`
/// without materializing the intermediate suffix string.
///
/// # Safety
/// `prefix` must be a valid NUL-terminated C string, or null (treated as empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_concat_i64(prefix: *const c_char, suffix: i64) -> *mut c_char {
    let prefix = if prefix.is_null() {
        b""
    } else {
        // SAFETY: caller guarantees a valid NUL-terminated string.
        unsafe { CStr::from_ptr(prefix) }.to_bytes()
    };
    let mut digits = [0u8; 20];
    let digits = i64_decimal(suffix, &mut digits);
    let mut bytes = Vec::with_capacity(prefix.len() + digits.len() + 1);
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(digits);
    // SAFETY: the prefix has no interior NUL (C string) and digits are ASCII.
    arena_c_string(unsafe { CString::from_vec_unchecked(bytes) })
}

/// Formats `n` as decimal ASCII into `buf` (right-aligned), returning the
/// written slice. `buf` fits every `i64` (19 digits + sign).
pub(crate) fn i64_decimal(n: i64, buf: &mut [u8; 20]) -> &[u8] {
    let mut magnitude = n.unsigned_abs();
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if n < 0 {
        at -= 1;
        buf[at] = b'-';
    }
    &buf[at..]
}

/// Renders an `i64` as its decimal string (the VM's integer display), returned as a
/// freshly allocated, arena-registered C string.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_to_str(n: i64) -> *mut c_char {
    let mut digits = [0u8; 20];
    let digits = i64_decimal(n, &mut digits);
    let mut bytes = Vec::with_capacity(digits.len() + 1);
    bytes.extend_from_slice(digits);
    // SAFETY: decimal digits and sign are ASCII, never NUL.
    arena_c_string(unsafe { CString::from_vec_unchecked(bytes) })
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
    fn concat_i64_matches_two_step_build() {
        let prefix = CString::new("n").unwrap();
        for n in [0, 7, -1, 1234567890123456789, i64::MIN, i64::MAX] {
            unsafe {
                let fused = lkrt_str_concat_i64(prefix.as_ptr(), n);
                assert_eq!(CStr::from_ptr(fused).to_str().unwrap(), format!("n{n}"), "suffix {n}");
                crate::lkrt_string_free(fused);
            }
        }
        unsafe {
            let empty = lkrt_str_concat_i64(std::ptr::null(), 42);
            assert_eq!(CStr::from_ptr(empty).to_bytes(), b"42");
            crate::lkrt_string_free(empty);
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
