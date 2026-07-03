//! Growable typed list handles for AOT (Phase 2 container handle-ification).
//!
//! Unlike the legacy caller-allocated fixed `[4096 x T]` buffers (see
//! `docs/llvm/native-stdlib.md`), a list is an opaque `*mut Vec<T>` handle that
//! grows without bound. Handles live in the runtime's default arena
//! (aot-redesign §3.4): registered on creation and reclaimed by `lkrt_cleanup`,
//! which generated entry code calls on the clean exit path.
//!
//! `get` follows the VM's indexing semantics exactly (see
//! `core/src/vm/exec/container/index.rs`): a negative index counts from the end,
//! and an out-of-range index yields "absent" (`present = 0`) rather than a value —
//! the caller models the result as `Maybe<Int>`.

use std::ffi::{CStr, CString, c_char, c_void};

/// Creates a fresh, empty `i64` list handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lklist_i64_new() -> *mut c_void {
    crate::state::arena_handle(Vec::<i64>::new())
}

/// `xs.map(f)` over an `i64` list with a compiled zero-capture lambda: calls
/// `f` per element in order and returns the fresh result list.
///
/// # Safety
/// `handle` must be a live `i64` list handle (or null → empty result); `f` a
/// valid `extern "C" fn(i64) -> i64` (a lowered `@lk_fn_N`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_map_fn(handle: *mut c_void, f: extern "C" fn(i64) -> i64) -> *mut c_void {
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<i64>` created by `lkrt_lklist_i64_new`.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    let mapped: Vec<i64> = values.iter().map(|&v| f(v)).collect();
    crate::state::arena_handle(mapped)
}

/// `xs.filter(p)` over an `i64` list: keeps the elements whose predicate holds.
///
/// # Safety
/// See [`lkrt_lklist_i64_map_fn`]; `p` returns the lambda's `Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_filter_fn(handle: *mut c_void, p: extern "C" fn(i64) -> bool) -> *mut c_void {
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: as above.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    let kept: Vec<i64> = values.iter().copied().filter(|&v| p(v)).collect();
    crate::state::arena_handle(kept)
}

/// `xs[start..]` over an `i64` list: elements from `start` onward (the VM's
/// `slice_from`). A negative `start` aborts (the VM requires it non-negative);
/// `start >= len` yields a fresh empty list. The result is a new handle.
///
/// # Safety
/// `handle` must be a live `i64` list handle (or null → empty result).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_slice_from(handle: *mut c_void, start: i64) -> *mut c_void {
    if start < 0 {
        crate::abi::flush_and_abort();
    }
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    let tail: Vec<i64> = values.iter().copied().skip(start as usize).collect();
    crate::state::arena_handle(tail)
}

/// `xs[start..]` over an `f64` list. See [`lkrt_lklist_i64_slice_from`].
///
/// # Safety
/// `handle` must be a live `f64` list handle (or null → empty result).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_slice_from(handle: *mut c_void, start: i64) -> *mut c_void {
    if start < 0 {
        crate::abi::flush_and_abort();
    }
    let values: &[f64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
        unsafe { &*(handle as *mut Vec<f64>) }
    };
    let tail: Vec<f64> = values.iter().copied().skip(start as usize).collect();
    crate::state::arena_handle(tail)
}

/// `xs[start..]` over a `str` list; elements are interned string-constant
/// pointers, copied as-is. See [`lkrt_lklist_i64_slice_from`].
///
/// # Safety
/// `handle` must be a live `str` list handle (or null → empty result).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_slice_from(handle: *mut c_void, start: i64) -> *mut c_void {
    if start < 0 {
        crate::abi::flush_and_abort();
    }
    let values: &[*const c_char] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
        unsafe { &*(handle as *mut Vec<*const c_char>) }
    };
    let tail: Vec<*const c_char> = values.iter().copied().skip(start as usize).collect();
    crate::state::arena_handle(tail)
}

