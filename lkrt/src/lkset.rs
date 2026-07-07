//! Native `Set` handles (plan deep-coverage B1): mirrors the VM's
//! `RuntimeSet` — a hash set of map keys (`Nil`/`Bool`/`Int`/strings; a
//! `Float` key is the VM's loud error, containers would need heap-handle
//! identity and stay out of the native subset). Elements arrive as boxed
//! `LkDyn` values; iteration/`values()` is *not* exposed (hash order).

use core::ffi::{CStr, c_char, c_void};

use rustc_hash::FxHashSet;

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_NIL, DYN_STR, LkDyn};

/// The VM's `RuntimeMapKey` equality, minus heap-handle identity: the VM's
/// short/long string split is canonical by length, so plain content equality
/// is equivalent.
#[derive(Clone, PartialEq, Eq, Hash)]
enum RtKey {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
}

type LkSet = FxHashSet<RtKey>;

fn key_from_dyn(v: LkDyn) -> RtKey {
    match v.tag {
        DYN_NIL => RtKey::Nil,
        DYN_BOOL => RtKey::Bool(v.payload != 0),
        DYN_I64 => RtKey::Int(v.payload),
        DYN_STR => {
            let ptr = v.payload as *const c_char;
            let text = if ptr.is_null() {
                ""
            } else {
                // SAFETY: DYN_STR payloads are NUL-terminated arena strings.
                unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
            };
            RtKey::Str(text.to_owned())
        }
        // Float is the VM's loud "cannot be used as a key" error; containers
        // compare by heap-handle identity, which native cannot mirror. The
        // loud-failure contract compares success + stdout only, not text.
        DYN_F64 => crate::panic::raise_str("runtime error"),
        _ => crate::panic::raise_str("runtime error"),
    }
}

fn set_mut<'a>(handle: *mut c_void) -> &'a mut LkSet {
    // SAFETY: `handle` addresses an `LkSet` from `lkrt_lkset_new`/`from_*`.
    unsafe { &mut *(handle as *mut LkSet) }
}

/// Creates a fresh, empty `Set` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkset_new() -> *mut c_void {
    crate::state::arena_handle(LkSet::default())
}

/// `Set(list)` over a `List<str>` handle: inserts every element (duplicates
/// collapse, mirroring the VM's `runtime_set_from_value`).
///
/// # Safety
/// `handle` must be a live `List<str>` handle, or null (→ empty set).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_from_str_list(handle: *mut c_void) -> *mut c_void {
    let mut set = LkSet::default();
    if !handle.is_null() {
        // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
        let items = unsafe { &*(handle as *mut Vec<*const c_char>) };
        for &item in items {
            let text = if item.is_null() {
                ""
            } else {
                // SAFETY: list elements are NUL-terminated arena strings.
                unsafe { CStr::from_ptr(item) }.to_str().unwrap_or("")
            };
            set.insert(RtKey::Str(text.to_owned()));
        }
    }
    crate::state::arena_handle(set)
}

/// `Set(list)` over a `List<i64>` handle.
///
/// # Safety
/// `handle` must be a live `List<i64>` handle, or null (→ empty set).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_from_i64_list(handle: *mut c_void) -> *mut c_void {
    let mut set = LkSet::default();
    if !handle.is_null() {
        // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
        let items = unsafe { &*(handle as *mut Vec<i64>) };
        for &item in items {
            set.insert(RtKey::Int(item));
        }
    }
    crate::state::arena_handle(set)
}

/// `set.has(v)` / `set.contains(v)` → 0/1.
///
/// # Safety
/// `handle` must be a live `Set` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_has(handle: *mut c_void, value: LkDyn) -> i64 {
    let key = key_from_dyn(value);
    i64::from(set_mut(handle).contains(&key))
}

/// `set.add(v)` → 1 when newly inserted (the VM's `insert` result).
///
/// # Safety
/// `handle` must be a live `Set` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_add(handle: *mut c_void, value: LkDyn) -> i64 {
    let key = key_from_dyn(value);
    i64::from(set_mut(handle).insert(key))
}

/// `set.delete(v)` / `set.remove(v)` → 1 when it was present.
///
/// # Safety
/// `handle` must be a live `Set` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_delete(handle: *mut c_void, value: LkDyn) -> i64 {
    let key = key_from_dyn(value);
    i64::from(set_mut(handle).remove(&key))
}

/// `set.len()`.
///
/// # Safety
/// `handle` must be a live `Set` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_len(handle: *mut c_void) -> i64 {
    set_mut(handle).len() as i64
}

/// `set.clear()`.
///
/// # Safety
/// `handle` must be a live `Set` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_clear(handle: *mut c_void) {
    set_mut(handle).clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lkdyn::{lkrt_dyn_from_i64, lkrt_dyn_from_str};
    use crate::lkstr::arena_c_string;
    use std::ffi::CString;

    fn s(text: &str) -> LkDyn {
        let ptr = arena_c_string(CString::new(text).unwrap());
        lkrt_dyn_from_str(ptr)
    }

    #[test]
    fn set_deduplicates_and_mutates() {
        let list = crate::lklist::lkrt_lklist_str_new();
        for text in ["a", "b", "a"] {
            let ptr = arena_c_string(CString::new(text).unwrap());
            unsafe { crate::lklist::lkrt_lklist_str_push(list, ptr) };
        }
        let set = unsafe { lkrt_lkset_from_str_list(list) };
        unsafe {
            assert_eq!(lkrt_lkset_len(set), 2);
            assert_eq!(lkrt_lkset_has(set, s("a")), 1);
            assert_eq!(lkrt_lkset_has(set, s("zz")), 0);
            // add: 1 only when newly inserted; delete: 1 only when present.
            assert_eq!(lkrt_lkset_add(set, s("c")), 1);
            assert_eq!(lkrt_lkset_add(set, s("c")), 0);
            assert_eq!(lkrt_lkset_delete(set, s("a")), 1);
            assert_eq!(lkrt_lkset_delete(set, s("a")), 0);
            assert_eq!(lkrt_lkset_len(set), 2);
            // Int and Str keys never collide.
            assert_eq!(lkrt_lkset_has(set, lkrt_dyn_from_i64(1)), 0);
        }
    }
}
