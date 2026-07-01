//! Monomorphized typed container helpers for LK LLVM AOT binaries.
//!
//! These functions are pure: they only read from the caller-provided source
//! buffer and write into the caller-provided destination buffer. They never
//! touch the `lkrt` runtime state, allocate host resources, or depend on the
//! parser/compiler/VM. LLVM lowering used to emit these as hand-written IR
//! (`lk_*_i64_list`, `lk_*_f64_list`); centralizing them here removes the
//! per-shape IR and keeps VM/AOT container semantics in a single audited place.
//!
//! ABI contract (shared with the previous hand-written IR helpers):
//! - `len` is a non-negative element count; negative values are treated as `0`.
//! - Destination buffers are owned by the caller and are guaranteed large
//!   enough to hold the produced element count (the caller allocates fixed
//!   `[4096 x T]` slots). These helpers only write the produced prefix.
//! - `dst_len` out-pointers receive the produced element count.
//! - The source and destination buffers may alias (LLVM lowers in-place shapes
//!   such as `xs.slice(..)` / `xs.sort()` onto the same storage). Every helper
//!   is written to tolerate aliasing exactly like the old hand-written IR: it
//!   uses raw pointers (never overlapping `&`/`&mut` slices) and `ptr::copy`
//!   (memmove) for any range move, and only ever copies forward into a lower or
//!   equal destination index.

use std::{
    cmp::Ordering,
    ffi::{CStr, c_char},
    ptr, slice,
};

fn clamp_len(len: i64) -> usize {
    len.max(0) as usize
}

/// Borrows a C string, treating null as the empty string. Used by the string
/// list helpers, which compare/measure elements with `strcmp`/`strlen` semantics.
///
/// # Safety
/// When non-null, `p` must point to a NUL-terminated C string valid for `'a`.
unsafe fn cstr<'a>(p: *const c_char) -> &'a CStr {
    if p.is_null() {
        return c"";
    }
    // SAFETY: The caller guarantees a NUL-terminated string when non-null.
    unsafe { CStr::from_ptr(p) }
}

/// Duplicates a C string into a freshly leaked allocation, matching the old IR's
/// `strdup` ownership: the copy outlives the call and is never freed (native AOT
/// binaries are short-lived). Null becomes an owned empty string.
///
/// # Safety
/// When non-null, `p` must point to a NUL-terminated C string.
unsafe fn dup_cstr(p: *const c_char) -> *const c_char {
    let bytes = unsafe { cstr(p) }.to_bytes_with_nul();
    let boxed = bytes.to_vec().into_boxed_slice();
    Box::leak(boxed).as_ptr() as *const c_char
}

/// A stable empty C string pointer, used when a helper has no element to return
/// (matches the old hand-written IR's `@lk_empty_text`).
fn empty_cstr() -> *const c_char {
    c"".as_ptr()
}

/// # Safety
/// `ptr` must be valid for reads of `len` `T` values, or `len` must be `0`.
unsafe fn as_slice<'a, T>(ptr: *const T, len: i64) -> &'a [T] {
    let len = clamp_len(len);
    if len == 0 || ptr.is_null() {
        return &[];
    }
    // SAFETY: The caller guarantees `ptr` addresses `len` initialized T slots.
    unsafe { slice::from_raw_parts(ptr, len) }
}

/// # Safety
/// `ptr` must be valid for writes of at least `cap` `T` values, and must not
/// alias any live shared slice for the duration of the returned borrow.
unsafe fn as_mut_slice<'a, T>(ptr: *mut T, cap: usize) -> &'a mut [T] {
    if cap == 0 || ptr.is_null() {
        return &mut [];
    }
    // SAFETY: The caller guarantees `ptr` addresses at least `cap` writable slots.
    unsafe { slice::from_raw_parts_mut(ptr, cap) }
}

/// # Safety
/// `out` must be a valid, writable `i64` pointer.
unsafe fn store_len(out: *mut i64, value: usize) {
    if out.is_null() {
        return;
    }
    // SAFETY: The caller provides a valid out pointer for the produced length.
    unsafe {
        *out = value as i64;
    }
}

/// Moves `count` elements from `src` to `dst` using memmove semantics, so the
/// buffers are allowed to overlap. No-op when `count` is `0` or a pointer is null.
///
/// # Safety
/// When `count > 0`, `src`/`dst` must be valid for `count` reads/writes.
unsafe fn move_elems<T>(src: *const T, dst: *mut T, count: usize) {
    if count == 0 || src.is_null() || dst.is_null() {
        return;
    }
    // SAFETY: `ptr::copy` is memmove; overlapping ranges are well-defined.
    unsafe { ptr::copy(src, dst, count) };
}

// ---------------------------------------------------------------------------
// DynamicList<i64>
// ---------------------------------------------------------------------------

/// Returns `1` when `needle` is present, `0` otherwise.
///
/// # Safety
/// `values` must be valid for `len` reads (see module ABI contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_contains(values: *const i64, len: i64, needle: i64) -> i64 {
    let values = unsafe { as_slice(values, len) };
    i64::from(values.contains(&needle))
}

/// Returns the first index of `needle`, or `-1` when absent.
///
/// # Safety
/// `values` must be valid for `len` reads (see module ABI contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_index_of(values: *const i64, len: i64, needle: i64) -> i64 {
    let values = unsafe { as_slice(values, len) };
    match values.iter().position(|value| *value == needle) {
        Some(index) => index as i64,
        None => -1,
    }
}

/// Writes the reverse of `src` into `dst` and stores the new length. `src` and
/// `dst` must not alias (LLVM allocates a fresh list for reversal).
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_reverse(src: *const i64, src_len: i64, dst: *mut i64, dst_len: *mut i64) {
    let n = clamp_len(src_len);
    for i in 0..n {
        // SAFETY: both indices are < n and buffers hold at least n slots.
        unsafe { *dst.add(i) = *src.add(n - 1 - i) };
    }
    unsafe { store_len(dst_len, n) };
}

/// Writes an ascending sort of `src` into `dst` and stores the new length.
/// Tolerates `src == dst` (in-place sort).
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_sort(src: *const i64, src_len: i64, dst: *mut i64, dst_len: *mut i64) {
    let n = clamp_len(src_len);
    // Materialize into `dst` first (memmove tolerates aliasing), then sort in
    // place with a single exclusive borrow — no `&`/`&mut` alias is ever live.
    unsafe { move_elems(src, dst, n) };
    let dst = unsafe { as_mut_slice(dst, n) };
    dst[..n].sort_unstable();
    unsafe { store_len(dst_len, n) };
}

/// Returns the last element, or `0` when empty. Does not mutate the list.
///
/// # Safety
/// `values` must be valid for `len` reads (see module ABI contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_pop(values: *const i64, len: i64) -> i64 {
    let values = unsafe { as_slice(values, len) };
    values.last().copied().unwrap_or(0)
}

