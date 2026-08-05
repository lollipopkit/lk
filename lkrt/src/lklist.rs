//! Growable typed list handles for AOT (Phase 2 container handle-ification).
//!
//! Unlike the legacy caller-allocated fixed `[4096 x T]` buffers (see
//! `docs/aot/native-stdlib.md`), a list is an opaque `*mut Vec<T>` handle that
//! grows without bound. Handles live in the runtime's default arena
//! (aot-redesign §3.4): registered on creation and reclaimed by `lkrt_cleanup`,
//! which generated entry code calls on the clean exit path.
//!
//! `get` follows the VM's indexing semantics exactly (see
//! `core/src/vm/exec/container/index.rs`): a negative index counts from the end,
//! and an out-of-range index yields "absent" (`present = 0`) rather than a value —
//! the caller models the result as `Maybe<Int>`.

// `alloc`, not the std prelude: this module is part of the computation-only
// subset that builds without an OS.
#[allow(unused_imports)]
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use alloc::ffi::CString;
use core::ffi::{CStr, c_char, c_void};

/// Length of the `str` list behind a handle; `0` for null.
///
/// Length and element reads are separate calls, and each *copies out*, so a
/// caller cannot hold a reference into the list across a callback into generated
/// code — the whole point of reading this way. Returning a slice would have to
/// invent a lifetime for a borrow of an opaque pointer.
///
/// # Safety
/// `handle` must be a live `str` list handle, or null.
unsafe fn list_str_len(handle: *mut c_void) -> usize {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as the caller's contract states.
    unsafe { &*(handle as *mut Vec<*const c_char>) }.len()
}

/// Element `index` of the `str` list behind a handle, copied out. `None` past the
/// end (which a callback that shrank the list can produce).
///
/// # Safety
/// As [`list_str_len`].
unsafe fn list_str_at(handle: *mut c_void, index: usize) -> Option<*const c_char> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: as the caller's contract states.
    unsafe { &*(handle as *mut Vec<*const c_char>) }.get(index).copied()
}

/// As [`list_str_len`], for an `i64` list.
///
/// # Safety
/// `handle` must be a live `i64` list handle, or null.
unsafe fn list_i64_len(handle: *mut c_void) -> usize {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as the caller's contract states.
    unsafe { &*(handle as *mut Vec<i64>) }.len()
}

/// As [`list_str_at`], for an `i64` list.
///
/// # Safety
/// As [`list_i64_len`].
unsafe fn list_i64_at(handle: *mut c_void, index: usize) -> Option<i64> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: as the caller's contract states.
    unsafe { &*(handle as *mut Vec<i64>) }.get(index).copied()
}

/// Creates a fresh, empty `i64` list handle.
/// Materializes an integer range (`a..b` / `a..=b`, optional step) as a
/// `List<i64>` — the VM's `build_int_range` semantics exactly: zero step and
/// stepping overflow are loud failures.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lklist_i64_from_range(start: i64, end: i64, step: i64, inclusive: i64) -> *mut c_void {
    if step == 0 {
        crate::panic::raise_str("runtime error");
    }
    let mut out = Vec::new();
    let mut current = start;
    if step > 0 {
        while if inclusive != 0 { current <= end } else { current < end } {
            out.push(current);
            current = match current.checked_add(step) {
                Some(v) => v,
                None => crate::panic::raise_str("runtime error"),
            };
        }
    } else {
        while if inclusive != 0 { current >= end } else { current > end } {
            out.push(current);
            current = match current.checked_add(step) {
                Some(v) => v,
                None => crate::panic::raise_str("runtime error"),
            };
        }
    }
    crate::state::arena_handle(out)
}

/// `xs.take(n)` / `xs.skip(n)` — a fresh prefix / suffix. A negative count
/// raises, as in the VM: a count has no negative meaning, and the cast this
/// used to perform (`-1 as usize`) took the whole list instead.
///
/// One macro over both directions and every carrier. Neither operation looks at
/// the element, and the four hand-written copies (`i64` and boxed, take and
/// skip) spelled that raise message four times while `f64` and `str` had no copy
/// at all — so `[1.5, 2.5].take(1)` dropped its module to the VM.
macro_rules! list_window {
    ($name:ident, $elem:ty, $method:literal, $window:expr, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, n: i64) -> *mut c_void {
            if n < 0 {
                crate::panic::raise_str(&format!(
                    concat!("list.", $method, "() count must be non-negative, got {}"),
                    n
                ));
            }
            let values: &[$elem] = if handle.is_null() {
                &[]
            } else {
                // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
                // constructor.
                unsafe { &*(handle as *mut Vec<$elem>) }
            };
            // Clamped once, so both directions see an in-range cut.
            let cut = (n as usize).min(values.len());
            let window: fn(&[$elem], usize) -> &[$elem] = $window;
            crate::state::arena_handle(window(values, cut).to_vec())
        }
    };
}

list_window!(
    lkrt_lklist_i64_take,
    i64,
    "take",
    |v, cut| &v[..cut],
    "`take(n)` on a `List<i64>`."
);
list_window!(
    lkrt_lklist_f64_take,
    f64,
    "take",
    |v, cut| &v[..cut],
    "`take(n)` on a `List<f64>`."
);
list_window!(
    lkrt_lklist_str_take,
    *const c_char,
    "take",
    |v, cut| &v[..cut],
    "`take(n)` on a `List<str>`."
);
list_window!(
    lkrt_lklist_dyn_take,
    crate::lkdyn::LkDyn,
    "take",
    |v, cut| &v[..cut],
    "`take(n)` on a boxed-element list."
);
list_window!(
    lkrt_lklist_i64_skip,
    i64,
    "skip",
    |v, cut| &v[cut..],
    "`skip(n)` on a `List<i64>`."
);
list_window!(
    lkrt_lklist_f64_skip,
    f64,
    "skip",
    |v, cut| &v[cut..],
    "`skip(n)` on a `List<f64>`."
);
list_window!(
    lkrt_lklist_str_skip,
    *const c_char,
    "skip",
    |v, cut| &v[cut..],
    "`skip(n)` on a `List<str>`."
);
list_window!(
    lkrt_lklist_dyn_skip,
    crate::lkdyn::LkDyn,
    "skip",
    |v, cut| &v[cut..],
    "`skip(n)` on a boxed-element list."
);

