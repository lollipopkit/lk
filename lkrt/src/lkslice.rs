//! List windows — what `xs.slice(a, b)` returns.
//!
//! A window is `(source handle, start, len)`, not a copy of the elements: the
//! VM's `HeapValue::Slice` (`core/src/val/runtime_model.rs`) in native form.
//! Until this module existed the native `.slice()` returned a fresh list, so
//! the same program had two different answers depending on the backend —
//! `w.to_list()` existed only on one side, and a write to the source showed
//! through the window on one side and not the other.
//!
//! The source is addressed **by handle**, re-read on every access. A pointer
//! into the `Vec`'s buffer would dangle the moment a `push` reallocated it;
//! going through the handle costs one extra load and cannot.
//!
//! Keeping the source alive is not this module's job but the ABI's: the
//! constructors here are annotated [`Receiver::ConstructsView`], which is what
//! stops the scope-drop pass from releasing a source that a live window still
//! points at.
//!
//! [`Receiver::ConstructsView`]: lk_aot_abi::Receiver::ConstructsView

// `alloc`, not the std prelude — same computation-only subset as `lklist`.
#[allow(unused_imports)]
use alloc::{boxed::Box, vec::Vec};

use core::ffi::c_void;

use crate::lklist::LkMaybeI64;

/// A window over a `Vec<i64>` handle.
///
/// `start`/`len` are positions in the *source*, already clamped to it at
/// construction. They are not re-clamped on read: a source that shrank after
/// the window was taken reads as absent element by element, which is what the
/// VM does (`slice_element` asks the list and takes nil for an answer).
pub struct LkSliceI64 {
    source: *mut c_void,
    start: usize,
    len: usize,
}

/// The elements of a live `Vec<i64>` handle, or empty for null.
///
/// # Safety
/// `handle` must be a live `i64` list handle, or null.
unsafe fn source_values<'a>(handle: *mut c_void) -> &'a [i64] {
    if handle.is_null() {
        return &[];
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    unsafe { &*(handle as *mut Vec<i64>) }
}

/// The window behind a handle, or `None` for null.
///
/// # Safety
/// `handle` must be a live window handle from [`lkrt_lkslice_i64_new`], or null.
unsafe fn window<'a>(handle: *mut c_void) -> Option<&'a LkSliceI64> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: `handle` addresses an `LkSliceI64` from `lkrt_lkslice_i64_new`.
    Some(unsafe { &*(handle as *mut LkSliceI64) })
}

/// A `slice` bound against a length: negative counts from the end, and the
/// result is clamped into `0..=len`. The VM's `slice_position` says the same.
fn resolve_position(index: i64, len: usize) -> usize {
    let len = len as i64;
    let resolved = if index < 0 { len + index } else { index };
    resolved.clamp(0, len) as usize
}

/// `xs.slice(start, end)` — a window over `xs`, no copy.
///
/// A negative bound counts from the end (`-1` is the last element, as in
/// `xs[-1]`) and past-the-end bounds clamp, matching `slice_position` in the VM.
/// It used to raise on a negative, which is what the VM did *for lists* — while
/// the VM's string slice clamped to 0 and this crate's string slice already
/// counted from the end. Four implementations, three conventions.
///
/// # Safety
/// `handle` must be a live `i64` list handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_new(handle: *mut c_void, start: i64, end: i64) -> *mut c_void {
    // SAFETY: the caller guarantees a live `i64` list handle or null.
    let source_len = unsafe { source_values(handle) }.len();
    let end = resolve_position(end, source_len);
    let start = resolve_position(start, source_len).min(end);
    crate::state::arena_handle(LkSliceI64 {
        source: handle,
        start,
        len: end - start,
    })
}

/// `w.len()`.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_len(handle: *mut c_void) -> i64 {
    // SAFETY: the caller guarantees a live window handle or null.
    unsafe { window(handle) }.map_or(0, |w| w.len as i64)
}