/// Writes `src[start..end]` (indices clamped to `0..=src_len`, `end >= start`)
/// into `dst` and stores the produced length. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_slice_range(
    src: *const i64,
    src_len: i64,
    start: i64,
    end: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let start = start.clamp(0, len);
    let end = end.clamp(0, len).max(start);
    let count = (end - start) as usize;
    // Destination index 0 <= source index `start`, so this is a forward move.
    unsafe { move_elems(src.add(start as usize), dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// Writes `src` followed by `value` into `dst` and stores `src_len + 1`.
/// Tolerates `src == dst` (append in place).
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len + 1` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_push(
    src: *const i64,
    src_len: i64,
    value: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    unsafe { move_elems(src, dst, n) };
    // SAFETY: `dst` holds at least n + 1 slots.
    unsafe { *dst.add(n) = value };
    unsafe { store_len(dst_len, n + 1) };
}

/// Inserts `value` at `index` (clamped to `0..=src_len`) and stores `src_len + 1`.
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len + 1` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_insert(
    src: *const i64,
    src_len: i64,
    index: i64,
    value: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    let index = index.clamp(0, n as i64) as usize;
    // Shift the tail right first (memmove handles the overlap), then drop the
    // new value in, then settle the (possibly no-op when aliased) head.
    unsafe { move_elems(src.add(index), dst.add(index + 1), n - index) };
    unsafe { move_elems(src, dst, index) };
    // SAFETY: `index <= n`, and `dst` holds at least n + 1 slots.
    unsafe { *dst.add(index) = value };
    unsafe { store_len(dst_len, n + 1) };
}

/// Removes the element at `index`, writing the compacted list into `dst` and
/// storing the new length. Returns the removed value, or `0` when `index` is out
/// of range (in which case every element is copied). Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_remove_at(
    src: *const i64,
    src_len: i64,
    index: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) -> i64 {
    let n = clamp_len(src_len);
    let mut removed = 0;
    let mut dst_i = 0;
    for i in 0..n {
        // SAFETY: `i < n`; `dst_i <= i`, so forward writes never clobber unread
        // source slots even when `src == dst`.
        let value = unsafe { *src.add(i) };
        if i as i64 == index {
            removed = value;
        } else {
            unsafe { *dst.add(dst_i) = value };
            dst_i += 1;
        }
    }
    unsafe { store_len(dst_len, dst_i) };
    removed
}

/// Copies `src` into `dst` with `dst[index] = value`, storing `src_len`. Returns
/// the previous value at `index`, or `0` when `index` is out of range. Tolerates
/// `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_set(
    src: *const i64,
    src_len: i64,
    index: i64,
    value: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) -> i64 {
    let n = clamp_len(src_len);
    let mut old = 0;
    for i in 0..n {
        // SAFETY: `i < n`; forward index writes are alias-safe (dst_i == i).
        let source = unsafe { *src.add(i) };
        if i as i64 == index {
            old = source;
            unsafe { *dst.add(i) = value };
        } else {
            unsafe { *dst.add(i) = source };
        }
    }
    unsafe { store_len(dst_len, n) };
    old
}

/// `xs.slice(start)`: writes `src[clamp(start)..src_len]`. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_slice(
    src: *const i64,
    src_len: i64,
    start: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let start = start.clamp(0, len);
    let count = (len - start) as usize;
    unsafe { move_elems(src.add(start as usize), dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `xs.take(count)`: writes the first `clamp(count, 0, src_len)` elements.
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_take(
    src: *const i64,
    src_len: i64,
    count: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let count = count.clamp(0, len) as usize;
    unsafe { move_elems(src, dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `lhs.concat(rhs)`: writes `lhs` followed by `rhs`. `dst` must not alias
/// `lhs`/`rhs` (LLVM allocates a fresh destination list).
///
/// # Safety
/// All buffers must satisfy the module ABI contract; `dst` holds `lhs_len + rhs_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_concat(
    lhs: *const i64,
    lhs_len: i64,
    rhs: *const i64,
    rhs_len: i64,
    dst: *mut i64,
    dst_len: *mut i64,
) {
    let lhs_n = clamp_len(lhs_len);
    let rhs_n = clamp_len(rhs_len);
    unsafe { move_elems(lhs, dst, lhs_n) };
    unsafe { move_elems(rhs, dst.add(lhs_n), rhs_n) };
    unsafe { store_len(dst_len, lhs_n + rhs_n) };
}

/// Returns `1` when the two lists are element-wise equal, `0` otherwise.
///
/// # Safety
/// `lhs`/`rhs` must be valid for their lengths of `i64` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_i64_eq(lhs: *const i64, lhs_len: i64, rhs: *const i64, rhs_len: i64) -> i64 {
    let a = unsafe { as_slice(lhs, lhs_len) };
    let b = unsafe { as_slice(rhs, rhs_len) };
    i64::from(a == b)
}

// ---------------------------------------------------------------------------
// DynamicList<f64>
//
// `f64` comparisons follow LLVM `fcmp oeq`/`fcmp ogt` semantics, which match
// Rust's `PartialEq`/`PartialOrd` for `f64` (NaN never equals or orders). The
// sort mirrors the old IR's O(n^2) `swap when a > b` pass so NaN placement is
// identical to the hand-written backend.
// ---------------------------------------------------------------------------

/// # Safety
/// `values` must be valid for `len` reads (see module ABI contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_contains(values: *const f64, len: i64, needle: f64) -> i64 {
    let values = unsafe { as_slice(values, len) };
    i64::from(values.iter().any(|value| *value == needle))
}

/// # Safety
/// `values` must be valid for `len` reads (see module ABI contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_index_of(values: *const f64, len: i64, needle: f64) -> i64 {
    let values = unsafe { as_slice(values, len) };
    match values.iter().position(|value| *value == needle) {
        Some(index) => index as i64,
        None => -1,
    }
}

/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `src`/`dst` must not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_reverse(src: *const f64, src_len: i64, dst: *mut f64, dst_len: *mut i64) {
    let n = clamp_len(src_len);
    for i in 0..n {
        unsafe { *dst.add(i) = *src.add(n - 1 - i) };
    }
    unsafe { store_len(dst_len, n) };
}

/// Ascending sort matching the hand-written IR's `swap when a > b` pass.
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_sort(src: *const f64, src_len: i64, dst: *mut f64, dst_len: *mut i64) {
    let n = clamp_len(src_len);
    unsafe { move_elems(src, dst, n) };
    let dst = unsafe { as_mut_slice(dst, n) };
    for i in 0..n {
        for j in (i + 1)..n {
            if dst[i] > dst[j] {
                dst.swap(i, j);
            }
        }
    }
    unsafe { store_len(dst_len, n) };
}

/// # Safety
/// `values` must be valid for `len` reads (see module ABI contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_pop(values: *const f64, len: i64) -> f64 {
    let values = unsafe { as_slice(values, len) };
    values.last().copied().unwrap_or(0.0)
}