/// The write position a list *method* names, in the VM's exact wording.
///
/// Deliberately not [`store_index_or_raise`]: that is the index-*assignment*
/// path (`xs[i] = v`, "list index N out of bounds"), and the method path words
/// the same range failure differently — `list.insert() index N out of bounds
/// (len=N)`. Which index appears also differs between the two messages: the
/// before-the-start one names the index *as written* (so a reader sees the `-9`
/// they typed), the out-of-bounds one names the *resolved* position. A caught
/// error is printed output, so each wording is part of an answer.
///
/// `allow_len` is the caller's upper bound: `insert` accepts `len`, because that
/// is where an append goes; `remove_at` does not. A null handle is a list of
/// zero, which makes every index a range failure without a special case.
fn method_index_or_raise(method: &str, index: i64, len: usize, allow_len: bool) -> usize {
    let resolved = if index < 0 { len as i64 + index } else { index };
    if resolved < 0 {
        crate::panic::raise_str(&alloc::format!(
            "list.{method}() index {index} is before the start of a list of {len}"
        ));
    }
    let resolved = resolved as usize;
    if if allow_len { resolved > len } else { resolved >= len } {
        crate::panic::raise_str(&alloc::format!(
            "list.{method}() index {resolved} out of bounds (len={len})"
        ));
    }
    resolved
}

/// `xs.pop()`'s mutation half: drops the last element, answering nothing.
///
/// `pop` is a read *and* a drop, and the read already exists — `xs.last()`
/// lowers to the carrier's `Maybe` machinery, which is verified and, for `f64`,
/// the only portable shape available: a by-value `{double, i64}` return is a
/// mixed-class aggregate whose registers differ across targets, so Cranelift's
/// scalar signatures cannot model it (hence lkrt's `_get_out` shims). A
/// `*_pop -> LkMaybeF64` would have needed a fifth mechanism for one carrier.
/// So the lowering reads the last element the way `last()` does and then calls
/// this, and the empty case needs no special agreement: reading past the end is
/// already nil, and dropping from empty is already nothing.
macro_rules! list_drop_last {
    ($name:ident, $elem:ty, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void) {
            if handle.is_null() {
                return;
            }
            // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
            // constructor.
            unsafe { (*(handle as *mut Vec<$elem>)).pop() };
        }
    };
}

list_drop_last!(lkrt_lklist_i64_drop_last, i64, "`pop()`'s drop half on a `List<i64>`.");
list_drop_last!(lkrt_lklist_f64_drop_last, f64, "`pop()`'s drop half on a `List<f64>`.");
list_drop_last!(
    lkrt_lklist_str_drop_last,
    *const c_char,
    "`pop()`'s drop half on a `List<str>`. The element pointer is arena-owned, so \
     the value the lowering already read stays valid."
);
list_drop_last!(
    lkrt_lklist_dyn_drop_last,
    crate::lkdyn::LkDyn,
    "`pop()`'s drop half on a boxed-element list."
);

/// `xs.insert(i, v)` — in place, like `push` and `set`. Answers nothing: the VM
/// evaluates it to the list itself, which the lowering supplies from the
/// receiver it already holds (see `list_clear!` for why returning the handle
/// would be wrong).
macro_rules! list_insert {
    ($name:ident, $elem:ty, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, index: i64, value: $elem) {
            if handle.is_null() {
                // Still range-checked, so a bad index raises the same message it
                // would for a real empty list.
                method_index_or_raise("insert", index, 0, true);
                return;
            }
            // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
            // constructor.
            let values = unsafe { &mut *(handle as *mut Vec<$elem>) };
            let at = method_index_or_raise("insert", index, values.len(), true);
            values.insert(at, value);
        }
    };
}

list_insert!(lkrt_lklist_i64_insert, i64, "`insert(i, v)` on a `List<i64>`.");
list_insert!(lkrt_lklist_f64_insert, f64, "`insert(i, v)` on a `List<f64>`.");
list_insert!(
    lkrt_lklist_str_insert,
    *const c_char,
    "`insert(i, v)` on a `List<str>`."
);
list_insert!(
    lkrt_lklist_dyn_insert,
    crate::lkdyn::LkDyn,
    "`insert(i, v)` on a boxed-element list."
);

/// `xs.remove_at(i)` — removes the element at `i` and answers it. Unlike `pop`
/// the answer is never nil: an out-of-range index raises first, so there is
/// always an element to hand back.
macro_rules! list_remove_at {
    ($name:ident, $elem:ty, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, index: i64) -> $elem {
            if handle.is_null() {
                // A list of zero: every index is out of range, and this raises
                // rather than returning.
                method_index_or_raise("remove_at", index, 0, false);
            }
            // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
            // constructor.
            let values = unsafe { &mut *(handle as *mut Vec<$elem>) };
            let at = method_index_or_raise("remove_at", index, values.len(), false);
            values.remove(at)
        }
    };
}

list_remove_at!(lkrt_lklist_i64_remove_at, i64, "`remove_at(i)` on a `List<i64>`.");
list_remove_at!(lkrt_lklist_f64_remove_at, f64, "`remove_at(i)` on a `List<f64>`.");
list_remove_at!(
    lkrt_lklist_str_remove_at,
    *const c_char,
    "`remove_at(i)` on a `List<str>`."
);
list_remove_at!(
    lkrt_lklist_dyn_remove_at,
    crate::lkdyn::LkDyn,
    "`remove_at(i)` on a boxed-element list."
);

/// `words.map(f)` over a `str` list (`fn(*const c_char) -> *const c_char`
/// callback returning an arena-owned string).
///
/// # Safety
/// `handle` must be a live `List<str>` handle (or null); `f` a compiled lambda.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_map_fn(
    handle: *mut c_void,
    f: extern "C" fn(*const c_char) -> *const c_char,
) -> *mut c_void {
    // Snapshotted before the callback runs. `f`/`p` re-enters generated code,
    // which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past the borrow — either way a slice held across the call is
    // unsound. CLAUDE.md's lkrt rule ("never call a raise-capable function while
    // holding a lock guard or RefCell borrow") is the same rule; a slice borrow
    // is just a third way to hold one.
    // Indexed, and the handle is re-dereferenced each step: `f`/`p` re-enters
    // generated code, which can push to *this* list (reallocating its buffer) or
    // raise and longjmp past a borrow — so no slice may be held across the call.
    // CLAUDE.md's lkrt rule ("never call a raise-capable function while holding a
    // lock guard or RefCell borrow") is the same rule; a slice borrow is a third
    // way to hold one. Re-deref rather than a `to_vec()` snapshot: this is the
    // native HOF hot path the perf gate measures, and copying the whole input on
    // top of the result allocation is not free.
    // SAFETY: the handle is live per this function's contract.
    let len = unsafe { list_str_len(handle) };
    let mut mapped: Vec<*const c_char> = Vec::with_capacity(len);
    for index in 0..len {
        // SAFETY: the handle is live per this function's contract.
        let Some(value) = (unsafe { list_str_at(handle, index) }) else {
            break;
        };
        mapped.push(f(value));
    }
    crate::state::arena_handle(mapped)
}

