//! Growable string-keyed map handle for AOT (Phase 2 container handle-ification).
//!
//! A `Map<str, i64>` is an opaque `*mut HashMap<String, i64>` handle. Keys arrive
//! as NUL-terminated C strings (materialized from interned string-constant globals,
//! since map keys in the lowered subset are always compile-time constants). Like the
//! list handles, maps are leaked (never freed) — matching the short-lived AOT binary
//! ownership convention (see `lklist`); a real free model is future work.
//!
//! Lookup follows the VM's map semantics: a missing key yields "absent"
//! (`present = 0`), modelled by the caller as `Maybe<Int>`. A store always
//! inserts-or-updates (never an error), unlike a list store.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};

use crate::lklist::{LkMaybeF64, LkMaybeI64};

type StrI64Map = HashMap<String, i64>;
type I64I64Map = HashMap<i64, i64>;
type StrF64Map = HashMap<String, f64>;
type I64F64Map = HashMap<i64, f64>;

/// Borrows the key as a `&str` (empty on null / invalid UTF-8).
///
/// # Safety
/// `key` must be a valid NUL-terminated C string, or null.
unsafe fn key_str<'a>(key: *const c_char) -> &'a str {
    if key.is_null() {
        return "";
    }
    // SAFETY: caller guarantees a valid NUL-terminated string.
    unsafe { CStr::from_ptr(key) }.to_str().unwrap_or("")
}

/// Creates a fresh, empty `Map<str, i64>` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_str_i64_new() -> *mut c_void {
    crate::state::arena_handle(StrI64Map::new())
}

/// Inserts or updates `map[key] = value`.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_str_i64_new`], or null; `key` a
/// valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_i64_set(handle: *mut c_void, key: *const c_char, value: i64) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `StrI64Map` from `lkrt_lkmap_str_i64_new`.
    let map = unsafe { &mut *(handle as *mut StrI64Map) };
    map.insert(unsafe { key_str(key) }.to_string(), value);
}

/// Returns the number of entries.
///
/// # Safety
/// See [`lkrt_lkmap_str_i64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_i64_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: `handle` addresses a `StrI64Map` from `lkrt_lkmap_str_i64_new`.
    unsafe { (*(handle as *mut StrI64Map)).len() as i64 }
}

/// Looks up `map[key]`, returning `Maybe<i64>` by value: a missing key yields
/// `present = 0` (the element is `nil`), matching the VM.
///
/// # Safety
/// See [`lkrt_lkmap_str_i64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_i64_get_pair(handle: *mut c_void, key: *const c_char) -> LkMaybeI64 {
    if handle.is_null() {
        return LkMaybeI64 { value: 0, present: 0 };
    }
    // SAFETY: as above.
    let map = unsafe { &*(handle as *mut StrI64Map) };
    match map.get(unsafe { key_str(key) }) {
        Some(&value) => LkMaybeI64 { value, present: 1 },
        None => LkMaybeI64 { value: 0, present: 0 },
    }
}

/// Creates a fresh, empty `Map<i64, i64>` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_i64_i64_new() -> *mut c_void {
    crate::state::arena_handle(I64I64Map::new())
}

/// Inserts or updates `map[key] = value`.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_i64_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_i64_i64_set(handle: *mut c_void, key: i64, value: i64) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses an `I64I64Map` from `lkrt_lkmap_i64_i64_new`.
    unsafe { (*(handle as *mut I64I64Map)).insert(key, value) };
}

/// Returns the number of entries.
///
/// # Safety
/// See [`lkrt_lkmap_i64_i64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_i64_i64_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as above.
    unsafe { (*(handle as *mut I64I64Map)).len() as i64 }
}

/// Looks up `map[key]`, returning `Maybe<i64>` by value (missing key → absent).
///
/// # Safety
/// See [`lkrt_lkmap_i64_i64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_i64_i64_get_pair(handle: *mut c_void, key: i64) -> LkMaybeI64 {
    if handle.is_null() {
        return LkMaybeI64 { value: 0, present: 0 };
    }
    // SAFETY: as above.
    let map = unsafe { &*(handle as *mut I64I64Map) };
    match map.get(&key) {
        Some(&value) => LkMaybeI64 { value, present: 1 },
        None => LkMaybeI64 { value: 0, present: 0 },
    }
}

/// Creates a fresh, empty `Map<str, f64>` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_str_f64_new() -> *mut c_void {
    crate::state::arena_handle(StrF64Map::new())
}

/// Inserts or updates `map[key] = value`.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_str_f64_new`], or null; `key` a
/// valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_f64_set(handle: *mut c_void, key: *const c_char, value: f64) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `StrF64Map` from `lkrt_lkmap_str_f64_new`.
    let map = unsafe { &mut *(handle as *mut StrF64Map) };
    map.insert(unsafe { key_str(key) }.to_string(), value);
}