/// `xs.slice(start)`: writes `src[clamp(start)..src_len]`. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_slice(
    src: *const f64,
    src_len: i64,
    start: i64,
    dst: *mut f64,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let start = start.clamp(0, len);
    let count = (len - start) as usize;
    unsafe { move_elems(src.add(start as usize), dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `xs.slice(start, end)`: writes `src[clamp(start)..clamp(end)]` (`end >= start`).
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_slice_range(
    src: *const f64,
    src_len: i64,
    start: i64,
    end: i64,
    dst: *mut f64,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let start = start.clamp(0, len);
    let end = end.clamp(0, len).max(start);
    let count = (end - start) as usize;
    unsafe { move_elems(src.add(start as usize), dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `xs.take(count)`: writes the first `clamp(count, 0, src_len)` elements.
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_take(
    src: *const f64,
    src_len: i64,
    count: i64,
    dst: *mut f64,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let count = count.clamp(0, len) as usize;
    unsafe { move_elems(src, dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `lhs.concat(rhs)`: writes `lhs` followed by `rhs`. `dst` must not alias `lhs`
/// or `rhs` (LLVM allocates a fresh destination list).
///
/// # Safety
/// All buffers must satisfy the module ABI contract; `dst` holds `lhs_len + rhs_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_concat(
    lhs: *const f64,
    lhs_len: i64,
    rhs: *const f64,
    rhs_len: i64,
    dst: *mut f64,
    dst_len: *mut i64,
) {
    let lhs_n = clamp_len(lhs_len);
    let rhs_n = clamp_len(rhs_len);
    unsafe { move_elems(lhs, dst, lhs_n) };
    unsafe { move_elems(rhs, dst.add(lhs_n), rhs_n) };
    unsafe { store_len(dst_len, lhs_n + rhs_n) };
}

/// `xs.unique()`: writes the order-preserving distinct elements. Tolerates
/// `src == dst` (destination index never exceeds the source index).
///
/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_unique(src: *const f64, src_len: i64, dst: *mut f64, dst_len: *mut i64) {
    let n = clamp_len(src_len);
    let mut dst_i = 0usize;
    for i in 0..n {
        // SAFETY: `i < n`; `dst_i <= i` keeps reads/writes within written prefix.
        let value = unsafe { *src.add(i) };
        let mut seen = false;
        for k in 0..dst_i {
            if unsafe { *dst.add(k) } == value {
                seen = true;
                break;
            }
        }
        if !seen {
            unsafe { *dst.add(dst_i) = value };
            dst_i += 1;
        }
    }
    unsafe { store_len(dst_len, dst_i) };
}

/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len + 1` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_push(
    src: *const f64,
    src_len: i64,
    value: f64,
    dst: *mut f64,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    unsafe { move_elems(src, dst, n) };
    unsafe { *dst.add(n) = value };
    unsafe { store_len(dst_len, n + 1) };
}

/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len + 1` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_insert(
    src: *const f64,
    src_len: i64,
    index: i64,
    value: f64,
    dst: *mut f64,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    let index = index.clamp(0, n as i64) as usize;
    unsafe { move_elems(src.add(index), dst.add(index + 1), n - index) };
    unsafe { move_elems(src, dst, index) };
    unsafe { *dst.add(index) = value };
    unsafe { store_len(dst_len, n + 1) };
}

/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_remove_at(
    src: *const f64,
    src_len: i64,
    index: i64,
    dst: *mut f64,
    dst_len: *mut i64,
) -> f64 {
    let n = clamp_len(src_len);
    let mut removed = 0.0;
    let mut dst_i = 0;
    for i in 0..n {
        let value = unsafe { *src.add(i) };
        if i as i64 == index {
            removed = value;
        } else {
            unsafe { *dst.add(dst_i) = value };
            dst_i += 1;
        }
    }
    unsafe { store_len(dst_len, dst_i) };
    removed
}

/// # Safety
/// `src`/`dst` must satisfy the module ABI contract; `dst` holds `src_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_f64_set(
    src: *const f64,
    src_len: i64,
    index: i64,
    value: f64,
    dst: *mut f64,
    dst_len: *mut i64,
) -> f64 {
    let n = clamp_len(src_len);
    let mut old = 0.0;
    for i in 0..n {
        let source = unsafe { *src.add(i) };
        if i as i64 == index {
            old = source;
            unsafe { *dst.add(i) = value };
        } else {
            unsafe { *dst.add(i) = source };
        }
    }
    unsafe { store_len(dst_len, n) };
    old
}

// ---------------------------------------------------------------------------
// DynamicList<str> (C string pointers)
//
// Elements are `*const c_char` slots. Structural helpers (reverse/sort/slice/
// pop/remove_at) only move the pointer values — they never copy the pointed-to
// text — exactly like the old IR. Only `push`/`insert`/`set` take ownership of
// the injected value by duplicating it (`dup_cstr`). Comparisons follow
// `strcmp` (byte-wise) via `CStr`.
// ---------------------------------------------------------------------------

/// # Safety
/// `values` must be valid for `len` `*const c_char` reads; each element must be
/// a valid C string or null. `needle` must be a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_contains(values: *const *const c_char, len: i64, needle: *const c_char) -> i64 {
    let n = clamp_len(len);
    let needle = unsafe { cstr(needle) };
    for i in 0..n {
        let value = unsafe { *values.add(i) };
        if unsafe { cstr(value) } == needle {
            return 1;
        }
    }
    0
}

/// # Safety
/// See [`lkrt_list_str_contains`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_index_of(values: *const *const c_char, len: i64, needle: *const c_char) -> i64 {
    let n = clamp_len(len);
    let needle = unsafe { cstr(needle) };
    for i in 0..n {
        let value = unsafe { *values.add(i) };
        if unsafe { cstr(value) } == needle {
            return i as i64;
        }
    }
    -1
}

/// Total byte length of all element strings (used by `join` length computation).
///
/// # Safety
/// `values` must be valid for `len` `*const c_char` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_text_len(values: *const *const c_char, len: i64) -> i64 {
    let n = clamp_len(len);
    let mut total = 0i64;
    for i in 0..n {
        let value = unsafe { *values.add(i) };
        total += unsafe { cstr(value) }.to_bytes().len() as i64;
    }
    total
}

/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes; they
/// must not alias (LLVM allocates a fresh list for reversal).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_reverse(
    src: *const *const c_char,
    src_len: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    for i in 0..n {
        unsafe { *dst.add(i) = *src.add(n - 1 - i) };
    }
    unsafe { store_len(dst_len, n) };
}

/// Ascending byte-wise (`strcmp`) sort. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_sort(
    src: *const *const c_char,
    src_len: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    unsafe { move_elems(src, dst, n) };
    for i in 0..n {
        for j in (i + 1)..n {
            let a = unsafe { *dst.add(i) };
            let b = unsafe { *dst.add(j) };
            if unsafe { cstr(a).cmp(cstr(b)) } == Ordering::Greater {
                unsafe {
                    *dst.add(i) = b;
                    *dst.add(j) = a;
                }
            }
        }
    }
    unsafe { store_len(dst_len, n) };
}

/// Returns the last element pointer, or an empty string when the list is empty.
///
/// # Safety
/// `values` must be valid for `len` `*const c_char` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_pop(values: *const *const c_char, len: i64) -> *const c_char {
    let n = clamp_len(len);
    if n == 0 {
        return empty_cstr();
    }
    unsafe { *values.add(n - 1) }
}

/// Writes `src[start..end]` (clamped to `0..=src_len`, `end >= start`) by moving
/// pointer values. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_slice_range(
    src: *const *const c_char,
    src_len: i64,
    start: i64,
    end: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let start = start.clamp(0, len);
    let end = end.clamp(0, len).max(start);
    let count = (end - start) as usize;
    unsafe { move_elems(src.add(start as usize), dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// Appends an owned copy of `value`. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len`/`src_len + 1` `*const c_char`
/// reads/writes; `value` must be a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_push(
    src: *const *const c_char,
    src_len: i64,
    value: *const c_char,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    unsafe { move_elems(src, dst, n) };
    unsafe { *dst.add(n) = dup_cstr(value) };
    unsafe { store_len(dst_len, n + 1) };
}

/// Inserts an owned copy of `value` at `index` (clamped to `0..=src_len`).
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len`/`src_len + 1` `*const c_char`
/// reads/writes; `value` must be a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_insert(
    src: *const *const c_char,
    src_len: i64,
    index: i64,
    value: *const c_char,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let n = clamp_len(src_len);
    let index = index.clamp(0, n as i64) as usize;
    unsafe { move_elems(src.add(index), dst.add(index + 1), n - index) };
    unsafe { move_elems(src, dst, index) };
    unsafe { *dst.add(index) = dup_cstr(value) };
    unsafe { store_len(dst_len, n + 1) };
}

/// Removes the element at `index` by moving pointers. Returns the removed
/// pointer, or an empty string when `index` is out of range / the list is empty.
/// Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_remove_at(
    src: *const *const c_char,
    src_len: i64,
    index: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) -> *const c_char {
    let n = clamp_len(src_len);
    let mut removed = empty_cstr();
    let mut dst_i = 0;
    for i in 0..n {
        let value = unsafe { *src.add(i) };
        if i as i64 == index {
            removed = value;
        } else {
            unsafe { *dst.add(dst_i) = value };
            dst_i += 1;
        }
    }
    unsafe { store_len(dst_len, dst_i) };
    removed
}

/// Copies pointers with `dst[index]` set to an owned copy of `value`. Returns the
/// previous pointer at `index`, or an empty string when out of range. Tolerates
/// `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes; `value`
/// must be a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_set(
    src: *const *const c_char,
    src_len: i64,
    index: i64,
    value: *const c_char,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) -> *const c_char {
    let n = clamp_len(src_len);
    let mut old = empty_cstr();
    for i in 0..n {
        let source = unsafe { *src.add(i) };
        if i as i64 == index {
            old = source;
            unsafe { *dst.add(i) = dup_cstr(value) };
        } else {
            unsafe { *dst.add(i) = source };
        }
    }
    unsafe { store_len(dst_len, n) };
    old
}