/// `words.filter(p)` over a `str` list (`fn(*const c_char) -> bool`).
///
/// # Safety
/// `handle` must be a live `List<str>` handle (or null); `p` a compiled lambda.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_filter_fn(
    handle: *mut c_void,
    p: extern "C" fn(*const c_char) -> bool,
) -> *mut c_void {
    // Snapshotted before the callback runs. `f`/`p` re-enters generated code,
    // which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past the borrow — either way a slice held across the call is
    // unsound. CLAUDE.md's lkrt rule ("never call a raise-capable function while
    // holding a lock guard or RefCell borrow") is the same rule; a slice borrow
    // is just a third way to hold one.
    // Indexed, and the handle is re-dereferenced each step: `f`/`p` re-enters
    // generated code, which can push to *this* list (reallocating its buffer) or
    // raise and longjmp past a borrow — so no slice may be held across the call.
    // CLAUDE.md's lkrt rule ("never call a raise-capable function while holding a
    // lock guard or RefCell borrow") is the same rule; a slice borrow is a third
    // way to hold one. Re-deref rather than a `to_vec()` snapshot: this is the
    // native HOF hot path the perf gate measures, and copying the whole input on
    // top of the result allocation is not free.
    // SAFETY: the handle is live per this function's contract.
    let len = unsafe { list_str_len(handle) };
    let mut kept: Vec<*const c_char> = Vec::new();
    for index in 0..len {
        // SAFETY: the handle is live per this function's contract.
        let Some(value) = (unsafe { list_str_at(handle, index) }) else {
            break;
        };
        if p(value) {
            kept.push(value);
        }
    }
    crate::state::arena_handle(kept)
}

/// `xs.unique()` over an `i64` list — first-occurrence order (integer
/// equality equals the VM's `to_bits` rule for Int).
///
/// # Safety
/// `handle` must be a live `List<i64>` handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_unique(handle: *mut c_void) -> *mut c_void {
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    let mut seen = crate::lkmap::FxSet::default();
    let mut out = Vec::new();
    for &v in values {
        if seen.insert(v) {
            out.push(v);
        }
    }
    crate::state::arena_handle(out)
}

/// `xs.chain(ys)` — a fresh concatenation of two `List<i64>`.
///
/// # Safety
/// Both handles must be live `List<i64>` handles, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_chain(a: *mut c_void, b: *mut c_void) -> *mut c_void {
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
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    out.extend_from_slice(lhs);
    out.extend_from_slice(rhs);
    crate::state::arena_handle(out)
}

/// `xs.chain(ys)` / `xs + ys` — a fresh concatenation of two `List<f64>`.
///
/// # Safety
/// Both handles must be live `List<f64>` handles, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_chain(a: *mut c_void, b: *mut c_void) -> *mut c_void {
    let lhs: &[f64] = if a.is_null() {
        &[]
    } else {
        // SAFETY: caller passes live `List<f64>` handles.
        unsafe { &*(a as *mut Vec<f64>) }
    };
    let rhs: &[f64] = if b.is_null() {
        &[]
    } else {
        // SAFETY: as above.
        unsafe { &*(b as *mut Vec<f64>) }
    };
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    out.extend_from_slice(lhs);
    out.extend_from_slice(rhs);
    crate::state::arena_handle(out)
}

/// `xs.chain(ys)` / `xs + ys` — a fresh concatenation of two `List<str>`
/// (the element pointers are arena-owned and shared, never copied).
///
/// # Safety
/// Both handles must be live `List<str>` handles, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_chain(a: *mut c_void, b: *mut c_void) -> *mut c_void {
    let lhs: &[*const c_char] = if a.is_null() {
        &[]
    } else {
        // SAFETY: caller passes live `List<str>` handles.
        unsafe { &*(a as *mut Vec<*const c_char>) }
    };
    let rhs: &[*const c_char] = if b.is_null() {
        &[]
    } else {
        // SAFETY: as above.
        unsafe { &*(b as *mut Vec<*const c_char>) }
    };
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    out.extend_from_slice(lhs);
    out.extend_from_slice(rhs);
    crate::state::arena_handle(out)
}

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
    // Snapshotted before the callback runs. `f`/`p` re-enters generated code,
    // which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past the borrow — either way a slice held across the call is
    // unsound. CLAUDE.md's lkrt rule ("never call a raise-capable function while
    // holding a lock guard or RefCell borrow") is the same rule; a slice borrow
    // is just a third way to hold one.
    // Indexed, and the handle is re-dereferenced each step: `f`/`p` re-enters
    // generated code, which can push to *this* list (reallocating its buffer) or
    // raise and longjmp past a borrow — so no slice may be held across the call.
    // CLAUDE.md's lkrt rule ("never call a raise-capable function while holding a
    // lock guard or RefCell borrow") is the same rule; a slice borrow is a third
    // way to hold one. Re-deref rather than a `to_vec()` snapshot: this is the
    // native HOF hot path the perf gate measures, and copying the whole input on
    // top of the result allocation is not free.
    // SAFETY: the handle is live per this function's contract.
    let len = unsafe { list_i64_len(handle) };
    let mut mapped: Vec<i64> = Vec::with_capacity(len);
    for index in 0..len {
        // SAFETY: the handle is live per this function's contract.
        let Some(value) = (unsafe { list_i64_at(handle, index) }) else {
            break;
        };
        mapped.push(f(value));
    }
    crate::state::arena_handle(mapped)
}