/// `w.is_empty()`, as `0`/`1`.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_is_empty(handle: *mut c_void) -> i64 {
    // SAFETY: the caller guarantees a live window handle or null.
    i64::from(unsafe { window(handle) }.is_none_or(|w| w.len == 0))
}

/// `w[i]` as `Maybe<i64>`: a negative index counts from the window's end, and
/// anything outside it is absent — the VM's `slice_element`, which resolves the
/// index against the window and then reads through to the source.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_get_pair(handle: *mut c_void, index: i64) -> LkMaybeI64 {
    const ABSENT: LkMaybeI64 = LkMaybeI64 { value: 0, present: 0 };
    // SAFETY: the caller guarantees a live window handle or null.
    let Some(w) = (unsafe { window(handle) }) else {
        return ABSENT;
    };
    let index = if index < 0 { w.len as i64 + index } else { index };
    if index < 0 || index as usize >= w.len {
        return ABSENT;
    }
    // SAFETY: `w.source` was a live `i64` list handle when the window was
    // taken, and the window keeps it alive (`Receiver::ConstructsView`).
    let values = unsafe { source_values(w.source) };
    match values.get(w.start + index as usize) {
        Some(&value) => LkMaybeI64 { value, present: 1 },
        // Only reachable if the source shrank after the window was taken.
        None => ABSENT,
    }
}

/// `w.slice(start, end)` — a window on a window, resolved against the *original*
/// source rather than nested, so that re-slicing in a loop does not build a
/// chain. Matches `dispatch_slice_builtin_method`.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_sub(handle: *mut c_void, start: i64, end: i64) -> *mut c_void {
    // SAFETY: the caller guarantees a live window handle or null.
    let Some(w) = (unsafe { window(handle) }) else {
        return crate::state::arena_handle(LkSliceI64 {
            source: core::ptr::null_mut(),
            start: 0,
            len: 0,
        });
    };
    let end = resolve_position(end, w.len);
    let start = resolve_position(start, w.len).min(end);
    crate::state::arena_handle(LkSliceI64 {
        source: w.source,
        start: w.start + start,
        len: end - start,
    })
}

/// The window's elements, without copying them.
///
/// Every read below goes through here rather than through `to_list`: a window
/// exists precisely so that asking it for a sum does not allocate a list first.
///
/// # Safety
/// `handle` must be a live window handle, or null.
unsafe fn window_values<'a>(handle: *mut c_void) -> &'a [i64] {
    // SAFETY: the caller guarantees a live window handle or null.
    let Some(w) = (unsafe { window(handle) }) else {
        return &[];
    };
    // SAFETY: as in `lkrt_lkslice_i64_get_pair`.
    let values = unsafe { source_values(w.source) };
    let end = (w.start + w.len).min(values.len());
    &values[w.start.min(end)..end]
}

/// `w.sum()` — wrapping, as the VM's list sum is.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_sum(handle: *mut c_void) -> i64 {
    // SAFETY: the caller guarantees a live window handle or null.
    unsafe { window_values(handle) }
        .iter()
        .fold(0i64, |total, value| total.wrapping_add(*value))
}

/// `w.min()` / `w.max()` — absent on an empty window, which is the nil the VM
/// answers there.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_min(handle: *mut c_void) -> crate::lkdyn::LkDyn {
    // SAFETY: the caller guarantees a live window handle or null.
    maybe(unsafe { window_values(handle) }.iter().min().copied())
}

/// The `max` half of [`lkrt_lkslice_i64_min`].
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_max(handle: *mut c_void) -> crate::lkdyn::LkDyn {
    // SAFETY: the caller guarantees a live window handle or null.
    maybe(unsafe { window_values(handle) }.iter().max().copied())
}

/// `w.contains(v)`.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_contains(handle: *mut c_void, value: i64) -> i64 {
    // SAFETY: the caller guarantees a live window handle or null.
    i64::from(unsafe { window_values(handle) }.contains(&value))
}