/// `xs.slice(start)`: moves pointers for `src[clamp(start)..src_len]`. Tolerates
/// `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_slice(
    src: *const *const c_char,
    src_len: i64,
    start: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let start = start.clamp(0, len);
    let count = (len - start) as usize;
    unsafe { move_elems(src.add(start as usize), dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `xs.take(count)`: moves pointers for the first `clamp(count, 0, src_len)`
/// elements. Tolerates `src == dst`.
///
/// # Safety
/// `src`/`dst` must be valid for `src_len` `*const c_char` reads/writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_take(
    src: *const *const c_char,
    src_len: i64,
    count: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let len = clamp_len(src_len) as i64;
    let count = count.clamp(0, len) as usize;
    unsafe { move_elems(src, dst, count) };
    unsafe { store_len(dst_len, count) };
}

/// `lhs.concat(rhs)`: moves `lhs` pointers followed by `rhs` pointers (no text is
/// duplicated). `dst` must not alias `lhs`/`rhs`.
///
/// # Safety
/// All buffers must be valid for their lengths of `*const c_char` reads/writes;
/// `dst` holds `lhs_len + rhs_len` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_list_str_concat(
    lhs: *const *const c_char,
    lhs_len: i64,
    rhs: *const *const c_char,
    rhs_len: i64,
    dst: *mut *const c_char,
    dst_len: *mut i64,
) {
    let lhs_n = clamp_len(lhs_len);
    let rhs_n = clamp_len(rhs_len);
    unsafe { move_elems(lhs, dst, lhs_n) };
    unsafe { move_elems(rhs, dst.add(lhs_n), rhs_n) };
    unsafe { store_len(dst_len, lhs_n + rhs_n) };
}

// ---------------------------------------------------------------------------
// DynamicMap<i64, V> (linear-probe association arrays)
//
// Maps are stored as parallel key/value arrays. `lookup` writes the found value
// through `out` and returns 1/0 (the caller stores that into its `present` slot,
// keeping the `nil`-vs-value distinction). `set` updates in place or appends,
// returning the new length. String values are duplicated on the caller side
// (`strdup`), so the map helpers only move the pointer.
// ---------------------------------------------------------------------------

/// # Safety
/// `keys`/`values` valid for `len` reads; `out` a valid writable `V` pointer.
unsafe fn map_lookup<K: Copy + PartialEq, V: Copy>(
    keys: *const K,
    values: *const V,
    len: i64,
    key: K,
    out: *mut V,
) -> i64 {
    let n = clamp_len(len);
    for i in 0..n {
        if unsafe { *keys.add(i) } == key {
            unsafe { *out = *values.add(i) };
            return 1;
        }
    }
    0
}

/// # Safety
/// `keys`/`values` valid for `len` reads and one append slot (`len + 1`).
unsafe fn map_set<K: Copy + PartialEq, V: Copy>(keys: *mut K, values: *mut V, len: i64, key: K, value: V) -> i64 {
    let n = clamp_len(len);
    for i in 0..n {
        if unsafe { *keys.add(i) } == key {
            unsafe { *values.add(i) = value };
            return len;
        }
    }
    unsafe {
        *keys.add(n) = key;
        *values.add(n) = value;
    }
    len + 1
}

/// # Safety
/// See [`map_lookup`]. `out` must be a valid writable `i64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_i64_int_lookup(
    keys: *const i64,
    values: *const i64,
    len: i64,
    key: i64,
    out: *mut i64,
) -> i64 {
    unsafe { map_lookup(keys, values, len, key, out) }
}

/// # Safety
/// See [`map_lookup`]. `out` must be a valid writable `f64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_i64_f64_lookup(
    keys: *const i64,
    values: *const f64,
    len: i64,
    key: i64,
    out: *mut f64,
) -> i64 {
    unsafe { map_lookup(keys, values, len, key, out) }
}

/// # Safety
/// See [`map_lookup`]. `out` must be a valid writable `*const c_char` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_i64_ptr_lookup(
    keys: *const i64,
    values: *const *const c_char,
    len: i64,
    key: i64,
    out: *mut *const c_char,
) -> i64 {
    unsafe { map_lookup(keys, values, len, key, out) }
}

/// # Safety
/// See [`map_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_i64_int_set(keys: *mut i64, values: *mut i64, len: i64, key: i64, value: i64) -> i64 {
    unsafe { map_set(keys, values, len, key, value) }
}

/// # Safety
/// See [`map_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_i64_f64_set(keys: *mut i64, values: *mut f64, len: i64, key: i64, value: f64) -> i64 {
    unsafe { map_set(keys, values, len, key, value) }
}

/// # Safety
/// See [`map_set`]. The pointer value is stored as-is (caller owns/duplicates it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_i64_ptr_set(
    keys: *mut i64,
    values: *mut *const c_char,
    len: i64,
    key: i64,
    value: *const c_char,
) -> i64 {
    unsafe { map_set(keys, values, len, key, value) }
}

// ---------------------------------------------------------------------------
// DynamicMap<str, V> (composite short-string key: prefix string + integer)
//
// LK maps whose keys look like `"n123"` / `"item42"` are stored with the numeric
// suffix split out into a parallel `numbers` array, so lookups compare the string
// prefix with `strcmp` plus an integer `==` instead of comparing the whole key.
// `split_key` scans trailing ASCII digits: a key that is all digits or has no
// trailing digit is "raw" (prefix = the original pointer, number = 0); otherwise
// the prefix is a leaked truncated copy and the number is the parsed suffix.
// ---------------------------------------------------------------------------

/// # Safety
/// `prefixes`/`numbers`/`values` valid for `len` reads; `prefix` a valid C string
/// or null; `out` a valid writable `V` pointer.
unsafe fn str_map_lookup<V: Copy>(
    prefixes: *const *const c_char,
    numbers: *const i64,
    values: *const V,
    len: i64,
    prefix: *const c_char,
    number: i64,
    out: *mut V,
) -> i64 {
    let n = clamp_len(len);
    let needle = unsafe { cstr(prefix) };
    for i in 0..n {
        if unsafe { cstr(*prefixes.add(i)) } == needle && unsafe { *numbers.add(i) } == number {
            unsafe { *out = *values.add(i) };
            return 1;
        }
    }
    0
}

/// # Safety
/// Arrays valid for `len` reads plus one append slot; `prefix` stored as-is
/// (the caller owns/duplicates it via `split_key`).
unsafe fn str_map_set<V: Copy>(
    prefixes: *mut *const c_char,
    numbers: *mut i64,
    values: *mut V,
    len: i64,
    prefix: *const c_char,
    number: i64,
    value: V,
) -> i64 {
    let n = clamp_len(len);
    let needle = unsafe { cstr(prefix) };
    for i in 0..n {
        if unsafe { cstr(*prefixes.add(i)) } == needle && unsafe { *numbers.add(i) } == number {
            unsafe { *values.add(i) = value };
            return len;
        }
    }
    unsafe {
        *prefixes.add(n) = prefix;
        *numbers.add(n) = number;
        *values.add(n) = value;
    }
    len + 1
}

