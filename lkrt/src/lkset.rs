//! Native `Set` handles (plan deep-coverage B1): mirrors the VM's
//! `RuntimeSet` — a hash set of map keys (`Nil`/`Bool`/`Int`/strings; a
//! `Float` key is the VM's loud error, containers would need heap-handle
//! identity and stay out of the native subset). Elements arrive as boxed
//! `LkDyn` values; iteration/`values()` is *not* exposed (hash order).

// `alloc`, not the std prelude: this module is part of the computation-only
// subset that builds without an OS.
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use core::ffi::{CStr, c_char, c_void};

use crate::lkmap::FxSet;

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

type LkSet = FxSet<RtKey>;

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

/// `Set(list)` over a `List<Dyn>` handle — the boxed spelling.
///
/// Needed because a *constant* list is `List<Dyn>` as soon as its elements are
/// not one uniform type, and "one uniform type" splits strings by length:
/// `["ab", "aaaaaaaaaa"]` is a short string and a long one, so
/// `Set(["ab", "aaaaaaaaaa"])` had no arm while `Set(["ab", "z"])` did. Each
/// element goes through the same `key_from_dyn` the `add` path uses, so a
/// member the VM refuses (a float, a container) raises here too.
///
/// # Safety
/// `handle` must be a live `List<Dyn>` handle, or null (→ empty set).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_from_dyn_list(handle: *mut c_void) -> *mut c_void {
    let mut set = LkSet::default();
    if !handle.is_null() {
        // SAFETY: `handle` addresses a `Vec<LkDyn>` from `lkrt_lklist_dyn_new`.
        let items = unsafe { &*(handle as *mut Vec<LkDyn>) };
        for &item in items {
            set.insert(key_from_dyn(item));
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

/// The mirror of `RuntimeMapKey::display_order`: nil, then Bool, then Int by
/// value, then String by content.
///
/// A set's display order is the one container order that needs **no** mirror
/// discipline, because it is not the hash order — it is imposed, and imposed on
/// the members' *values*. So this is content comparison on both sides, and
/// nothing about hashers or table layout can drift it apart. (Iteration order,
/// `for x in s`, is a different question and still the hash order's.)
fn display_order(a: &RtKey, b: &RtKey) -> core::cmp::Ordering {
    fn kind(key: &RtKey) -> u8 {
        match key {
            RtKey::Nil => 0,
            RtKey::Bool(_) => 1,
            RtKey::Int(_) => 2,
            RtKey::Str(_) => 3,
        }
    }
    kind(a).cmp(&kind(b)).then_with(|| match (a, b) {
        (RtKey::Bool(x), RtKey::Bool(y)) => x.cmp(y),
        (RtKey::Int(x), RtKey::Int(y)) => x.cmp(y),
        (RtKey::Str(x), RtKey::Str(y)) => x.cmp(y),
        _ => core::cmp::Ordering::Equal,
    })
}

/// One member as `Set(…)` renders it: a string quoted with Rust's `{:?}` (the
/// VM's `quote_string`), everything else bare.
fn member_text(key: &RtKey) -> String {
    match key {
        RtKey::Nil => "nil".to_string(),
        RtKey::Bool(v) => v.to_string(),
        RtKey::Int(v) => v.to_string(),
        RtKey::Str(v) => format!("{v:?}"),
    }
}

/// `println(s)` → `Set([1,2,3])`, sorted by member.
///
/// # Safety
/// `handle` must be a live `Set` handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_display(handle: *mut c_void) -> *mut c_char {
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(set_text(handle)).unwrap_or_default())
}

/// `Set([1,2,3])` as text, sorted by member — also what the boxed-value
/// renderer calls, so a set inside a list renders through this one function.
pub(crate) fn set_text(handle: *mut c_void) -> String {
    let empty = LkSet::default();
    // SAFETY: caller passes a live `LkSet` handle.
    let set: &LkSet = if handle.is_null() {
        &empty
    } else {
        unsafe { &*(handle as *mut LkSet) }
    };
    let mut members: Vec<&RtKey> = set.iter().collect();
    members.sort_by(|a, b| display_order(a, b));
    let mut out = String::from("Set([");
    for (i, key) in members.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&member_text(key));
    }
    out.push_str("])");
    out
}

/// `a == b` → 0/1: same size and every member of `a` present in `b`.
///
/// Order-free, like the VM's — a set is its member set.
///
/// # Safety
/// Both handles must be live `Set` handles, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_eq(a: *mut c_void, b: *mut c_void) -> i64 {
    let empty = LkSet::default();
    // SAFETY: caller passes live `LkSet` handles.
    let borrow = |h: *mut c_void| -> &LkSet {
        if h.is_null() {
            &empty
        } else {
            unsafe { &*(h as *mut LkSet) }
        }
    };
    let (x, y) = (borrow(a), borrow(b));
    i64::from(x.len() == y.len() && x.iter().all(|k| y.contains(k)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lkdyn::{lkrt_dyn_from_i64, lkrt_dyn_from_str};
    use crate::lkstr::arena_c_string;
    use alloc::ffi::CString;

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