/// `s.split(sep)` → a fresh `str` list handle. Uses Rust's `str::split`, so it
/// matches the VM's `string_split` exactly (same empty-part behavior on
/// leading/trailing/consecutive separators, and an empty separator splits
/// between every char). Each part is copied into an arena-owned C string so the
/// element pointers outlive the list (the str-list ABI otherwise expects
/// interned string-constant globals).
///
/// # Safety
/// `s` and `sep` must be NUL-terminated C strings (or null → treated as empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_str_split(s: *const c_char, sep: *const c_char) -> *mut c_void {
    let read = |p: *const c_char| -> &str {
        if p.is_null() {
            ""
        } else {
            // SAFETY: non-null pointers are NUL-terminated per the ABI.
            unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
        }
    };
    let (haystack, sep) = (read(s), read(sep));
    let parts: Vec<*const c_char> = haystack
        .split(sep)
        .map(|part| crate::lkstr::arena_c_string(CString::new(part).unwrap_or_default()) as *const c_char)
        .collect();
    crate::state::arena_handle(parts)
}

/// `xs.reduce(init, f)` over an `i64` list: left fold with `f(acc, element)`.
///
/// # Safety
/// See [`lkrt_lklist_i64_map_fn`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_reduce_fn(
    handle: *mut c_void,
    init: i64,
    f: extern "C" fn(i64, i64) -> i64,
) -> i64 {
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: as above.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    values.iter().fold(init, |acc, &v| f(acc, v))
}

/// Renders the list as the VM's display text (`[1,2,3]` — comma separated,
/// no spaces; see `runtime_display_list` in `stdlib/common`). Returned as an
/// owned, arena-registered C string.
///
/// # Safety
/// `handle` must be a live `i64` list handle, or null (renders `[]`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_display(handle: *mut c_void) -> *mut c_char {
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<i64>` created by `lkrt_lklist_i64_new`.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    display_joined(values.iter().map(i64::to_string))
}

/// `f64` list display (`[1.5,2]` — elements via Rust `f64::to_string`, the
/// VM's float display).
///
/// # Safety
/// See [`lkrt_lklist_i64_display`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_display(handle: *mut c_void) -> *mut c_char {
    let values: &[f64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<f64>` created by `lkrt_lklist_f64_new`.
        unsafe { &*(handle as *mut Vec<f64>) }
    };
    display_joined(values.iter().map(f64::to_string))
}

/// `str` list display (`["a","b c"]` — elements quoted/escaped with Rust's
/// `{:?}`, exactly the VM's `quote_string`).
///
/// # Safety
/// See [`lkrt_lklist_i64_display`]; elements must be valid C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_display(handle: *mut c_void) -> *mut c_char {
    let values: &[*const c_char] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
        unsafe { &*(handle as *mut Vec<*const c_char>) }
    };
    display_joined(values.iter().map(|&ptr| {
        let text = if ptr.is_null() {
            ""
        } else {
            // SAFETY: elements are NUL-terminated C strings per the list ABI.
            unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
        };
        format!("{text:?}")
    }))
}

/// `[e1,e2,…]` with the VM's separator convention, as an arena C string.
fn display_joined(parts: impl Iterator<Item = String>) -> *mut c_char {
    let mut out = String::from("[");
    for (i, part) in parts.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&part);
    }
    out.push(']');
    crate::lkstr::arena_c_string(std::ffi::CString::new(out).unwrap_or_default())
}

/// Appends `value` to the list.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_push(handle: *mut c_void, value: i64) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `Vec<i64>` created by `lkrt_lklist_i64_new`.
    unsafe { (*(handle as *mut Vec<i64>)).push(value) };
}

/// Returns the number of elements.
///
/// # Safety
/// See [`lkrt_lklist_i64_push`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as above.
    unsafe { (*(handle as *mut Vec<i64>)).len() as i64 }
}

/// Returns the element at a **caller-proven in-range, non-negative** index. Codegen
/// only emits this when the index is a compile-time constant within the list's
/// known length, so no bounds/`nil` handling is needed here (out-of-range would be
/// a codegen bug); returns `0` defensively if somehow out of range.
///
/// # Safety
/// See [`lkrt_lklist_i64_push`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_at(handle: *mut c_void, index: i64) -> i64 {
    if handle.is_null() || index < 0 {
        return 0;
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    let values = unsafe { &*(handle as *mut Vec<i64>) };
    values.get(index as usize).copied().unwrap_or(0)
}