/// Copies every entry whose composite key differs from `(prefix, number)` into the
/// destination arrays, compacting out the (at most one) matching entry. The removed
/// value is written through `out_value` and `out_present` is set to `1` on a hit;
/// the caller pre-seeds both with the "missing" defaults, so a miss leaves them
/// untouched. Returns the destination length. Tolerates `src == dst` (the
/// destination index never exceeds the source index, so forward writes are safe).
///
/// # Safety
/// The source arrays must be valid for `len` reads and the destination arrays for
/// `len` writes; `out_value`/`out_present` must be valid writable pointers.
#[allow(clippy::too_many_arguments)]
unsafe fn str_map_delete<V: Copy>(
    src_prefixes: *const *const c_char,
    src_numbers: *const i64,
    src_values: *const V,
    len: i64,
    dst_prefixes: *mut *const c_char,
    dst_numbers: *mut i64,
    dst_values: *mut V,
    prefix: *const c_char,
    number: i64,
    out_value: *mut V,
    out_present: *mut i64,
) -> i64 {
    let n = clamp_len(len);
    let needle = unsafe { cstr(prefix) };
    let mut dst_i = 0usize;
    for i in 0..n {
        // Read the whole entry before writing, so `src == dst` stays sound
        // (`dst_i <= i`, i.e. writes only ever land on an already-read slot).
        let p = unsafe { *src_prefixes.add(i) };
        let num = unsafe { *src_numbers.add(i) };
        let v = unsafe { *src_values.add(i) };
        if unsafe { cstr(p) } == needle && num == number {
            unsafe {
                *out_value = v;
                *out_present = 1;
            }
        } else {
            unsafe {
                *dst_prefixes.add(dst_i) = p;
                *dst_numbers.add(dst_i) = num;
                *dst_values.add(dst_i) = v;
            }
            dst_i += 1;
        }
    }
    dst_i as i64
}

/// Returns `1` when the composite key `(prefix, number)` is present, `0` otherwise.
///
/// # Safety
/// `prefixes`/`numbers` valid for `len` reads; `prefix` a valid C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_contains(
    prefixes: *const *const c_char,
    numbers: *const i64,
    len: i64,
    prefix: *const c_char,
    number: i64,
) -> i64 {
    let n = clamp_len(len);
    let needle = unsafe { cstr(prefix) };
    for i in 0..n {
        if unsafe { cstr(*prefixes.add(i)) } == needle && unsafe { *numbers.add(i) } == number {
            return 1;
        }
    }
    0
}

/// Splits a composite string key into a prefix pointer (written through
/// `prefix_out`) and returns the integer suffix. A key that is all digits or has
/// no trailing digit is "raw": `prefix_out` gets the original pointer and the
/// result is `0`. Otherwise `prefix_out` gets a leaked truncated copy and the
/// result is the parsed trailing-digit suffix.
///
/// # Safety
/// `key` a valid C string or null; `prefix_out` a valid writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_split_key(key: *const c_char, prefix_out: *mut *const c_char) -> i64 {
    let bytes = unsafe { cstr(key) }.to_bytes();
    let len = bytes.len();
    let mut start = len;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == 0 || start == len {
        // all digits, or no trailing digit -> keep the original pointer, number 0
        unsafe { *prefix_out = key };
        return 0;
    }
    let mut prefix = Vec::with_capacity(start + 1);
    prefix.extend_from_slice(&bytes[..start]);
    prefix.push(0);
    unsafe { *prefix_out = Box::leak(prefix.into_boxed_slice()).as_ptr() as *const c_char };
    let mut acc: i64 = 0;
    for &b in &bytes[start..] {
        acc = acc.wrapping_mul(10).wrapping_add(i64::from(b - b'0'));
    }
    acc
}

/// # Safety
/// See [`str_map_lookup`]. `out` must be a valid writable `i64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_int_lookup(
    prefixes: *const *const c_char,
    numbers: *const i64,
    values: *const i64,
    len: i64,
    prefix: *const c_char,
    number: i64,
    out: *mut i64,
) -> i64 {
    unsafe { str_map_lookup(prefixes, numbers, values, len, prefix, number, out) }
}

/// # Safety
/// See [`str_map_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_int_set(
    prefixes: *mut *const c_char,
    numbers: *mut i64,
    values: *mut i64,
    len: i64,
    prefix: *const c_char,
    number: i64,
    value: i64,
) -> i64 {
    unsafe { str_map_set(prefixes, numbers, values, len, prefix, number, value) }
}

/// # Safety
/// See [`str_map_lookup`]. `out` must be a valid writable `f64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_f64_lookup(
    prefixes: *const *const c_char,
    numbers: *const i64,
    values: *const f64,
    len: i64,
    prefix: *const c_char,
    number: i64,
    out: *mut f64,
) -> i64 {
    unsafe { str_map_lookup(prefixes, numbers, values, len, prefix, number, out) }
}

/// # Safety
/// See [`str_map_set`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_f64_set(
    prefixes: *mut *const c_char,
    numbers: *mut i64,
    values: *mut f64,
    len: i64,
    prefix: *const c_char,
    number: i64,
    value: f64,
) -> i64 {
    unsafe { str_map_set(prefixes, numbers, values, len, prefix, number, value) }
}

/// # Safety
/// See [`str_map_lookup`]. `out` must be a valid writable `*const c_char` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_ptr_lookup(
    prefixes: *const *const c_char,
    numbers: *const i64,
    values: *const *const c_char,
    len: i64,
    prefix: *const c_char,
    number: i64,
    out: *mut *const c_char,
) -> i64 {
    unsafe { str_map_lookup(prefixes, numbers, values, len, prefix, number, out) }
}

/// # Safety
/// See [`str_map_set`]. The pointer value is stored as-is (caller owns/duplicates it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_ptr_set(
    prefixes: *mut *const c_char,
    numbers: *mut i64,
    values: *mut *const c_char,
    len: i64,
    prefix: *const c_char,
    number: i64,
    value: *const c_char,
) -> i64 {
    unsafe { str_map_set(prefixes, numbers, values, len, prefix, number, value) }
}

/// # Safety
/// See [`str_map_delete`]. `out_value` must be a valid writable `i64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_int_delete(
    src_prefixes: *const *const c_char,
    src_numbers: *const i64,
    src_values: *const i64,
    len: i64,
    dst_prefixes: *mut *const c_char,
    dst_numbers: *mut i64,
    dst_values: *mut i64,
    prefix: *const c_char,
    number: i64,
    out_value: *mut i64,
    out_present: *mut i64,
) -> i64 {
    unsafe {
        str_map_delete(
            src_prefixes,
            src_numbers,
            src_values,
            len,
            dst_prefixes,
            dst_numbers,
            dst_values,
            prefix,
            number,
            out_value,
            out_present,
        )
    }
}

/// # Safety
/// See [`str_map_delete`]. `out_value` must be a valid writable `f64` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_f64_delete(
    src_prefixes: *const *const c_char,
    src_numbers: *const i64,
    src_values: *const f64,
    len: i64,
    dst_prefixes: *mut *const c_char,
    dst_numbers: *mut i64,
    dst_values: *mut f64,
    prefix: *const c_char,
    number: i64,
    out_value: *mut f64,
    out_present: *mut i64,
) -> i64 {
    unsafe {
        str_map_delete(
            src_prefixes,
            src_numbers,
            src_values,
            len,
            dst_prefixes,
            dst_numbers,
            dst_values,
            prefix,
            number,
            out_value,
            out_present,
        )
    }
}

/// # Safety
/// See [`str_map_delete`]. `out_value` must be a valid writable `*const c_char`
/// pointer; the removed pointer is moved as-is (no free).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_map_str_ptr_delete(
    src_prefixes: *const *const c_char,
    src_numbers: *const i64,
    src_values: *const *const c_char,
    len: i64,
    dst_prefixes: *mut *const c_char,
    dst_numbers: *mut i64,
    dst_values: *mut *const c_char,
    prefix: *const c_char,
    number: i64,
    out_value: *mut *const c_char,
    out_present: *mut i64,
) -> i64 {
    unsafe {
        str_map_delete(
            src_prefixes,
            src_numbers,
            src_values,
            len,
            dst_prefixes,
            dst_numbers,
            dst_values,
            prefix,
            number,
            out_value,
            out_present,
        )
    }
}