/// Returns the number of entries.
///
/// # Safety
/// See [`lkrt_lkmap_str_f64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_f64_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as above.
    unsafe { (*(handle as *mut StrF64Map)).len() as i64 }
}

/// Looks up `map[key]`, returning `Maybe<f64>` by value (missing key → absent).
///
/// # Safety
/// See [`lkrt_lkmap_str_f64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_f64_get_pair(handle: *mut c_void, key: *const c_char) -> LkMaybeF64 {
    if handle.is_null() {
        return LkMaybeF64 { value: 0.0, present: 0 };
    }
    // SAFETY: as above.
    let map = unsafe { &*(handle as *mut StrF64Map) };
    match map.get(unsafe { key_str(key) }) {
        Some(&value) => LkMaybeF64 { value, present: 1 },
        None => LkMaybeF64 { value: 0.0, present: 0 },
    }
}

/// Creates a fresh, empty `Map<i64, f64>` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_i64_f64_new() -> *mut c_void {
    crate::state::arena_handle(I64F64Map::new())
}

/// Inserts or updates `map[key] = value`.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_i64_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_i64_f64_set(handle: *mut c_void, key: i64, value: f64) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses an `I64F64Map` from `lkrt_lkmap_i64_f64_new`.
    unsafe { (*(handle as *mut I64F64Map)).insert(key, value) };
}

/// Returns the number of entries.
///
/// # Safety
/// See [`lkrt_lkmap_i64_f64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_i64_f64_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as above.
    unsafe { (*(handle as *mut I64F64Map)).len() as i64 }
}

/// Looks up `map[key]`, returning `Maybe<f64>` by value (missing key → absent).
///
/// # Safety
/// See [`lkrt_lkmap_i64_f64_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_i64_f64_get_pair(handle: *mut c_void, key: i64) -> LkMaybeF64 {
    if handle.is_null() {
        return LkMaybeF64 { value: 0.0, present: 0 };
    }
    // SAFETY: as above.
    let map = unsafe { &*(handle as *mut I64F64Map) };
    match map.get(&key) {
        Some(&value) => LkMaybeF64 { value, present: 1 },
        None => LkMaybeF64 { value: 0.0, present: 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn i64_f64_set_get_missing() {
        unsafe {
            let h = lkrt_lkmap_i64_f64_new();
            lkrt_lkmap_i64_f64_set(h, 5, 1.5);
            assert_eq!(lkrt_lkmap_i64_f64_get_pair(h, 5).value, 1.5);
            assert_eq!(lkrt_lkmap_i64_f64_len(h), 1);
            assert_eq!(lkrt_lkmap_i64_f64_get_pair(h, 9).present, 0);
        }
    }

    #[test]
    fn str_f64_set_get_missing() {
        unsafe {
            let h = lkrt_lkmap_str_f64_new();
            let a = CString::new("a").unwrap();
            lkrt_lkmap_str_f64_set(h, a.as_ptr(), 1.5);
            assert_eq!(lkrt_lkmap_str_f64_get_pair(h, a.as_ptr()).value, 1.5);
            assert_eq!(lkrt_lkmap_str_f64_len(h), 1);
            let miss = CString::new("z").unwrap();
            assert_eq!(lkrt_lkmap_str_f64_get_pair(h, miss.as_ptr()).present, 0);
        }
    }

    #[test]
    fn i64_key_set_get_missing() {
        unsafe {
            let h = lkrt_lkmap_i64_i64_new();
            lkrt_lkmap_i64_i64_set(h, 5, 50);
            lkrt_lkmap_i64_i64_set(h, 7, 70);
            assert_eq!(lkrt_lkmap_i64_i64_get_pair(h, 5).value, 50);
            assert_eq!(lkrt_lkmap_i64_i64_len(h), 2);
            lkrt_lkmap_i64_i64_set(h, 5, 99); // update
            assert_eq!(lkrt_lkmap_i64_i64_get_pair(h, 5).value, 99);
            assert_eq!(lkrt_lkmap_i64_i64_get_pair(h, 123).present, 0);
        }
    }

    #[test]
    fn set_get_missing() {
        unsafe {
            let h = lkrt_lkmap_str_i64_new();
            let a = CString::new("a").unwrap();
            let b = CString::new("b").unwrap();
            lkrt_lkmap_str_i64_set(h, a.as_ptr(), 1);
            lkrt_lkmap_str_i64_set(h, b.as_ptr(), 2);
            let m = lkrt_lkmap_str_i64_get_pair(h, a.as_ptr());
            assert_eq!((m.value, m.present), (1, 1));
            // update
            lkrt_lkmap_str_i64_set(h, a.as_ptr(), 9);
            assert_eq!(lkrt_lkmap_str_i64_get_pair(h, a.as_ptr()).value, 9);
            // missing key -> absent
            let miss = CString::new("z").unwrap();
            assert_eq!(lkrt_lkmap_str_i64_get_pair(h, miss.as_ptr()).present, 0);
        }
    }
}