/// Indexes the list with VM semantics: a negative index counts from the end, and
/// an out-of-range index sets `*present = 0` (the element is `nil`). On an in-range
/// access `*present = 1` and the element is returned.
///
/// # Safety
/// `handle` as above; `present` must be a valid writable `i64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_get(handle: *mut c_void, index: i64, present: *mut i64) -> i64 {
    if handle.is_null() {
        unsafe { *present = 0 };
        return 0;
    }
    // SAFETY: as above.
    let values = unsafe { &*(handle as *mut Vec<i64>) };
    let idx = if index < 0 { values.len() as i64 + index } else { index };
    if idx < 0 || idx as usize >= values.len() {
        unsafe { *present = 0 };
        0
    } else {
        unsafe { *present = 1 };
        values[idx as usize]
    }
}

/// Stores `value` at `index`. Unlike indexing (`get`), the VM treats an
/// out-of-range or **negative** store index as a fatal error (`list index N out of
/// bounds` / `list index must be non-negative`), not a nil/grow — so this
/// `abort()`s on an invalid index, matching the VM's *halt* (a loud failure, never
/// a silent wrong write). An in-range store is the only non-aborting path.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_set(handle: *mut c_void, index: i64, value: i64) {
    if handle.is_null() {
        crate::abi::flush_and_abort();
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    let values = unsafe { &mut *(handle as *mut Vec<i64>) };
    if index < 0 || index as usize >= values.len() {
        crate::abi::flush_and_abort();
    }
    values[index as usize] = value;
}

/// Stores `value` at `index` in an `f64` list; aborts on an invalid index (see
/// [`lkrt_lklist_i64_set`]).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_set(handle: *mut c_void, index: i64, value: f64) {
    if handle.is_null() {
        crate::abi::flush_and_abort();
    }
    // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
    let values = unsafe { &mut *(handle as *mut Vec<f64>) };
    if index < 0 || index as usize >= values.len() {
        crate::abi::flush_and_abort();
    }
    values[index as usize] = value;
}

/// A `Maybe<i64>` returned by value: `present == 0` means the element was absent
/// (out of range) and `value` is unspecified. `#[repr(C)]` with two `i64` fields
/// lowers to the SysV/LLVM `{i64, i64}` two-register return, so codegen can
/// `extractvalue` without an out-parameter or `alloca`.
#[repr(C)]
pub struct LkMaybeI64 {
    pub value: i64,
    pub present: i64,
}

/// A `Maybe<f64>` returned by value (`{double, i64}`): SysV returns the `f64` in
/// `xmm0` and `present` in `rax`, matching LLVM `{double, i64}`.
#[repr(C)]
pub struct LkMaybeF64 {
    pub value: f64,
    pub present: i64,
}

/// A `Maybe<str>` returned by value (`{ptr, i64}`): the string pointer in `rax`
/// and `present` in `rdx`, matching LLVM `{ptr, i64}`. `value` is unspecified
/// (null) when absent.
#[repr(C)]
pub struct LkMaybeStr {
    pub value: *const c_char,
    pub present: i64,
}

/// Dynamic-index read of a `str` list with VM semantics (negative-from-end,
/// out-of-range → `present = 0`), returning `Maybe<str>` by value. The `str`
/// counterpart of [`lkrt_lklist_i64_get_pair`].
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_str_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_get_pair(handle: *mut c_void, index: i64) -> LkMaybeStr {
    if handle.is_null() {
        return LkMaybeStr {
            value: std::ptr::null(),
            present: 0,
        };
    }
    // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
    let values = unsafe { &*(handle as *mut Vec<*const c_char>) };
    let idx = if index < 0 { values.len() as i64 + index } else { index };
    if idx < 0 || idx as usize >= values.len() {
        LkMaybeStr {
            value: std::ptr::null(),
            present: 0,
        }
    } else {
        LkMaybeStr {
            value: values[idx as usize],
            present: 1,
        }
    }
}

