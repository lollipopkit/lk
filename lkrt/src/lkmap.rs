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

use std::ffi::{CStr, c_char, c_void};

use rustc_hash::FxHashMap;

use crate::lklist::{LkMaybeF64, LkMaybeI64};

// FxHash instead of SipHash: map keys here are short strings / i64s hashed on
// every dynamic get/set, there is no iteration-order ABI (no keys/values/iter
// helper), and the inputs are program data, not untrusted external input.
type StrI64Map = FxHashMap<String, i64>;
type I64I64Map = FxHashMap<i64, i64>;
type StrF64Map = FxHashMap<String, f64>;
type I64F64Map = FxHashMap<i64, f64>;

/// Insert-or-update without allocating when the key is already present: the
/// common map workload pattern is repeated updates of existing keys, and
/// `insert(key.to_string(), ..)` would heap-allocate the key on every call.
fn set_str_key<V>(map: &mut FxHashMap<String, V>, key: &str, value: V) {
    match map.get_mut(key) {
        Some(slot) => *slot = value,
        None => {
            map.insert(key.to_string(), value);
        }
    }
}

/// Builds the composite `prefix ++ decimal(suffix)` key on the stack (heap only
/// for prefixes longer than the inline buffer) and passes it to `f` — the
/// zero-allocation key path for the `SetIndexStrI` map-store shape. Invalid
/// UTF-8 degrades to the empty key, matching [`key_str`].
///
/// # Safety
/// `prefix` must be a valid NUL-terminated C string, or null.
unsafe fn with_ik_key<R>(prefix: *const c_char, suffix: i64, f: impl FnOnce(&str) -> R) -> R {
    let prefix = if prefix.is_null() {
        b""
    } else {
        // SAFETY: caller guarantees a valid NUL-terminated string.
        unsafe { CStr::from_ptr(prefix) }.to_bytes()
    };
    let mut digits = [0u8; 20];
    let digits = crate::lkstr::i64_decimal(suffix, &mut digits);
    let total = prefix.len() + digits.len();
    let mut inline = [0u8; 88];
    let mut spill;
    let bytes: &mut [u8] = if total <= inline.len() {
        &mut inline[..total]
    } else {
        spill = vec![0u8; total];
        &mut spill[..]
    };
    bytes[..prefix.len()].copy_from_slice(prefix);
    bytes[prefix.len()..].copy_from_slice(digits);
    f(std::str::from_utf8(bytes).unwrap_or(""))
}

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
    crate::state::arena_handle(StrI64Map::default())
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
    set_str_key(map, unsafe { key_str(key) }, value);
}

/// Inserts or updates `map[prefix ++ decimal(suffix)] = value` — the composite
/// string-int key store (`m["n${i}"] = v`) with the key built on the stack, so
/// no key string is ever heap-allocated for the lookup (only a hit-miss insert
/// copies it).
///
/// # Safety
/// See [`lkrt_lkmap_str_i64_set`]; `prefix` a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_i64_set_ik(
    handle: *mut c_void,
    prefix: *const c_char,
    suffix: i64,
    value: i64,
) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `StrI64Map`; `prefix` per caller contract.
    let map = unsafe { &mut *(handle as *mut StrI64Map) };
    unsafe { with_ik_key(prefix, suffix, |key| set_str_key(map, key, value)) }
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

/// `{ ..rest }` over a `Map<str, i64>` (also the `Map<str, bool>` ABI): a fresh
/// handle with `key` removed, mirroring the VM's `MapRest`. Chained once per
/// removed key by the lowering.
///
/// # Safety
/// `handle` must be a live `Map<str, i64>` handle (or null → empty); `key` a
/// NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_i64_without(handle: *mut c_void, key: *const c_char) -> *mut c_void {
    let mut copy: StrI64Map = if handle.is_null() {
        StrI64Map::default()
    } else {
        // SAFETY: `handle` addresses a `StrI64Map` from `lkrt_lkmap_str_i64_new`.
        unsafe { (*(handle as *mut StrI64Map)).clone() }
    };
    copy.remove(unsafe { key_str(key) });
    crate::state::arena_handle(copy)
}

/// `{ ..rest }` over a `Map<str, f64>`. See [`lkrt_lkmap_str_i64_without`].
///
/// # Safety
/// `handle` must be a live `Map<str, f64>` handle (or null → empty); `key` a
/// NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_f64_without(handle: *mut c_void, key: *const c_char) -> *mut c_void {
    let mut copy: StrF64Map = if handle.is_null() {
        StrF64Map::default()
    } else {
        // SAFETY: `handle` addresses a `StrF64Map` from `lkrt_lkmap_str_f64_new`.
        unsafe { (*(handle as *mut StrF64Map)).clone() }
    };
    copy.remove(unsafe { key_str(key) });
    crate::state::arena_handle(copy)
}

/// Creates a fresh, empty `Map<i64, i64>` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_i64_i64_new() -> *mut c_void {
    crate::state::arena_handle(I64I64Map::default())
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
    crate::state::arena_handle(StrF64Map::default())
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
    set_str_key(map, unsafe { key_str(key) }, value);
}

/// Inserts or updates `map[prefix ++ decimal(suffix)] = value` (f64 values);
/// see [`lkrt_lkmap_str_i64_set_ik`].
///
/// # Safety
/// See [`lkrt_lkmap_str_f64_set`]; `prefix` a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_f64_set_ik(
    handle: *mut c_void,
    prefix: *const c_char,
    suffix: i64,
    value: f64,
) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `StrF64Map`; `prefix` per caller contract.
    let map = unsafe { &mut *(handle as *mut StrF64Map) };
    unsafe { with_ik_key(prefix, suffix, |key| set_str_key(map, key, value)) }
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
    crate::state::arena_handle(I64F64Map::default())
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
    fn set_ik_matches_materialized_key() {
        unsafe {
            let h = lkrt_lkmap_str_i64_new();
            let prefix = CString::new("n").unwrap();
            lkrt_lkmap_str_i64_set_ik(h, prefix.as_ptr(), 7, 70);
            lkrt_lkmap_str_i64_set_ik(h, prefix.as_ptr(), -3, 30);
            lkrt_lkmap_str_i64_set_ik(h, prefix.as_ptr(), 7, 71); // update, no alloc
            let k7 = CString::new("n7").unwrap();
            let km3 = CString::new("n-3").unwrap();
            assert_eq!(lkrt_lkmap_str_i64_get_pair(h, k7.as_ptr()).value, 71);
            assert_eq!(lkrt_lkmap_str_i64_get_pair(h, km3.as_ptr()).value, 30);
            assert_eq!(lkrt_lkmap_str_i64_len(h), 2);
            // Prefix longer than the inline stack buffer spills to the heap.
            let long = CString::new("p".repeat(120)).unwrap();
            lkrt_lkmap_str_i64_set_ik(h, long.as_ptr(), 1, 5);
            let long_key = CString::new(format!("{}1", "p".repeat(120))).unwrap();
            assert_eq!(lkrt_lkmap_str_i64_get_pair(h, long_key.as_ptr()).value, 5);

            let hf = lkrt_lkmap_str_f64_new();
            lkrt_lkmap_str_f64_set_ik(hf, prefix.as_ptr(), 2, 1.5);
            let k2 = CString::new("n2").unwrap();
            assert_eq!(lkrt_lkmap_str_f64_get_pair(hf, k2.as_ptr()).value, 1.5);
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