/// `w.count(v)` — how many elements of the window equal `v`.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_count(handle: *mut c_void, value: i64) -> i64 {
    // SAFETY: the caller guarantees a live window handle or null.
    unsafe { window_values(handle) }.iter().filter(|v| **v == value).count() as i64
}

/// `w.index_of(v)` — the position *within the window*, or absent.
///
/// Absent rather than `-1`: `-1` is a legal position (the last element), so
/// `w[w.index_of(v)]` would quietly read the end instead of failing.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_index_of(handle: *mut c_void, value: i64) -> crate::lkdyn::LkDyn {
    // SAFETY: the caller guarantees a live window handle or null.
    maybe(
        unsafe { window_values(handle) }
            .iter()
            .position(|candidate| *candidate == value)
            .map(|index| index as i64),
    )
}

/// `w.take(n)` / `w.skip(n)` — a sub-window, not a copy.
///
/// A separate entry from `sub` because a **count is not a position**: a
/// negative one is the refusal the VM gives, where `sub` would measure from
/// the end. Same split as `lklist`'s and `lkbytes`'s windows.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_take(handle: *mut c_void, count: i64) -> *mut c_void {
    if count < 0 {
        crate::panic::raise_str(&alloc::format!("slice.take() count must be non-negative, got {count}"));
    }
    // SAFETY: the caller guarantees a live window handle or null.
    unsafe { lkrt_lkslice_i64_sub(handle, 0, count.min(window_len(handle))) }
}

/// The `skip` half of [`lkrt_lkslice_i64_take`].
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_skip(handle: *mut c_void, count: i64) -> *mut c_void {
    if count < 0 {
        crate::panic::raise_str(&alloc::format!("slice.skip() count must be non-negative, got {count}"));
    }
    // SAFETY: the caller guarantees a live window handle or null.
    let len = window_len(handle);
    unsafe { lkrt_lkslice_i64_sub(handle, count.min(len), len) }
}

/// The window's length as an `i64`, for the two count guards above.
fn window_len(handle: *mut c_void) -> i64 {
    // SAFETY: the callers guarantee a live window handle or null.
    unsafe { window(handle) }.map_or(0, |w| w.len as i64)
}

/// An `Int?` answer as the boxed carrier every other optional-answering helper
/// uses (`bytes_h.min`, `list_h.i64_index_of`): a `Maybe<i64>` return is
/// declared in codegen rather than in the ABI table, and these are ordinary
/// table rows.
fn maybe(value: Option<i64>) -> crate::lkdyn::LkDyn {
    match value {
        Some(value) => crate::lkdyn::lkrt_dyn_from_i64(value),
        None => crate::lkdyn::LkDyn::NIL,
    }
}

/// `w.to_list()` — the copy, asked for explicitly.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_to_list(handle: *mut c_void) -> *mut c_void {
    // SAFETY: the caller guarantees a live window handle or null.
    let items: Vec<i64> = match unsafe { window(handle) } {
        // SAFETY: as in `lkrt_lkslice_i64_get_pair`.
        Some(w) => {
            let values = unsafe { source_values(w.source) };
            let end = (w.start + w.len).min(values.len());
            values[w.start.min(end)..end].to_vec()
        }
        None => Vec::new(),
    };
    crate::state::arena_handle(items)
}

/// The window's elements, for the boxed carrier's display and equality.
///
/// # Safety
/// `handle` must be a live window handle, or null.
pub(crate) unsafe fn window_elements<'a>(handle: *mut c_void) -> &'a [i64] {
    // SAFETY: the caller guarantees a live window handle or null.
    unsafe { window_values(handle) }
}

/// `println(w)` — the same rendering as the list it windows, because a window
/// *is* a list as far as the language is concerned.
pub(crate) fn slice_text(handle: *mut c_void) -> alloc::string::String {
    // SAFETY: callers pass a live window handle or null.
    let values = unsafe { window_values(handle) };
    let mut text = alloc::string::String::with_capacity(values.len() * 4 + 2);
    text.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&alloc::format!("{value}"));
    }
    text.push(']');
    text
}