/// `xs.filter(p)` over an `i64` list: keeps the elements whose predicate holds.
///
/// # Safety
/// See [`lkrt_lklist_i64_map_fn`]; `p` returns the lambda's `Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_filter_fn(handle: *mut c_void, p: extern "C" fn(i64) -> bool) -> *mut c_void {
    // Snapshotted before the callback runs. `f`/`p` re-enters generated code,
    // which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past the borrow — either way a slice held across the call is
    // unsound. CLAUDE.md's lkrt rule ("never call a raise-capable function while
    // holding a lock guard or RefCell borrow") is the same rule; a slice borrow
    // is just a third way to hold one.
    // Indexed, and the handle is re-dereferenced each step: `f`/`p` re-enters
    // generated code, which can push to *this* list (reallocating its buffer) or
    // raise and longjmp past a borrow — so no slice may be held across the call.
    // CLAUDE.md's lkrt rule ("never call a raise-capable function while holding a
    // lock guard or RefCell borrow") is the same rule; a slice borrow is a third
    // way to hold one. Re-deref rather than a `to_vec()` snapshot: this is the
    // native HOF hot path the perf gate measures, and copying the whole input on
    // top of the result allocation is not free.
    // SAFETY: the handle is live per this function's contract.
    let len = unsafe { list_i64_len(handle) };
    let mut kept: Vec<i64> = Vec::new();
    for index in 0..len {
        // SAFETY: the handle is live per this function's contract.
        let Some(value) = (unsafe { list_i64_at(handle, index) }) else {
            break;
        };
        if p(value) {
            kept.push(value);
        }
    }
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
        crate::panic::raise_str("runtime error");
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
        crate::panic::raise_str("runtime error");
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
        crate::panic::raise_str("runtime error");
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
    // Every element was minted right here and is reachable from nowhere else,
    // so a proven-dead list can take them with it (`handle_release_deep`).
    unsafe fn owned(ptr: *mut c_void) -> Vec<*mut c_char> {
        // SAFETY: registered with this exact element type below.
        let parts = unsafe { &*(ptr as *const Vec<*const c_char>) };
        parts.iter().map(|part| *part as *mut c_char).collect()
    }
    crate::state::arena_handle_owning_strings(parts, owned)
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
    // Snapshotted before the callback runs. `f`/`p` re-enters generated code,
    // which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past the borrow — either way a slice held across the call is
    // unsound. CLAUDE.md's lkrt rule ("never call a raise-capable function while
    // holding a lock guard or RefCell borrow") is the same rule; a slice borrow
    // is just a third way to hold one.
    // Indexed, and the handle is re-dereferenced each step: `f`/`p` re-enters
    // generated code, which can push to *this* list (reallocating its buffer) or
    // raise and longjmp past a borrow — so no slice may be held across the call.
    // CLAUDE.md's lkrt rule ("never call a raise-capable function while holding a
    // lock guard or RefCell borrow") is the same rule; a slice borrow is a third
    // way to hold one. Re-deref rather than a `to_vec()` snapshot: this is the
    // native HOF hot path the perf gate measures, and copying the whole input on
    // top of the result allocation is not free.
    // SAFETY: the handle is live per this function's contract.
    let len = unsafe { list_i64_len(handle) };
    let mut acc = init;
    for index in 0..len {
        // SAFETY: the handle is live per this function's contract.
        let Some(value) = (unsafe { list_i64_at(handle, index) }) else {
            break;
        };
        acc = f(acc, value);
    }
    acc
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
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(out).unwrap_or_default())
}

/// `xs.clear()` — empties the list in place, and answers nothing.
///
/// The VM's `clear` evaluates to the list, but the *helper* does not hand it
/// back: a pointer-returning ABI entry has to be `Constructs` (a fresh handle
/// the scope-drop pass may release) or `Retained`, and this is neither — it
/// would be the caller's own list, which that pass would then free. The lowering
/// already holds the receiver and uses it as the expression's value, so there is
/// nothing to return. `pointer_returning_entries_are_not_marked_borrowed` is
/// the test that says so.
///
/// One macro over every carrier rather than one function per element type: the
/// operation does not depend on the element at all, and writing it four times is
/// how three of the four end up missing. (`pop` / `insert` / `remove_at` do
/// depend on the element — they are the next piece of work, tracked separately.)
macro_rules! list_clear {
    ($name:ident, $elem:ty, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void) {
            if handle.is_null() {
                return;
            }
            // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
            // constructor.
            unsafe { (*(handle as *mut Vec<$elem>)).clear() };
        }
    };
}

list_clear!(lkrt_lklist_i64_clear, i64, "`clear()` on a `List<i64>`.");
list_clear!(lkrt_lklist_f64_clear, f64, "`clear()` on a `List<f64>`.");
list_clear!(
    lkrt_lklist_str_clear,
    *const c_char,
    "`clear()` on a `List<str>`. The element pointers are arena-owned, so \
     dropping them is not a leak this crate can do anything about (see the \
     module header's ownership note)."
);
list_clear!(
    lkrt_lklist_dyn_clear,
    crate::lkdyn::LkDyn,
    "`clear()` on a boxed-element list. This was a hand-written copy in \
     `lkdyn.rs` — so the macro above claimed to cover every carrier while \
     covering three, which is the shape it was written to prevent."
);

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

/// A store index resolved against `len`: a negative one counts from the end,
/// exactly as the read does. `None` means it is out of range even after that,
/// which is a *halt* for a store — unlike a read, which answers nil.
fn store_index(index: i64, len: usize) -> Option<usize> {
    let len = len as i64;
    let resolved = if index < 0 { len + index } else { index };
    (resolved >= 0 && resolved < len).then_some(resolved as usize)
}

/// The same, raising in the VM's exact wording when it is out of range.
///
/// One message, because the VM has one: out of range at either end is
/// `list index N out of bounds`. A caught error is printed output, so the text
/// is part of the answer and has to match the VM's to the character.
///
/// It used to be two, the negative end saying `list index must be
/// non-negative` — a rule the language does not have, `xs[-1]` being the last
/// element. The two builds agreed only by being wrong the same way.
///
/// `N` is the index **as written**, at both ends. The VM briefly reported the
/// resolved one for a negative index — `-6` for `xs.set(-9, v)` on a
/// three-element list, a number the program never wrote — because it resolved
/// when the key was built and raised several steps later. It now raises at the
/// resolution point, where the original is still in hand, so this side does not
/// have to mirror a worse message to agree.
pub(crate) fn store_index_or_raise(index: i64, len: usize) -> usize {
    match store_index(index, len) {
        Some(resolved) => resolved,
        None => crate::panic::raise_str(&alloc::format!("list index {index} out of bounds")),
    }
}

/// Stores `value` at `index`. Unlike indexing (`get`), the VM treats an
/// out-of-range store index as a fatal error (`list index N out of bounds`),
/// not a nil/grow — so this raises, matching the VM's *halt* (a loud failure,
/// never a silent wrong write). A negative index counts from the end, as
/// `xs[-1] = v` does in the VM.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_set(handle: *mut c_void, index: i64, value: i64) {
    if handle.is_null() {
        crate::panic::raise_str("runtime error");
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    let values = unsafe { &mut *(handle as *mut Vec<i64>) };
    let index = store_index_or_raise(index, values.len());
    values[index] = value;
}