/// Unwraps a `Maybe<str>` in a string context, aborting if absent (see
/// [`lkrt_maybe_i64_unwrap`] — the VM halts when a `nil` element is used as a
/// string, e.g. concatenated or compared).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_maybe_str_unwrap(value: *const c_char, present: i64) -> *const c_char {
    if present == 0 {
        crate::abi::flush_and_abort();
    }
    value
}

/// Dynamic-index read of an `f64` list with VM semantics (negative-from-end,
/// out-of-range → `present = 0`), returning `Maybe<f64>` by value. The `f64`
/// counterpart of [`lkrt_lklist_i64_get_pair`].
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_get_pair(handle: *mut c_void, index: i64) -> LkMaybeF64 {
    if handle.is_null() {
        return LkMaybeF64 { value: 0.0, present: 0 };
    }
    // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
    let values = unsafe { &*(handle as *mut Vec<f64>) };
    let idx = if index < 0 { values.len() as i64 + index } else { index };
    if idx < 0 || idx as usize >= values.len() {
        LkMaybeF64 { value: 0.0, present: 0 }
    } else {
        LkMaybeF64 {
            value: values[idx as usize],
            present: 1,
        }
    }
}

/// Unwraps a `Maybe<f64>` in a scalar context, aborting if absent (see
/// [`lkrt_maybe_i64_unwrap`]).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_maybe_f64_unwrap(value: f64, present: i64) -> f64 {
    if present == 0 {
        crate::abi::flush_and_abort();
    }
    value
}

/// Unwraps a `Maybe<i64>` in a scalar (arithmetic/comparison) context: returns
/// `value` when `present != 0`, otherwise `abort()`s. This matches the VM, which
/// *halts* when a `nil` (out-of-range) element is used numerically (e.g.
/// `xs[oob] + 1`) — so an out-of-range index in arithmetic is a loud abort, never a
/// silent wrong value. In a `for x in xs` loop the index is always in range, so the
/// guard never fires.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_maybe_i64_unwrap(value: i64, present: i64) -> i64 {
    if present == 0 {
        crate::abi::flush_and_abort();
    }
    value
}

/// Dynamic-index read with exact VM semantics, returning `Maybe<i64>` by value: a
/// negative index counts from the end, and an out-of-range index yields
/// `present = 0` (the element is `nil`). This is the by-value counterpart of
/// [`lkrt_lklist_i64_get`], used by codegen for dynamic (not provably in-range)
/// indexing where the result must model `nil`.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_get_pair(handle: *mut c_void, index: i64) -> LkMaybeI64 {
    if handle.is_null() {
        return LkMaybeI64 { value: 0, present: 0 };
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    let values = unsafe { &*(handle as *mut Vec<i64>) };
    let idx = if index < 0 { values.len() as i64 + index } else { index };
    if idx < 0 || idx as usize >= values.len() {
        LkMaybeI64 { value: 0, present: 0 }
    } else {
        LkMaybeI64 {
            value: values[idx as usize],
            present: 1,
        }
    }
}

/// Linear membership test: returns `1` if `needle` is an element, else `0` (the
/// VM's `list.contains` on a typed int list — an exact `==` search).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_contains(handle: *mut c_void, needle: i64) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    let values = unsafe { &*(handle as *mut Vec<i64>) };
    i64::from(values.contains(&needle))
}

/// Linear membership test for an `f64` list (see [`lkrt_lklist_i64_contains`]).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_contains(handle: *mut c_void, needle: f64) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
    let values = unsafe { &*(handle as *mut Vec<f64>) };
    i64::from(values.contains(&needle))
}

/// Creates a fresh, empty `f64` list handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lklist_f64_new() -> *mut c_void {
    crate::state::arena_handle(Vec::<f64>::new())
}

/// Appends `value` to an `f64` list.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_push(handle: *mut c_void, value: f64) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
    unsafe { (*(handle as *mut Vec<f64>)).push(value) };
}

/// Returns the number of elements of an `f64` list.
///
/// # Safety
/// See [`lkrt_lklist_f64_push`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as above.
    unsafe { (*(handle as *mut Vec<f64>)).len() as i64 }
}