/// Returns the number of characters in the decimal spelling of `value`, counting
/// a leading `-` for negatives (used to size template string buffers).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_decimal_len(value: i64) -> i64 {
    if value == 0 {
        return 1;
    }
    let mut len = i64::from(value < 0);
    let mut v = value;
    loop {
        len += 1;
        let done = if value >= 0 { v < 10 } else { v > -10 };
        if done {
            break;
        }
        v /= 10;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i64_ptr(values: &[i64]) -> (*const i64, i64) {
        (values.as_ptr(), values.len() as i64)
    }

    fn f64_ptr(values: &[f64]) -> (*const f64, i64) {
        (values.as_ptr(), values.len() as i64)
    }

    #[test]
    fn i64_contains_and_index_of() {
        let values = [3i64, 1, 4, 1, 5];
        let (ptr, len) = i64_ptr(&values);
        unsafe {
            assert_eq!(lkrt_list_i64_contains(ptr, len, 4), 1);
            assert_eq!(lkrt_list_i64_contains(ptr, len, 9), 0);
            assert_eq!(lkrt_list_i64_index_of(ptr, len, 1), 1);
            assert_eq!(lkrt_list_i64_index_of(ptr, len, 9), -1);
        }
    }

    #[test]
    fn i64_reverse_sort_pop() {
        let values = [3i64, 1, 2];
        let (ptr, len) = i64_ptr(&values);
        let mut dst = [0i64; 8];
        let mut dst_len = 0i64;
        unsafe {
            lkrt_list_i64_reverse(ptr, len, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(dst_len, 3);
            assert_eq!(&dst[..3], &[2, 1, 3]);
            lkrt_list_i64_sort(ptr, len, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..3], &[1, 2, 3]);
            assert_eq!(lkrt_list_i64_pop(ptr, len), 2);
            assert_eq!(lkrt_list_i64_pop(std::ptr::null(), 0), 0);
        }
    }

    #[test]
    fn i64_slice_push_insert() {
        let values = [10i64, 20, 30, 40];
        let (ptr, len) = i64_ptr(&values);
        let mut dst = [0i64; 8];
        let mut dst_len = 0i64;
        unsafe {
            lkrt_list_i64_slice_range(ptr, len, 1, 3, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[20, 30]);
            lkrt_list_i64_slice_range(ptr, len, -5, 99, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 30, 40]);
            lkrt_list_i64_push(ptr, len, 50, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 30, 40, 50]);
            lkrt_list_i64_insert(ptr, len, 2, 25, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 25, 30, 40]);
            lkrt_list_i64_insert(ptr, len, 99, 25, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 30, 40, 25]);
        }
    }

    #[test]
    fn i64_remove_and_set() {
        let values = [10i64, 20, 30, 40];
        let (ptr, len) = i64_ptr(&values);
        let mut dst = [0i64; 8];
        let mut dst_len = 0i64;
        unsafe {
            assert_eq!(lkrt_list_i64_remove_at(ptr, len, 1, dst.as_mut_ptr(), &mut dst_len), 20);
            assert_eq!(&dst[..dst_len as usize], &[10, 30, 40]);
            assert_eq!(lkrt_list_i64_remove_at(ptr, len, 99, dst.as_mut_ptr(), &mut dst_len), 0);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 30, 40]);
            assert_eq!(lkrt_list_i64_set(ptr, len, 2, 99, dst.as_mut_ptr(), &mut dst_len), 30);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 99, 40]);
            assert_eq!(lkrt_list_i64_set(ptr, len, 99, 0, dst.as_mut_ptr(), &mut dst_len), 0);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 30, 40]);
        }
    }

    #[test]
    fn i64_slice_take_concat_eq() {
        let values = [10i64, 20, 30, 40];
        let (ptr, len) = i64_ptr(&values);
        let mut dst = [0i64; 16];
        let mut dst_len = 0i64;
        unsafe {
            lkrt_list_i64_slice(ptr, len, 1, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[20, 30, 40]);
            lkrt_list_i64_take(ptr, len, 2, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[10, 20]);
            let rhs = [50i64, 60];
            lkrt_list_i64_concat(ptr, len, rhs.as_ptr(), 2, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[10, 20, 30, 40, 50, 60]);
            assert_eq!(lkrt_list_i64_eq(ptr, len, ptr, len), 1);
            let other = [10i64, 20, 30, 99];
            assert_eq!(lkrt_list_i64_eq(ptr, len, other.as_ptr(), 4), 0);
            assert_eq!(lkrt_list_i64_eq(ptr, len, ptr, 3), 0);
        }
    }

    /// In-place aliasing (src == dst) must behave like the old hand-written IR.
    #[test]
    fn i64_in_place_aliasing() {
        let mut buf = [3i64, 1, 2, 0, 0, 0, 0, 0];
        let mut dst_len = 0i64;
        let ptr = buf.as_mut_ptr();
        unsafe {
            lkrt_list_i64_slice_range(ptr, 3, 1, 3, ptr, &mut dst_len);
            assert_eq!(dst_len, 2);
            assert_eq!(&buf[..2], &[1, 2]);
        }
        let mut buf = [3i64, 1, 2, 0, 0, 0, 0, 0];
        let ptr = buf.as_mut_ptr();
        unsafe {
            lkrt_list_i64_sort(ptr, 3, ptr, &mut dst_len);
            assert_eq!(&buf[..3], &[1, 2, 3]);
            lkrt_list_i64_push(ptr, 3, 9, ptr, &mut dst_len);
            assert_eq!(dst_len, 4);
            assert_eq!(&buf[..4], &[1, 2, 3, 9]);
        }
        let mut buf = [10i64, 20, 30, 0, 0, 0, 0, 0];
        let ptr = buf.as_mut_ptr();
        unsafe {
            lkrt_list_i64_insert(ptr, 3, 1, 15, ptr, &mut dst_len);
            assert_eq!(dst_len, 4);
            assert_eq!(&buf[..4], &[10, 15, 20, 30]);
        }
        let mut buf = [10i64, 20, 30, 40, 0, 0, 0, 0];
        let ptr = buf.as_mut_ptr();
        unsafe {
            assert_eq!(lkrt_list_i64_remove_at(ptr, 4, 1, ptr, &mut dst_len), 20);
            assert_eq!(dst_len, 3);
            assert_eq!(&buf[..3], &[10, 30, 40]);
        }
    }

    #[test]
    fn f64_scalar_queries() {
        let values = [3.0f64, 1.5, 2.0];
        let (ptr, len) = f64_ptr(&values);
        unsafe {
            assert_eq!(lkrt_list_f64_contains(ptr, len, 1.5), 1);
            assert_eq!(lkrt_list_f64_contains(ptr, len, 9.0), 0);
            assert_eq!(lkrt_list_f64_index_of(ptr, len, 2.0), 2);
            assert_eq!(lkrt_list_f64_index_of(ptr, len, 9.0), -1);
            assert_eq!(lkrt_list_f64_pop(ptr, len), 2.0);
        }
    }

    #[test]
    fn f64_producers() {
        let values = [3.0f64, 1.0, 2.0, 2.0];
        let (ptr, len) = f64_ptr(&values);
        let mut dst = [0.0f64; 16];
        let mut dst_len = 0i64;
        unsafe {
            lkrt_list_f64_sort(ptr, len, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[1.0, 2.0, 2.0, 3.0]);
            lkrt_list_f64_reverse(ptr, len, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[2.0, 2.0, 1.0, 3.0]);
            lkrt_list_f64_unique(ptr, len, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[3.0, 1.0, 2.0]);
            lkrt_list_f64_slice(ptr, len, 2, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[2.0, 2.0]);
            lkrt_list_f64_slice_range(ptr, len, 1, 3, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[1.0, 2.0]);
            lkrt_list_f64_take(ptr, len, 2, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[3.0, 1.0]);
            lkrt_list_f64_push(ptr, len, 9.0, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[3.0, 1.0, 2.0, 2.0, 9.0]);
            lkrt_list_f64_insert(ptr, len, 1, 7.0, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[3.0, 7.0, 1.0, 2.0, 2.0]);
            assert_eq!(
                lkrt_list_f64_remove_at(ptr, len, 0, dst.as_mut_ptr(), &mut dst_len),
                3.0
            );
            assert_eq!(&dst[..dst_len as usize], &[1.0, 2.0, 2.0]);
            assert_eq!(lkrt_list_f64_set(ptr, len, 1, 8.0, dst.as_mut_ptr(), &mut dst_len), 1.0);
            assert_eq!(&dst[..dst_len as usize], &[3.0, 8.0, 2.0, 2.0]);
        }
    }

    #[test]
    fn f64_concat_and_in_place() {
        let lhs = [1.0f64, 2.0];
        let rhs = [3.0f64, 4.0];
        let mut dst = [0.0f64; 8];
        let mut dst_len = 0i64;
        unsafe {
            lkrt_list_f64_concat(lhs.as_ptr(), 2, rhs.as_ptr(), 2, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(&dst[..dst_len as usize], &[1.0, 2.0, 3.0, 4.0]);
        }
        // in-place sort/slice aliasing
        let mut buf = [3.0f64, 1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let ptr = buf.as_mut_ptr();
        unsafe {
            lkrt_list_f64_sort(ptr, 3, ptr, &mut dst_len);
            assert_eq!(&buf[..3], &[1.0, 2.0, 3.0]);
            lkrt_list_f64_slice_range(ptr, 3, 1, 3, ptr, &mut dst_len);
            assert_eq!(dst_len, 2);
            assert_eq!(&buf[..2], &[2.0, 3.0]);
        }
    }

    fn read_ptrs(dst: &[*const c_char], len: i64) -> Vec<String> {
        (0..len as usize)
            .map(|i| unsafe { CStr::from_ptr(dst[i]).to_str().unwrap().to_string() })
            .collect()
    }

    #[test]
    fn str_queries_and_producers() {
        use std::ffi::CString;
        let owned: Vec<CString> = ["b", "a", "c"].iter().map(|s| CString::new(*s).unwrap()).collect();
        let ptrs: Vec<*const c_char> = owned.iter().map(|s| s.as_ptr()).collect();
        let src = ptrs.as_ptr();
        let needle_a = CString::new("a").unwrap();
        let needle_z = CString::new("z").unwrap();
        let value = CString::new("x").unwrap();
        let mut dst = [std::ptr::null::<c_char>(); 16];
        let mut dst_len = 0i64;
        unsafe {
            assert_eq!(lkrt_list_str_contains(src, 3, needle_a.as_ptr()), 1);
            assert_eq!(lkrt_list_str_contains(src, 3, needle_z.as_ptr()), 0);
            assert_eq!(lkrt_list_str_index_of(src, 3, needle_a.as_ptr()), 1);
            assert_eq!(lkrt_list_str_index_of(src, 3, needle_z.as_ptr()), -1);
            assert_eq!(lkrt_list_str_text_len(src, 3), 3);
            assert_eq!(CStr::from_ptr(lkrt_list_str_pop(src, 3)).to_str().unwrap(), "c");
            assert_eq!(CStr::from_ptr(lkrt_list_str_pop(src, 0)).to_str().unwrap(), "");

            lkrt_list_str_sort(src, 3, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["a", "b", "c"]);
            lkrt_list_str_reverse(src, 3, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["c", "a", "b"]);
            lkrt_list_str_slice_range(src, 3, 1, 3, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["a", "c"]);
            lkrt_list_str_push(src, 3, value.as_ptr(), dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["b", "a", "c", "x"]);
            lkrt_list_str_insert(src, 3, 1, value.as_ptr(), dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["b", "x", "a", "c"]);

            let removed = lkrt_list_str_remove_at(src, 3, 0, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(CStr::from_ptr(removed).to_str().unwrap(), "b");
            assert_eq!(read_ptrs(&dst, dst_len), ["a", "c"]);
            let removed = lkrt_list_str_remove_at(src, 3, 9, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(CStr::from_ptr(removed).to_str().unwrap(), "");
            assert_eq!(read_ptrs(&dst, dst_len), ["b", "a", "c"]);

            let old = lkrt_list_str_set(src, 3, 1, value.as_ptr(), dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(CStr::from_ptr(old).to_str().unwrap(), "a");
            assert_eq!(read_ptrs(&dst, dst_len), ["b", "x", "c"]);

            lkrt_list_str_slice(src, 3, 1, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["a", "c"]);
            lkrt_list_str_take(src, 3, 2, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["b", "a"]);
            lkrt_list_str_concat(src, 3, src, 3, dst.as_mut_ptr(), &mut dst_len);
            assert_eq!(read_ptrs(&dst, dst_len), ["b", "a", "c", "b", "a", "c"]);
        }
    }

    #[test]
    fn i64_map_int_set_lookup() {
        let mut keys = [0i64; 8];
        let mut vals = [0i64; 8];
        let mut out = 0i64;
        unsafe {
            let mut len = 0i64;
            len = lkrt_map_i64_int_set(keys.as_mut_ptr(), vals.as_mut_ptr(), len, 10, 100);
            assert_eq!(len, 1);
            len = lkrt_map_i64_int_set(keys.as_mut_ptr(), vals.as_mut_ptr(), len, 20, 200);
            assert_eq!(len, 2);
            // updating an existing key keeps the length
            len = lkrt_map_i64_int_set(keys.as_mut_ptr(), vals.as_mut_ptr(), len, 10, 111);
            assert_eq!(len, 2);
            assert_eq!(
                lkrt_map_i64_int_lookup(keys.as_ptr(), vals.as_ptr(), len, 10, &mut out),
                1
            );
            assert_eq!(out, 111);
            assert_eq!(
                lkrt_map_i64_int_lookup(keys.as_ptr(), vals.as_ptr(), len, 20, &mut out),
                1
            );
            assert_eq!(out, 200);
            assert_eq!(
                lkrt_map_i64_int_lookup(keys.as_ptr(), vals.as_ptr(), len, 99, &mut out),
                0
            );
        }
    }

    #[test]
    fn i64_map_f64_and_ptr() {
        use std::ffi::CString;
        let mut keys = [0i64; 8];
        let mut fvals = [0.0f64; 8];
        let mut fout = 0.0f64;
        unsafe {
            let mut len = 0i64;
            len = lkrt_map_i64_f64_set(keys.as_mut_ptr(), fvals.as_mut_ptr(), len, 1, 1.5);
            len = lkrt_map_i64_f64_set(keys.as_mut_ptr(), fvals.as_mut_ptr(), len, 2, 2.5);
            assert_eq!(len, 2);
            assert_eq!(
                lkrt_map_i64_f64_lookup(keys.as_ptr(), fvals.as_ptr(), len, 2, &mut fout),
                1
            );
            assert_eq!(fout, 2.5);
            assert_eq!(
                lkrt_map_i64_f64_lookup(keys.as_ptr(), fvals.as_ptr(), len, 9, &mut fout),
                0
            );
        }

        let hello = CString::new("hello").unwrap();
        let world = CString::new("world").unwrap();
        let mut pkeys = [0i64; 8];
        let mut pvals = [std::ptr::null::<c_char>(); 8];
        let mut pout = std::ptr::null::<c_char>();
        unsafe {
            let mut len = 0i64;
            len = lkrt_map_i64_ptr_set(pkeys.as_mut_ptr(), pvals.as_mut_ptr(), len, 1, hello.as_ptr());
            len = lkrt_map_i64_ptr_set(pkeys.as_mut_ptr(), pvals.as_mut_ptr(), len, 2, world.as_ptr());
            assert_eq!(len, 2);
            assert_eq!(
                lkrt_map_i64_ptr_lookup(pkeys.as_ptr(), pvals.as_ptr(), len, 2, &mut pout),
                1
            );
            assert_eq!(CStr::from_ptr(pout).to_str().unwrap(), "world");
            assert_eq!(
                lkrt_map_i64_ptr_lookup(pkeys.as_ptr(), pvals.as_ptr(), len, 9, &mut pout),
                0
            );
        }
    }

    #[test]
    fn i64_decimal_len_matches_display_width() {
        assert_eq!(lkrt_i64_decimal_len(0), 1);
        assert_eq!(lkrt_i64_decimal_len(7), 1);
        assert_eq!(lkrt_i64_decimal_len(42), 2);
        assert_eq!(lkrt_i64_decimal_len(100), 3);
        assert_eq!(lkrt_i64_decimal_len(-1), 2);
        assert_eq!(lkrt_i64_decimal_len(-1234), 5);
        // The width must equal the actual decimal spelling, including `-`.
        for v in [0i64, 1, 9, 10, -9, -10, 12345, -12345, i64::MAX, i64::MIN] {
            assert_eq!(lkrt_i64_decimal_len(v) as usize, v.to_string().len(), "width for {v}");
        }
    }

    #[test]
    fn str_map_split_key_matches_hand_written_ir() {
        use std::ffi::CString;
        let mut prefix_out = std::ptr::null::<c_char>();
        let read = |p: *const c_char| unsafe { CStr::from_ptr(p).to_str().unwrap().to_string() };
        unsafe {
            // trailing digits split off into prefix + number
            let k = CString::new("n123").unwrap();
            assert_eq!(lkrt_map_str_split_key(k.as_ptr(), &mut prefix_out), 123);
            assert_eq!(read(prefix_out), "n");
            let k = CString::new("item42").unwrap();
            assert_eq!(lkrt_map_str_split_key(k.as_ptr(), &mut prefix_out), 42);
            assert_eq!(read(prefix_out), "item");
            let k = CString::new("a1b2").unwrap();
            assert_eq!(lkrt_map_str_split_key(k.as_ptr(), &mut prefix_out), 2);
            assert_eq!(read(prefix_out), "a1b");
            // "raw" keys: no trailing digit, all digits, or empty -> number 0 and
            // the original pointer is passed straight through.
            for raw in ["abc", "123", ""] {
                let k = CString::new(raw).unwrap();
                assert_eq!(lkrt_map_str_split_key(k.as_ptr(), &mut prefix_out), 0);
                assert_eq!(prefix_out, k.as_ptr());
            }
        }
    }

    /// Round-trips a `DynamicMap<str, i64>` through the composite-key helpers:
    /// keys carry a string prefix + integer suffix, and set/lookup/contains/delete
    /// must agree on that pairing.
    #[test]
    fn str_map_int_set_lookup_contains_delete() {
        use std::ffi::CString;
        let ka = CString::new("k").unwrap();
        let kb = CString::new("k").unwrap(); // same prefix, different number
        let mut prefixes = [std::ptr::null::<c_char>(); 8];
        let mut numbers = [0i64; 8];
        let mut values = [0i64; 8];
        let mut out = 0i64;
        unsafe {
            let mut len = 0i64;
            len = lkrt_map_str_int_set(
                prefixes.as_mut_ptr(),
                numbers.as_mut_ptr(),
                values.as_mut_ptr(),
                len,
                ka.as_ptr(),
                1,
                10,
            );
            len = lkrt_map_str_int_set(
                prefixes.as_mut_ptr(),
                numbers.as_mut_ptr(),
                values.as_mut_ptr(),
                len,
                kb.as_ptr(),
                2,
                20,
            );
            assert_eq!(len, 2);
            // updating (same prefix + number) keeps the length
            len = lkrt_map_str_int_set(
                prefixes.as_mut_ptr(),
                numbers.as_mut_ptr(),
                values.as_mut_ptr(),
                len,
                ka.as_ptr(),
                1,
                11,
            );
            assert_eq!(len, 2);
            assert_eq!(
                lkrt_map_str_int_lookup(
                    prefixes.as_ptr(),
                    numbers.as_ptr(),
                    values.as_ptr(),
                    len,
                    ka.as_ptr(),
                    1,
                    &mut out
                ),
                1
            );
            assert_eq!(out, 11);
            // same prefix but wrong number must miss
            assert_eq!(
                lkrt_map_str_int_lookup(
                    prefixes.as_ptr(),
                    numbers.as_ptr(),
                    values.as_ptr(),
                    len,
                    ka.as_ptr(),
                    9,
                    &mut out
                ),
                0
            );
            assert_eq!(
                lkrt_map_str_contains(prefixes.as_ptr(), numbers.as_ptr(), len, kb.as_ptr(), 2),
                1
            );
            assert_eq!(
                lkrt_map_str_contains(prefixes.as_ptr(), numbers.as_ptr(), len, kb.as_ptr(), 3),
                0
            );

            // delete (k,1) into a fresh destination; removed value + presence reported
            let mut dp = [std::ptr::null::<c_char>(); 8];
            let mut dn = [0i64; 8];
            let mut dv = [0i64; 8];
            let mut removed = 0i64;
            let mut present = 0i64;
            let new_len = lkrt_map_str_int_delete(
                prefixes.as_ptr(),
                numbers.as_ptr(),
                values.as_ptr(),
                len,
                dp.as_mut_ptr(),
                dn.as_mut_ptr(),
                dv.as_mut_ptr(),
                ka.as_ptr(),
                1,
                &mut removed,
                &mut present,
            );
            assert_eq!(new_len, 1);
            assert_eq!(present, 1);
            assert_eq!(removed, 11);
            // the surviving entry is (k,2)=20
            assert_eq!(
                lkrt_map_str_int_lookup(dp.as_ptr(), dn.as_ptr(), dv.as_ptr(), new_len, kb.as_ptr(), 2, &mut out),
                1
            );
            assert_eq!(out, 20);
            assert_eq!(
                lkrt_map_str_int_lookup(dp.as_ptr(), dn.as_ptr(), dv.as_ptr(), new_len, ka.as_ptr(), 1, &mut out),
                0
            );
        }
    }

    /// The `str` value layout only ever moves element pointers around; the removed
    /// pointer is reported as-is.
    #[test]
    fn str_map_ptr_set_lookup_delete() {
        use std::ffi::CString;
        let key = CString::new("name").unwrap();
        let hello = CString::new("hello").unwrap();
        let world = CString::new("world").unwrap();
        let mut prefixes = [std::ptr::null::<c_char>(); 8];
        let mut numbers = [0i64; 8];
        let mut values = [std::ptr::null::<c_char>(); 8];
        let mut out = std::ptr::null::<c_char>();
        unsafe {
            let mut len = 0i64;
            len = lkrt_map_str_ptr_set(
                prefixes.as_mut_ptr(),
                numbers.as_mut_ptr(),
                values.as_mut_ptr(),
                len,
                key.as_ptr(),
                0,
                hello.as_ptr(),
            );
            assert_eq!(len, 1);
            // update in place, same key
            len = lkrt_map_str_ptr_set(
                prefixes.as_mut_ptr(),
                numbers.as_mut_ptr(),
                values.as_mut_ptr(),
                len,
                key.as_ptr(),
                0,
                world.as_ptr(),
            );
            assert_eq!(len, 1);
            assert_eq!(
                lkrt_map_str_ptr_lookup(
                    prefixes.as_ptr(),
                    numbers.as_ptr(),
                    values.as_ptr(),
                    len,
                    key.as_ptr(),
                    0,
                    &mut out
                ),
                1
            );
            assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "world");

            let mut dp = [std::ptr::null::<c_char>(); 8];
            let mut dn = [0i64; 8];
            let mut dv = [std::ptr::null::<c_char>(); 8];
            let mut removed = std::ptr::null::<c_char>();
            let mut present = 0i64;
            let new_len = lkrt_map_str_ptr_delete(
                prefixes.as_ptr(),
                numbers.as_ptr(),
                values.as_ptr(),
                len,
                dp.as_mut_ptr(),
                dn.as_mut_ptr(),
                dv.as_mut_ptr(),
                key.as_ptr(),
                0,
                &mut removed,
                &mut present,
            );
            assert_eq!(new_len, 0);
            assert_eq!(present, 1);
            assert_eq!(CStr::from_ptr(removed).to_str().unwrap(), "world");
        }
    }
}