/// Stores `value` at `index` in a `str` list; the same index rule as
/// [`lkrt_lklist_i64_set`].
///
/// The carrier had `at` but no `set`, so `xs[i] = s` and `xs.set(i, s)` on a
/// string list dropped the whole module to the VM while the same two lines on an
/// `Int` list stayed native — a difference in the list's internal representation
/// deciding the fate of a program that cannot see it.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_str_new`], or null;
/// `value` a valid string-constant pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_set(handle: *mut c_void, index: i64, value: *const c_char) {
    if handle.is_null() {
        crate::panic::raise_str("runtime error");
    }
    // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
    let values = unsafe { &mut *(handle as *mut Vec<*const c_char>) };
    let index = store_index_or_raise(index, values.len());
    values[index] = value;
}

/// Stores `value` at `index` in an `f64` list; aborts on an invalid index (see
/// [`lkrt_lklist_i64_set`]).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_set(handle: *mut c_void, index: i64, value: f64) {
    if handle.is_null() {
        crate::panic::raise_str("runtime error");
    }
    // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
    let values = unsafe { &mut *(handle as *mut Vec<f64>) };
    let index = store_index_or_raise(index, values.len());
    values[index] = value;
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
            value: core::ptr::null(),
            present: 0,
        };
    }
    // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
    let values = unsafe { &*(handle as *mut Vec<*const c_char>) };
    let idx = if index < 0 { values.len() as i64 + index } else { index };
    if idx < 0 || idx as usize >= values.len() {
        LkMaybeStr {
            value: core::ptr::null(),
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
        crate::panic::raise_str("runtime error");
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

/// Out-pointer form of [`lkrt_lklist_f64_get_pair`] for the Cranelift backend:
/// a `{double, i64}` returned *by value* is a mixed-class 16-byte aggregate
/// whose registers differ across targets (x86-64 `xmm0:rax`, AArch64 `x0:x1`),
/// which Cranelift's scalar-only signatures cannot model portably. Writing the
/// two components through pointers sidesteps the struct-return ABI entirely.
///
/// # Safety
/// `handle` as in [`lkrt_lklist_f64_get_pair`]; `out_value`/`out_present` must be
/// valid, aligned, writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_get_out(
    handle: *mut c_void,
    index: i64,
    out_value: *mut f64,
    out_present: *mut i64,
) {
    let m = unsafe { lkrt_lklist_f64_get_pair(handle, index) };
    unsafe {
        *out_value = m.value;
        *out_present = m.present;
    }
}

/// Unwraps a `Maybe<f64>` in a scalar context, aborting if absent (see
/// [`lkrt_maybe_i64_unwrap`]).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_maybe_f64_unwrap(value: f64, present: i64) -> f64 {
    if present == 0 {
        crate::panic::raise_str("runtime error");
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
        crate::panic::raise_str("runtime error");
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

/// `x in xs` where the list holds `i64` and the needle is an `f64`.
///
/// Numeric comparison, the same rule `==` uses: the element is widened, not
/// the needle narrowed, so `1 in [1.0]` and `1.0 in [1, 2]` answer the same
/// way `1 == 1.0` does. The VM spells it `*value as f64 == *needle`; this is
/// that expression.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_i64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_contains_f64(handle: *mut c_void, needle: f64) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: `handle` addresses a `Vec<i64>` from `lkrt_lklist_i64_new`.
    let values = unsafe { &*(handle as *mut Vec<i64>) };
    i64::from(values.iter().any(|value| *value as f64 == needle))
}

/// `x in xs` where the list holds `f64` and the needle is an `i64` (see
/// [`lkrt_lklist_i64_contains_f64`]).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_f64_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_contains_i64(handle: *mut c_void, needle: i64) -> i64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: `handle` addresses a `Vec<f64>` from `lkrt_lklist_f64_new`.
    let values = unsafe { &*(handle as *mut Vec<f64>) };
    i64::from(values.contains(&(needle as f64)))
}

/// Linear membership test for a string list — by *content*, matching the
/// VM's `TypedList::String` contains (which stringifies and compares text,
/// for short and long strings alike).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_str_new`], or null;
/// `needle` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_contains(handle: *mut c_void, needle: *const c_char) -> i64 {
    if handle.is_null() || needle.is_null() {
        return 0;
    }
    let needle = unsafe { CStr::from_ptr(needle) };
    // SAFETY: `handle` addresses a `Vec<*const c_char>` from `lkrt_lklist_str_new`.
    let values = unsafe { &*(handle as *mut Vec<*const c_char>) };
    i64::from(
        values
            .iter()
            .any(|&p| !p.is_null() && unsafe { CStr::from_ptr(p) } == needle),
    )
}

/// The half-open range `[start, end)` a two-argument `slice` names, resolved
/// against a list of `len` elements.
///
/// One function because it is one rule (see the negative-position rule the VM
/// and this crate share): negative counts from the tail, everything clamps, and
/// an inverted range is empty rather than a panic. Writing it out per carrier is
/// how four implementations of one rule start.
pub(crate) fn slice_bounds(len: usize, start: i64, end: i64) -> (usize, usize) {
    let signed_len = len as i64;
    let start = if start < 0 { (signed_len + start).max(0) } else { start } as usize;
    let end = (if end < 0 { (signed_len + end).max(0) } else { end } as usize).min(len);
    (start.min(end), end)
}

/// Range slice of a list carrier (`xs[1..5]` / `xs.slice(1, 5)`), exactly the
/// VM's: negative indices count from the tail, everything clamps.
macro_rules! list_slice {
    ($name:ident, $elem:ty, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, start: i64, end: i64) -> *mut c_void {
            let values: &[$elem] = if handle.is_null() {
                &[]
            } else {
                // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
                // constructor.
                unsafe { &*(handle as *mut Vec<$elem>) }
            };
            let (start, end) = slice_bounds(values.len(), start, end);
            crate::state::arena_handle(values[start..end].to_vec())
        }
    };
}

list_slice!(lkrt_lklist_i64_slice, i64, "`i64` list range slice.");
list_slice!(lkrt_lklist_f64_slice, f64, "`f64` list range slice.");
list_slice!(
    lkrt_lklist_str_slice,
    *const c_char,
    "`str` list range slice (elements are interned string-constant pointers)."
);