/// Returns the `f64` element at a caller-proven in-range, non-negative index (see
/// [`lkrt_lklist_i64_at`]).
///
/// # Safety
/// See [`lkrt_lklist_f64_push`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_at(handle: *mut c_void, index: i64) -> f64 {
    if handle.is_null() || index < 0 {
        return 0.0;
    }
    // SAFETY: as above.
    let values = unsafe { &*(handle as *mut Vec<f64>) };
    values.get(index as usize).copied().unwrap_or(0.0)
}

/// Creates a fresh, empty `List<str>` handle (a `Vec` of C-string pointers). The
/// pushed pointers reference interned string-constant globals, which live for the
/// whole program, so storing raw pointers never dangles.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lklist_str_new() -> *mut c_void {
    crate::state::arena_handle(Vec::<*const c_char>::new())
}

/// Appends a string pointer.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_str_new`], or null; `s` a valid
/// C string (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_push(handle: *mut c_void, s: *const c_char) {
    if handle.is_null() {
        return;
    }
    // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
    unsafe { (*(handle as *mut Vec<*const c_char>)).push(s) };
}

/// Returns the number of elements.
///
/// # Safety
/// See [`lkrt_lklist_str_push`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as above.
    unsafe { (*(handle as *mut Vec<*const c_char>)).len() as i64 }
}

/// Returns the element pointer at a caller-proven in-range, non-negative index (see
/// [`lkrt_lklist_i64_at`]); null defensively if out of range.
///
/// # Safety
/// See [`lkrt_lklist_str_push`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_at(handle: *mut c_void, index: i64) -> *const c_char {
    if handle.is_null() || index < 0 {
        return std::ptr::null();
    }
    // SAFETY: as above.
    let values = unsafe { &*(handle as *mut Vec<*const c_char>) };
    values.get(index as usize).copied().unwrap_or(std::ptr::null())
}

/// Joins the string elements with `separator`, returning a freshly allocated,
/// arena-registered C string. Matches the VM's `list.join` on a string list.
///
/// # Safety
/// `handle` as above; `separator` a valid C string (or null → empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_join(handle: *mut c_void, separator: *const c_char) -> *mut c_char {
    use std::ffi::CString;
    let sep = if separator.is_null() {
        ""
    } else {
        // SAFETY: caller guarantees a valid C string.
        unsafe { CStr::from_ptr(separator) }.to_str().unwrap_or("")
    };
    if handle.is_null() {
        return crate::lkstr::arena_c_string(CString::default());
    }
    // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
    let values = unsafe { &*(handle as *mut Vec<*const c_char>) };
    let parts: Vec<&str> = values
        .iter()
        .map(|&p| {
            if p.is_null() {
                ""
            } else {
                // SAFETY: elements are valid string-constant pointers.
                unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
            }
        })
        .collect();
    crate::lkstr::arena_c_string(CString::new(parts.join(sep)).unwrap_or_default())
}

/// Structural equality for two `i64` lists (1 = equal), the VM's typed-list
/// `==`: same length and element-wise `==`. Null handles compare as empty.
///
/// # Safety
/// Both handles must be live handles from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_eq(a: *mut c_void, b: *mut c_void) -> i64 {
    // SAFETY: handles address `Vec<i64>`s per the list ABI.
    let lhs: &[i64] = if a.is_null() {
        &[]
    } else {
        unsafe { &*(a as *mut Vec<i64>) }
    };
    let rhs: &[i64] = if b.is_null() {
        &[]
    } else {
        unsafe { &*(b as *mut Vec<i64>) }
    };
    i64::from(lhs == rhs)
}

/// Structural equality for two `f64` lists (element-wise `==`, so a NaN
/// element makes the lists unequal — the VM's float semantics).
///
/// # Safety
/// Both handles must be live handles from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_eq(a: *mut c_void, b: *mut c_void) -> i64 {
    // SAFETY: handles address `Vec<f64>`s per the list ABI.
    let lhs: &[f64] = if a.is_null() {
        &[]
    } else {
        unsafe { &*(a as *mut Vec<f64>) }
    };
    let rhs: &[f64] = if b.is_null() {
        &[]
    } else {
        unsafe { &*(b as *mut Vec<f64>) }
    };
    i64::from(lhs == rhs)
}