/// `println(w)` / string interpolation.
///
/// # Safety
/// `handle` must be a live window handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkslice_i64_display(handle: *mut c_void) -> *mut core::ffi::c_char {
    // The text is built from the window directly rather than from a
    // materialized list: rendering is a read, and a read does not need a copy.
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(slice_text(handle)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `i64` list handle the way generated code does.
    fn list(values: &[i64]) -> *mut c_void {
        let handle = crate::lklist::lkrt_lklist_i64_new();
        for &value in values {
            unsafe { crate::lklist::lkrt_lklist_i64_push(handle, value) };
        }
        handle
    }

    fn read(window: *mut c_void, index: i64) -> Option<i64> {
        let got = unsafe { lkrt_lkslice_i64_get_pair(window, index) };
        (got.present != 0).then_some(got.value)
    }

    #[test]
    fn a_window_reads_through_to_its_source() {
        let source = list(&[3, 1, 4, 1, 5, 9, 2, 6]);
        let window = unsafe { lkrt_lkslice_i64_new(source, 1, 4) };
        assert_eq!(unsafe { lkrt_lkslice_i64_len(window) }, 3);
        // The window is `[1, 4, 1]`.
        assert_eq!(read(window, 0), Some(1));
        assert_eq!(read(window, 1), Some(4));
        assert_eq!(read(window, 2), Some(1));
        assert_eq!(read(window, -1), Some(1));
        assert_eq!(read(window, 3), None);
        assert_eq!(read(window, -4), None);
    }

    /// The point of a view: it is not a snapshot. A `push` that reallocates the
    /// source must not be able to leave the window pointing at freed memory,
    /// which is why the source is addressed by handle rather than by data
    /// pointer.
    #[test]
    fn a_window_sees_the_source_change_under_it() {
        let source = list(&[10, 20, 30]);
        let window = unsafe { lkrt_lkslice_i64_new(source, 0, 3) };
        for extra in 0..64 {
            unsafe { crate::lklist::lkrt_lklist_i64_push(source, extra) };
        }
        assert_eq!(read(window, 0), Some(10));
        assert_eq!(unsafe { lkrt_lkslice_i64_len(window) }, 3);
    }

    #[test]
    fn bounds_clamp_and_a_sub_window_resolves_against_the_original() {
        let source = list(&[0, 1, 2, 3, 4]);
        let window = unsafe { lkrt_lkslice_i64_new(source, 2, 99) };
        assert_eq!(unsafe { lkrt_lkslice_i64_len(window) }, 3);

        let inner = unsafe { lkrt_lkslice_i64_sub(window, 1, 3) };
        assert_eq!(unsafe { lkrt_lkslice_i64_len(inner) }, 2);
        assert_eq!(read(inner, 0), Some(3));
        assert_eq!(read(inner, 1), Some(4));
    }

    #[test]
    fn to_list_copies_exactly_the_window() {
        let source = list(&[7, 8, 9, 10]);
        let window = unsafe { lkrt_lkslice_i64_new(source, 1, 3) };
        let copied = unsafe { lkrt_lkslice_i64_to_list(window) };
        assert_eq!(unsafe { crate::lklist::lkrt_lklist_i64_len(copied) }, 2);
        let first = unsafe { crate::lklist::lkrt_lklist_i64_get_pair(copied, 0) };
        assert_eq!((first.value, first.present), (8, 1));
    }

    #[test]
    fn an_empty_window_is_empty() {
        let source = list(&[1, 2, 3]);
        let window = unsafe { lkrt_lkslice_i64_new(source, 2, 2) };
        assert_eq!(unsafe { lkrt_lkslice_i64_is_empty(window) }, 1);
        assert_eq!(read(window, 0), None);
    }
}