/// `xs.sort()` — a fresh ascending copy (the VM sorts a snapshot, the receiver
/// is untouched).
///
/// Unlike `reverse`, this *is* about the element, and each carrier's order has
/// to be the one `typed_list_sorted` uses — not merely "ascending":
///
/// * `i64`: `sort_unstable`, which is what the VM calls. `compare_runtime_values`
///   on two `Int`s *is* `i64`'s `Ord`, and equal integers are indistinguishable,
///   so the algorithm cannot show.
/// * `f64`: [`compare_floats`], which is a *total* order. The obvious mirror of
///   the VM — `partial_cmp().unwrap_or(Equal)` — is not one, and Rust's `sort_by`
///   detects that and panics; writing this arm is what found it, on both
///   backends. See [`compare_floats`].
/// * `str`: `sort_by` on the bytes, which is what `Arc<str>`'s `Ord` does in the
///   VM. LK strings hold no interior NUL, so the C representation compares the
///   same bytes.
///
/// The boxed carrier is deliberately absent: its order is
/// `compare_runtime_values` across *kinds*, which needs two rank tables, a
/// depth-limited recursive list comparison, and the slice view — a mirror of
/// that size wants its own conformance test (see `vm_mirror`), not a copy.
/// The VM's `val::compare_floats`, mirrored: a *total* ascending order over
/// floats.
///
/// `partial_cmp(..).unwrap_or(Equal)` is not one — a NaN reads equal to every
/// value while those values stay ordered — and Rust's `sort_by` detects that and
/// panics ("user-provided comparison function does not correctly implement a
/// total order"). In lkrt a panic is an abort, so `[NaN, 5.0, 1.0, …].sort()`
/// killed the process; in the VM it killed the interpreter. Both sides now order
/// NaN instead: all NaNs equal, every NaN greater than every number, `-0.0` and
/// `0.0` still equal (which is what `==` says).
fn compare_floats(left: f64, right: f64) -> core::cmp::Ordering {
    match left.partial_cmp(&right) {
        Some(ordering) => ordering,
        None => match (left.is_nan(), right.is_nan()) {
            (true, true) => core::cmp::Ordering::Equal,
            (true, false) => core::cmp::Ordering::Greater,
            (false, true) => core::cmp::Ordering::Less,
            (false, false) => core::cmp::Ordering::Equal,
        },
    }
}

macro_rules! list_sort {
    ($name:ident, $elem:ty, $sort:expr, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void) -> *mut c_void {
            let mut values: Vec<$elem> = if handle.is_null() {
                Vec::new()
            } else {
                // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
                // constructor.
                unsafe { (*(handle as *mut Vec<$elem>)).clone() }
            };
            let sort: fn(&mut Vec<$elem>) = $sort;
            sort(&mut values);
            crate::state::arena_handle(values)
        }
    };
}

/// `sum()` / `min()` / `max()` on a typed list.
///
/// The empty answers are the VM's: `sum` is `0` (the identity a fold would
/// start from) and `min`/`max` are nil — so those two box their result, the way
/// `first`/`last` already do.
///
/// The orders are the same ones `list_sort!` uses on each carrier, which is
/// what keeps `xs.sort().first()` and `xs.min()` from disagreeing here as well.
///
/// # Safety
/// `handle` must be a live list handle of the carrier named by the entry point.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_sum(handle: *mut c_void) -> i64 {
    // SAFETY: a live `List<i64>` handle, as the ABI declares.
    let values: &Vec<i64> = unsafe { &*(handle as *mut Vec<i64>) };
    // Wrapping, because `+` wraps: one rule for adding integers.
    values.iter().fold(0i64, |total, value| total.wrapping_add(*value))
}

/// # Safety
/// `handle` must be a live `List<f64>` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_sum(handle: *mut c_void) -> f64 {
    // SAFETY: as above.
    let values: &Vec<f64> = unsafe { &*(handle as *mut Vec<f64>) };
    values.iter().sum()
}

macro_rules! list_extreme {
    ($name:ident, $elem:ty, $box_value:expr, $order:expr, $want_max:expr, $doc:literal) => {
        #[doc = $doc]
        ///
        /// # Safety
        /// `handle` must be a live list handle of this carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void) -> crate::lkdyn::LkDyn {
            // SAFETY: a live list handle of this carrier, as the ABI declares.
            let values: &Vec<$elem> = unsafe { &*(handle as *mut Vec<$elem>) };
            let mut best: Option<&$elem> = None;
            for value in values.iter() {
                best = Some(match best {
                    None => value,
                    // Ties keep the earlier element, as the VM's does: `min`
                    // names a *value*, and the first element that has it is the
                    // one a reader would point at.
                    Some(current) => {
                        #[allow(clippy::redundant_closure_call)]
                        let ordering = ($order)(current, value);
                        let keep = match ordering {
                            core::cmp::Ordering::Less => !$want_max,
                            core::cmp::Ordering::Equal => true,
                            core::cmp::Ordering::Greater => $want_max,
                        };
                        if keep { current } else { value }
                    }
                });
            }
            match best {
                #[allow(clippy::redundant_closure_call)]
                Some(value) => ($box_value)(value),
                None => crate::lkdyn::lkrt_dyn_from_nil(),
            }
        }
    };
}

list_extreme!(
    lkrt_lklist_i64_min,
    i64,
    |value: &i64| crate::lkdyn::lkrt_dyn_from_i64(*value),
    |a: &i64, b: &i64| a.cmp(b),
    false,
    "`min()` on a `List<i64>`."
);
list_extreme!(
    lkrt_lklist_i64_max,
    i64,
    |value: &i64| crate::lkdyn::lkrt_dyn_from_i64(*value),
    |a: &i64, b: &i64| a.cmp(b),
    true,
    "`max()` on a `List<i64>`."
);
list_extreme!(
    lkrt_lklist_f64_min,
    f64,
    |value: &f64| crate::lkdyn::lkrt_dyn_from_f64(*value),
    |a: &f64, b: &f64| compare_floats(*a, *b),
    false,
    "`min()` on a `List<f64>` — the same total order `f64_sort` uses."
);
list_extreme!(
    lkrt_lklist_f64_max,
    f64,
    |value: &f64| crate::lkdyn::lkrt_dyn_from_f64(*value),
    |a: &f64, b: &f64| compare_floats(*a, *b),
    true,
    "`max()` on a `List<f64>`."
);
list_extreme!(
    lkrt_lklist_str_min,
    *const c_char,
    |value: &*const c_char| crate::lkdyn::lkrt_dyn_from_str(*value),
    |a: &*const c_char, b: &*const c_char| str_order(*a, *b),
    false,
    "`min()` on a `List<str>`."
);
list_extreme!(
    lkrt_lklist_str_max,
    *const c_char,
    |value: &*const c_char| crate::lkdyn::lkrt_dyn_from_str(*value),
    |a: &*const c_char, b: &*const c_char| str_order(*a, *b),
    true,
    "`max()` on a `List<str>`."
);