/// Structural equality of an `i64` list against an `f64` list: the VM
/// compares Int/Float typed lists with numeric coercion (`[1] == [1.0]`).
///
/// # Safety
/// `a` must be a live `i64`-list handle and `b` a live `f64`-list handle
/// (either may be null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_f64_eq(a: *mut c_void, b: *mut c_void) -> i64 {
    // SAFETY: handles address a `Vec<i64>` / `Vec<f64>` per the list ABI.
    let ints: &[i64] = if a.is_null() {
        &[]
    } else {
        unsafe { &*(a as *mut Vec<i64>) }
    };
    let floats: &[f64] = if b.is_null() {
        &[]
    } else {
        unsafe { &*(b as *mut Vec<f64>) }
    };
    let equal = ints.len() == floats.len() && ints.iter().zip(floats).all(|(&i, &f)| i as f64 == f);
    i64::from(equal)
}

/// Structural equality for two `str` lists (element bytes compared as C
/// strings; a null element equals only another null/empty element).
///
/// # Safety
/// Both handles must be live handles from [`lkrt_lklist_str_new`], or null;
/// elements must be valid C strings per the list ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_eq(a: *mut c_void, b: *mut c_void) -> i64 {
    // SAFETY: handles address `Vec<*const c_char>`s per the list ABI.
    let lhs: &[*const c_char] = if a.is_null() {
        &[]
    } else {
        unsafe { &*(a as *mut Vec<*const c_char>) }
    };
    let rhs: &[*const c_char] = if b.is_null() {
        &[]
    } else {
        unsafe { &*(b as *mut Vec<*const c_char>) }
    };
    let bytes = |p: *const c_char| {
        if p.is_null() {
            &b""[..]
        } else {
            // SAFETY: elements are NUL-terminated C strings per the list ABI.
            unsafe { CStr::from_ptr(p) }.to_bytes()
        }
    };
    let equal = lhs.len() == rhs.len() && lhs.iter().zip(rhs).all(|(&l, &r)| bytes(l) == bytes(r));
    i64::from(equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_structural_eq() {
        use std::ffi::CString;
        unsafe {
            let a = lkrt_lklist_i64_new();
            let b = lkrt_lklist_i64_new();
            for v in [1, 2, 3] {
                lkrt_lklist_i64_push(a, v);
                lkrt_lklist_i64_push(b, v);
            }
            assert_eq!(lkrt_lklist_i64_eq(a, b), 1);
            lkrt_lklist_i64_push(b, 4);
            assert_eq!(lkrt_lklist_i64_eq(a, b), 0);
            assert_eq!(lkrt_lklist_i64_eq(std::ptr::null_mut(), std::ptr::null_mut()), 1);

            let f = lkrt_lklist_f64_new();
            lkrt_lklist_f64_push(f, 1.0);
            lkrt_lklist_f64_push(f, 2.0);
            let g = lkrt_lklist_f64_new();
            lkrt_lklist_f64_push(g, 1.0);
            lkrt_lklist_f64_push(g, 2.0);
            assert_eq!(lkrt_lklist_f64_eq(f, g), 1);
            lkrt_lklist_f64_push(g, f64::NAN);
            lkrt_lklist_f64_push(f, f64::NAN);
            assert_eq!(lkrt_lklist_f64_eq(f, g), 0, "NaN element must break equality");

            let ints = lkrt_lklist_i64_new();
            lkrt_lklist_i64_push(ints, 1);
            let floats = lkrt_lklist_f64_new();
            lkrt_lklist_f64_push(floats, 1.0);
            assert_eq!(lkrt_lklist_i64_f64_eq(ints, floats), 1);
            lkrt_lklist_f64_push(floats, 0.5);
            assert_eq!(lkrt_lklist_i64_f64_eq(ints, floats), 0);

            let s1 = lkrt_lklist_str_new();
            let s2 = lkrt_lklist_str_new();
            let x1 = CString::new("x").unwrap();
            let x2 = CString::new("x").unwrap();
            lkrt_lklist_str_push(s1, x1.as_ptr());
            lkrt_lklist_str_push(s2, x2.as_ptr());
            assert_eq!(lkrt_lklist_str_eq(s1, s2), 1);
            let y = CString::new("y").unwrap();
            lkrt_lklist_str_push(s1, y.as_ptr());
            assert_eq!(lkrt_lklist_str_eq(s1, s2), 0);
        }
    }

    #[test]
    fn list_display_exact_bytes() {
        use std::ffi::CString;
        let text = |ptr: *mut c_char| {
            // SAFETY: display returns a NUL-terminated arena C string.
            unsafe { CStr::from_ptr(ptr) }.to_str().expect("utf8").to_string()
        };
        unsafe {
            let ints = lkrt_lklist_i64_new();
            assert_eq!(text(lkrt_lklist_i64_display(ints)), "[]");
            for v in [1, -2, 30] {
                lkrt_lklist_i64_push(ints, v);
            }
            assert_eq!(text(lkrt_lklist_i64_display(ints)), "[1,-2,30]");

            let floats = lkrt_lklist_f64_new();
            lkrt_lklist_f64_push(floats, 1.5);
            lkrt_lklist_f64_push(floats, 2.0);
            lkrt_lklist_f64_push(floats, 0.25);
            // Rust `f64::to_string` (the VM's float display): `2.0` renders `2`.
            assert_eq!(text(lkrt_lklist_f64_display(floats)), "[1.5,2,0.25]");

            let strs = lkrt_lklist_str_new();
            let a = CString::new("a").unwrap();
            let spaced = CString::new("b c").unwrap();
            let quoted = CString::new("he said \"hi\"\tok").unwrap();
            lkrt_lklist_str_push(strs, a.as_ptr());
            lkrt_lklist_str_push(strs, spaced.as_ptr());
            lkrt_lklist_str_push(strs, quoted.as_ptr());
            // Elements quote/escape with Rust `{:?}` (the VM's `quote_string`).
            assert_eq!(
                text(lkrt_lklist_str_display(strs)),
                "[\"a\",\"b c\",\"he said \\\"hi\\\"\\tok\"]"
            );
        }
    }

    #[test]
    fn str_list_join() {
        use std::ffi::CString;
        unsafe {
            let h = lkrt_lklist_str_new();
            let a = CString::new("a").unwrap();
            let b = CString::new("b").unwrap();
            let c = CString::new("c").unwrap();
            lkrt_lklist_str_push(h, a.as_ptr());
            lkrt_lklist_str_push(h, b.as_ptr());
            lkrt_lklist_str_push(h, c.as_ptr());
            assert_eq!(lkrt_lklist_str_len(h), 3);
            let sep = CString::new(", ").unwrap();
            let joined = lkrt_lklist_str_join(h, sep.as_ptr());
            assert_eq!(CStr::from_ptr(joined).to_bytes(), b"a, b, c");
            crate::lkrt_string_free(joined);
        }
    }

    #[test]
    fn str_get_pair_matches_vm_semantics() {
        use std::ffi::CString;
        unsafe {
            let h = lkrt_lklist_str_new();
            let a = CString::new("foo").unwrap();
            let b = CString::new("bar").unwrap();
            lkrt_lklist_str_push(h, a.as_ptr());
            lkrt_lklist_str_push(h, b.as_ptr());
            // In range.
            let hit = lkrt_lklist_str_get_pair(h, 1);
            assert_eq!(hit.present, 1);
            assert_eq!(CStr::from_ptr(hit.value).to_bytes(), b"bar");
            // Negative counts from the end.
            let neg = lkrt_lklist_str_get_pair(h, -2);
            assert_eq!(neg.present, 1);
            assert_eq!(CStr::from_ptr(neg.value).to_bytes(), b"foo");
            // Out of range / too negative → absent.
            assert_eq!(lkrt_lklist_str_get_pair(h, 2).present, 0);
            assert_eq!(lkrt_lklist_str_get_pair(h, -3).present, 0);
        }
    }

    #[test]
    fn set_in_range_mutates_element() {
        unsafe {
            let h = lkrt_lklist_i64_new();
            lkrt_lklist_i64_push(h, 10);
            lkrt_lklist_i64_push(h, 20);
            lkrt_lklist_i64_set(h, 1, 99);
            assert_eq!(lkrt_lklist_i64_at(h, 1), 99);
            assert_eq!(lkrt_lklist_i64_len(h), 2); // set never grows

            let g = lkrt_lklist_f64_new();
            lkrt_lklist_f64_push(g, 1.5);
            lkrt_lklist_f64_set(g, 0, 9.5);
            assert_eq!(lkrt_lklist_f64_at(g, 0), 9.5);
        }
    }

    #[test]
    fn get_pair_by_value_matches_vm_semantics() {
        unsafe {
            let h = lkrt_lklist_i64_new();
            lkrt_lklist_i64_push(h, 10);
            lkrt_lklist_i64_push(h, 20);
            lkrt_lklist_i64_push(h, 30);
            let m = lkrt_lklist_i64_get_pair(h, 1);
            assert_eq!((m.value, m.present), (20, 1));
            // negative counts from the end
            let m = lkrt_lklist_i64_get_pair(h, -1);
            assert_eq!((m.value, m.present), (30, 1));
            // out of range and too-negative -> absent
            assert_eq!(lkrt_lklist_i64_get_pair(h, 7).present, 0);
            assert_eq!(lkrt_lklist_i64_get_pair(h, -4).present, 0);
            // null handle -> absent
            assert_eq!(lkrt_lklist_i64_get_pair(std::ptr::null_mut(), 0).present, 0);
        }
    }

    #[test]
    fn f64_new_push_len() {
        unsafe {
            let h = lkrt_lklist_f64_new();
            lkrt_lklist_f64_push(h, 1.5);
            lkrt_lklist_f64_push(h, 2.5);
            assert_eq!(lkrt_lklist_f64_len(h), 2);
        }
    }

    #[test]
    fn new_push_len_get() {
        unsafe {
            let h = lkrt_lklist_i64_new();
            lkrt_lklist_i64_push(h, 10);
            lkrt_lklist_i64_push(h, 20);
            lkrt_lklist_i64_push(h, 30);
            assert_eq!(lkrt_lklist_i64_len(h), 3);
            let mut present = 0i64;
            assert_eq!(lkrt_lklist_i64_get(h, 0, &mut present), 10);
            assert_eq!(present, 1);
            assert_eq!(lkrt_lklist_i64_get(h, 2, &mut present), 30);
            assert_eq!(present, 1);
            // negative index counts from the end
            assert_eq!(lkrt_lklist_i64_get(h, -1, &mut present), 30);
            assert_eq!(present, 1);
            // out of range -> absent
            lkrt_lklist_i64_get(h, 3, &mut present);
            assert_eq!(present, 0);
            lkrt_lklist_i64_get(h, -4, &mut present);
            assert_eq!(present, 0);
        }
    }

    #[test]
    fn i64_hof_map_filter_reduce() {
        extern "C" fn double(v: i64) -> i64 {
            v * 2
        }
        extern "C" fn is_even(v: i64) -> bool {
            v % 2 == 0
        }
        extern "C" fn add(acc: i64, v: i64) -> i64 {
            acc + v
        }
        unsafe {
            let xs = lkrt_lklist_i64_new();
            for v in [1, 2, 3, 4, 5] {
                lkrt_lklist_i64_push(xs, v);
            }
            let mapped = lkrt_lklist_i64_map_fn(xs, double);
            assert_eq!(lkrt_lklist_i64_len(mapped), 5);
            let mut present = 0;
            assert_eq!(lkrt_lklist_i64_get(mapped, 4, &mut present), 10);
            assert_eq!(present, 1);
            let kept = lkrt_lklist_i64_filter_fn(xs, is_even);
            assert_eq!(lkrt_lklist_i64_len(kept), 2);
            assert_eq!(lkrt_lklist_i64_get(kept, 0, &mut present), 2);
            assert_eq!(present, 1);
            assert_eq!(lkrt_lklist_i64_reduce_fn(xs, 0, add), 15);
            assert_eq!(lkrt_lklist_i64_reduce_fn(std::ptr::null_mut(), 7, add), 7);
        }
    }
}