/// The `str_sort` comparator, as a function so `min`/`max` order strings the
/// same way rather than by a second copy of it.
fn str_order(left: *const c_char, right: *const c_char) -> core::cmp::Ordering {
    match (left.is_null(), right.is_null()) {
        (true, true) => core::cmp::Ordering::Equal,
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        // SAFETY: a non-null element of a live `str` list is a NUL-terminated
        // arena string.
        (false, false) => unsafe { CStr::from_ptr(left).to_bytes().cmp(CStr::from_ptr(right).to_bytes()) },
    }
}

list_sort!(
    lkrt_lklist_i64_sort,
    i64,
    |values| values.sort_unstable(),
    "`sort()` on a `List<i64>`."
);
list_sort!(
    lkrt_lklist_f64_sort,
    f64,
    |values| values.sort_by(|left, right| compare_floats(*left, *right)),
    "`sort()` on a `List<f64>`."
);
list_sort!(
    lkrt_lklist_str_sort,
    *const c_char,
    |values| values.sort_by(|left, right| {
        // A null element cannot occur in a live `str` list; ordering it first
        // keeps the comparator total rather than reaching for `CStr` on null.
        match (left.is_null(), right.is_null()) {
            (true, true) => core::cmp::Ordering::Equal,
            (true, false) => core::cmp::Ordering::Less,
            (false, true) => core::cmp::Ordering::Greater,
            (false, false) => unsafe { CStr::from_ptr(*left).to_bytes().cmp(CStr::from_ptr(*right).to_bytes()) },
        }
    }),
    "`sort()` on a `List<str>`."
);

/// `xs.reverse()` — a fresh reversed copy (non-mutating, like the VM).
///
/// Like [`list_clear`], the operation does not look at the element, so it is one
/// macro over every carrier. It was written for `i64` alone, which is why
/// `[1.5, 2.5].reverse()` dropped its whole module to the VM.
macro_rules! list_reverse {
    ($name:ident, $elem:ty, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void) -> *mut c_void {
            let mut values: Vec<$elem> = if handle.is_null() {
                Vec::new()
            } else {
                // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
                // constructor.
                unsafe { (*(handle as *mut Vec<$elem>)).clone() }
            };
            values.reverse();
            crate::state::arena_handle(values)
        }
    };
}

list_reverse!(lkrt_lklist_i64_reverse, i64, "`reverse()` on a `List<i64>`.");
list_reverse!(lkrt_lklist_f64_reverse, f64, "`reverse()` on a `List<f64>`.");
list_reverse!(
    lkrt_lklist_str_reverse,
    *const c_char,
    "`reverse()` on a `List<str>`. The element pointers are arena-owned and \
     shared with the source list, which is what makes copying them sound."
);
list_reverse!(
    lkrt_lklist_dyn_reverse,
    crate::lkdyn::LkDyn,
    "`reverse()` on a boxed-element list."
);

/// `xs.index_of(v)` — the first position holding `v`, or nil when absent.
///
/// The element comparison is the carrier's own, and it has to be *the same one*
/// its `contains` uses: in the VM both answer through one `typed_list_position`,
/// so a mismatch here would make `xs.contains(v)` and `xs.index_of(v) != nil`
/// disagree. Hence the comparison arrives as a function rather than being
/// spelled inside the macro — for the boxed carrier that means `contains_eq`
/// (the `in` operator's equality), not `dyn_eq_inner`.
/// `xs.count(v)` per carrier — `index_of`'s sibling, sharing its element
/// comparison so the two spellings of "which elements equal this" cannot
/// drift apart.
macro_rules! list_count {
    ($name:ident, $elem:ty, $needle:ty, $eq:expr, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, needle: $needle) -> i64 {
            if handle.is_null() {
                return 0;
            }
            // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
            // constructor.
            let values: &Vec<$elem> = unsafe { &*(handle as *mut Vec<$elem>) };
            let eq: fn(&$elem, $needle) -> bool = $eq;
            values.iter().filter(|value| eq(value, needle)).count() as i64
        }
    };
}

list_count!(
    lkrt_lklist_i64_count,
    i64,
    i64,
    |value, needle| *value == needle,
    "`count` on a `List<i64>`."
);
list_count!(
    lkrt_lklist_f64_count,
    f64,
    f64,
    |value, needle| *value == needle,
    "`count` on a `List<f64>`."
);

macro_rules! list_index_of {
    ($name:ident, $elem:ty, $needle:ty, $position:expr, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live list handle of the matching carrier, or null.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, needle: $needle) -> crate::lkdyn::LkDyn {
            if handle.is_null() {
                return crate::lkdyn::LkDyn::NIL;
            }
            // SAFETY: `handle` addresses a `Vec<$elem>` from the matching
            // constructor.
            let values: &Vec<$elem> = unsafe { &*(handle as *mut Vec<$elem>) };
            let position: fn(&[$elem], $needle) -> Option<usize> = $position;
            match position(values.as_slice(), needle) {
                Some(index) => crate::lkdyn::lkrt_dyn_from_i64(index as i64),
                None => crate::lkdyn::LkDyn::NIL,
            }
        }
    };
}

list_index_of!(
    lkrt_lklist_i64_index_of,
    i64,
    i64,
    |values, needle| values.iter().position(|value| *value == needle),
    "`index_of` on a `List<i64>`."
);
list_index_of!(
    lkrt_lklist_f64_index_of,
    f64,
    f64,
    |values, needle| values.iter().position(|value| *value == needle),
    "`index_of` on a `List<f64>`. An `Int` needle is coerced by the lowering, \
     the same way `contains` takes one."
);
list_index_of!(
    lkrt_lklist_str_index_of,
    *const c_char,
    *const c_char,
    |values, needle| {
        if needle.is_null() {
            return None;
        }
        // Converted once, not per element: `CStr::from_ptr` walks to the NUL.
        let needle = unsafe { CStr::from_ptr(needle) };
        values
            .iter()
            .position(|&p| !p.is_null() && unsafe { CStr::from_ptr(p) } == needle)
    },
    "`index_of` on a `List<str>`."
);
list_index_of!(
    lkrt_lklist_dyn_index_of,
    crate::lkdyn::LkDyn,
    crate::lkdyn::LkDyn,
    |values, needle| values.iter().position(|&e| crate::lkdyn::contains_eq(e, needle)),
    "`index_of` on a boxed-element list."
);

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
        return core::ptr::null();
    }
    // SAFETY: as above.
    let values = unsafe { &*(handle as *mut Vec<*const c_char>) };
    values.get(index as usize).copied().unwrap_or(core::ptr::null())
}

/// Joins the string elements with `separator`, returning a freshly allocated,
/// arena-registered C string. Matches the VM's `list.join` on a string list.
///
/// # Safety
/// `handle` as above; `separator` a valid C string (or null → empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_join(handle: *mut c_void, separator: *const c_char) -> *mut c_char {
    use alloc::ffi::CString;
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

/// Joins an `i64` list with `separator`, elements written as the VM writes them.
///
/// `[1, 2].join("-")` used to raise in the VM ("list must contain only strings")
/// and was therefore left unlowered here on purpose — one arbitrary rule turning
/// into a second one in another back end. The VM renders every element now, so
/// this renders them the same way: `i64::to_string`, exactly what
/// `lkrt_lklist_i64_display` puts between its brackets.
///
/// # Safety
/// See [`lkrt_lklist_str_join`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_join(handle: *mut c_void, separator: *const c_char) -> *mut c_char {
    use alloc::ffi::CString;
    let sep = join_separator(separator);
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<i64>` created by `lkrt_lklist_i64_new`.
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    let parts: Vec<String> = values.iter().map(i64::to_string).collect();
    crate::lkstr::arena_c_string(CString::new(parts.join(sep)).unwrap_or_default())
}

/// Joins an `f64` list with `separator`; see [`lkrt_lklist_i64_join`].
///
/// # Safety
/// See [`lkrt_lklist_str_join`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_join(handle: *mut c_void, separator: *const c_char) -> *mut c_char {
    use alloc::ffi::CString;
    let sep = join_separator(separator);
    let values: &[f64] = if handle.is_null() {
        &[]
    } else {
        // SAFETY: `handle` addresses a `Vec<f64>` created by `lkrt_lklist_f64_new`.
        unsafe { &*(handle as *mut Vec<f64>) }
    };
    let parts: Vec<String> = values.iter().map(f64::to_string).collect();
    crate::lkstr::arena_c_string(CString::new(parts.join(sep)).unwrap_or_default())
}

/// The separator a `join` was handed; null and invalid UTF-8 both mean empty,
/// matching the three `*_join` entry points that share it.
fn join_separator<'a>(separator: *const c_char) -> &'a str {
    if separator.is_null() {
        return "";
    }
    // SAFETY: caller guarantees a valid C string.
    unsafe { CStr::from_ptr(separator) }.to_str().unwrap_or("")
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

/// A typed list's elements, boxed — the carrier read behind every `DYN_TLIST_*`
/// consumer.
///
/// A copy, and only reads use it: `lkdyn::dyn_list_values` documents why, and
/// [`lkrt_dyn_list_push`](crate::lkrt_dyn_list_push) is the write that does not.
pub(crate) fn typed_list_boxed(kind: i64, handle: *mut c_void) -> alloc::vec::Vec<crate::lkdyn::LkDyn> {
    use crate::lkdyn::{TLIST_F64, TLIST_I64, TLIST_STR, lkrt_dyn_from_f64, lkrt_dyn_from_i64, lkrt_dyn_from_str};
    if handle.is_null() {
        return alloc::vec::Vec::new();
    }
    // SAFETY: `kind` names the carrier the caller tagged this handle with.
    unsafe {
        match kind {
            TLIST_I64 => (*(handle as *mut Vec<i64>))
                .iter()
                .map(|&v| lkrt_dyn_from_i64(v))
                .collect(),
            TLIST_F64 => (*(handle as *mut Vec<f64>))
                .iter()
                .map(|&v| lkrt_dyn_from_f64(v))
                .collect(),
            TLIST_STR => (*(handle as *mut Vec<*const c_char>))
                .iter()
                .map(|&v| lkrt_dyn_from_str(v))
                .collect(),
            _ => crate::panic::raise_str("runtime type error"),
        }
    }
}

/// Element count without boxing anything.
pub(crate) fn typed_list_len(kind: i64, handle: *mut c_void) -> i64 {
    use crate::lkdyn::{TLIST_F64, TLIST_I64, TLIST_STR};
    if handle.is_null() {
        return 0;
    }
    // SAFETY: as in [`typed_list_boxed`].
    unsafe {
        match kind {
            TLIST_I64 => (*(handle as *mut Vec<i64>)).len() as i64,
            TLIST_F64 => (*(handle as *mut Vec<f64>)).len() as i64,
            TLIST_STR => (*(handle as *mut Vec<*const c_char>)).len() as i64,
            _ => crate::panic::raise_str("runtime type error"),
        }
    }
}

/// `xs.push(v)` on a **boxed** typed list: appends to the carrier itself, so
/// the box and the original stay one list.
///
/// The element is unboxed back to the carrier's type. A value the carrier
/// cannot hold is the VM's loud failure — the same one the unboxed spelling
/// gives, because the static types would have rejected it there.
pub(crate) fn typed_list_push(kind: i64, handle: *mut c_void, value: crate::lkdyn::LkDyn) {
    use crate::lkdyn::{TLIST_F64, TLIST_I64, TLIST_STR};
    if handle.is_null() {
        crate::panic::raise_str("runtime type error");
    }
    // SAFETY: as in [`typed_list_boxed`], and the handle is uniquely reachable
    // through this call for its duration.
    unsafe {
        match kind {
            TLIST_I64 => (*(handle as *mut Vec<i64>)).push(crate::lkdyn::lkrt_dyn_as_i64(value)),
            TLIST_F64 => (*(handle as *mut Vec<f64>)).push(crate::lkdyn::lkrt_dyn_as_f64(value)),
            TLIST_STR => (*(handle as *mut Vec<*const c_char>)).push(crate::lkdyn::lkrt_dyn_as_str(value)),
            _ => crate::panic::raise_str("runtime type error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_structural_eq() {
        use alloc::ffi::CString;
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
            assert_eq!(lkrt_lklist_i64_eq(core::ptr::null_mut(), core::ptr::null_mut()), 1);

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
        use alloc::ffi::CString;
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
        use alloc::ffi::CString;
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
        use alloc::ffi::CString;
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
            assert_eq!(lkrt_lklist_i64_get_pair(core::ptr::null_mut(), 0).present, 0);
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
            assert_eq!(lkrt_lklist_i64_reduce_fn(core::ptr::null_mut(), 7, add), 7);
        }
    }
}
