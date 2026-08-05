//! Native `Bytes` handles: an arena-owned `Vec<u8>`, mirroring the VM's
//! `HeapValue::Bytes`.
//!
//! There *was* already a "bytes" in this crate — the one-shot host handle the
//! `tcp` path uses (`HandleKind::Bytes`, read with `take_bytes`, which removes
//! it). That is right for "read a socket, decode it once" and wrong for a
//! *value*: `bytes.len(b)` followed by `bytes.to_string_utf8(b)` would find the
//! second call's handle already gone. A `Bytes` in the language is an ordinary
//! value you may read twice, so it gets an ordinary arena handle like
//! `List`/`Map`/`Set` do.
//!
//! Display is the VM's: `Bytes([104,105])` — the byte values, comma-separated,
//! no spaces, wrapped in `Bytes([…])`. Equality is content equality, which is
//! what the VM does too (unlike a struct, which compared by handle until it was
//! fixed).

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

use alloc::ffi::CString;
use core::ffi::{CStr, c_char, c_void};

use crate::lkdyn::LkDyn;
use crate::lkstr::arena_c_string;

type LkBytes = Vec<u8>;

fn bytes_ref<'a>(handle: *mut c_void) -> &'a LkBytes {
    debug_assert!(!handle.is_null(), "bytes handle must be live");
    // SAFETY: handles come from `arena_handle::<LkBytes>` and stay alive for
    // the arena's lifetime.
    unsafe { &*(handle as *const LkBytes) }
}

fn view<'a>(p: *const c_char) -> &'a str {
    if p.is_null() {
        return "";
    }
    // SAFETY: non-null pointers are NUL-terminated per the ABI.
    unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
}

fn out(text: String) -> *mut c_char {
    arena_c_string(CString::new(text).unwrap_or_default())
}

/// `bytes.from_string(s)` and the `s.bytes()` method — the string's UTF-8 bytes.
///
/// # Safety
/// `s` must be a valid C string, or null (→ empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_from_str(s: *const c_char) -> *mut c_void {
    crate::state::arena_handle(view(s).as_bytes().to_vec())
}

/// `bytes.len(b)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_len(handle: *mut c_void) -> i64 {
    bytes_ref(handle).len() as i64
}

/// `bytes.is_empty(b)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_is_empty(handle: *mut c_void) -> i64 {
    i64::from(bytes_ref(handle).is_empty())
}

/// `a == b` — content equality, the VM's rule.
///
/// # Safety
/// Both handles must be live `Bytes` handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_eq(left: *mut c_void, right: *mut c_void) -> i64 {
    i64::from(bytes_ref(left) == bytes_ref(right))
}

/// `bytes.get(b, i)` — the byte as an `Int`, or nil when out of range.
///
/// Negative indexes count from the end, the one rule every container's *read*
/// side follows (`docs/semantics.md`).
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_get(handle: *mut c_void, index: i64) -> LkDyn {
    let bytes = bytes_ref(handle);
    let resolved = if index < 0 { bytes.len() as i64 + index } else { index };
    if resolved < 0 || resolved >= bytes.len() as i64 {
        return LkDyn::NIL;
    }
    LkDyn {
        tag: crate::lkdyn::DYN_I64,
        payload: i64::from(bytes[resolved as usize]),
    }
}

/// `bytes.concat(a, b)`.
///
/// # Safety
/// Both handles must be live `Bytes` handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_concat(left: *mut c_void, right: *mut c_void) -> *mut c_void {
    let mut joined = bytes_ref(left).clone();
    joined.extend_from_slice(bytes_ref(right));
    crate::state::arena_handle(joined)
}

/// `bytes.to_string_utf8(b)` — raises on invalid UTF-8, with the stdlib
/// module's exact message.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_utf8(handle: *mut c_void) -> *mut c_char {
    match core::str::from_utf8(bytes_ref(handle)) {
        Ok(text) => out(text.to_owned()),
        Err(error) => crate::panic::raise_str(&alloc::format!("bytes are not valid UTF-8: {error}")),
    }
}

/// `sum()` / `min()` / `max()` on a `Bytes`.
///
/// A `Bytes` is a sequence of numbers, so it answers the same three reductions
/// a list does — and the empty answers match: `0` for the sum, nil for the two
/// extremes.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_sum(handle: *mut c_void) -> i64 {
    bytes_slice(handle)
        .iter()
        .fold(0i64, |total, byte| total.wrapping_add(i64::from(*byte)))
}

/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_min(handle: *mut c_void) -> crate::lkdyn::LkDyn {
    match bytes_slice(handle).iter().min() {
        Some(byte) => crate::lkdyn::lkrt_dyn_from_i64(i64::from(*byte)),
        None => crate::lkdyn::lkrt_dyn_from_nil(),
    }
}

/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_max(handle: *mut c_void) -> crate::lkdyn::LkDyn {
    match bytes_slice(handle).iter().max() {
        Some(byte) => crate::lkdyn::lkrt_dyn_from_i64(i64::from(*byte)),
        None => crate::lkdyn::lkrt_dyn_from_nil(),
    }
}

/// `bytes.to_string_lossy(b)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_utf8_lossy(handle: *mut c_void) -> *mut c_char {
    out(String::from_utf8_lossy(bytes_ref(handle)).into_owned())
}

/// `Bytes([104,105])` — the VM's display.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_to_str(handle: *mut c_void) -> *mut c_char {
    out(bytes_text(handle))
}

/// `Bytes([104,105])` as text — the same rendering [`lkrt_lkbytes_to_str`]
/// returns, reachable from the boxed-value renderer without going through a
/// C string and back.
pub(crate) fn bytes_text(handle: *mut c_void) -> String {
    let bytes = bytes_ref(handle);
    let mut text = String::with_capacity(bytes.len() * 4 + 9);
    text.push_str("Bytes([");
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&alloc::format!("{byte}"));
    }
    text.push_str("])");
    text
}

/// `bytes.slice(b, start[, end])` — a window, copied out as its own `Bytes`.
///
/// Positions follow the read rule: negative counts from the end, out of range
/// clamps, and a *reversed* window is empty.
///
/// It used to raise on `end < start`, which was the `bytes` module's rule —
/// while the method it shares this symbol with answered `Bytes([])`. So
/// `b.slice(2, 1)` raised compiled and answered an empty window interpreted,
/// and no corpus program had ever written a reversed window down. Every other
/// sequence clamps: `"abcde".slice(-1, -3)` and `xs.slice(-1, -3)` are empty.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_slice(handle: *mut c_void, start: i64, end: i64) -> *mut c_void {
    let bytes = bytes_ref(handle);
    let len = bytes.len() as i64;
    let resolve = |index: i64| {
        let resolved = if index < 0 { len + index } else { index };
        resolved.clamp(0, len) as usize
    };
    let (from, to) = (resolve(start), resolve(end));
    crate::state::arena_handle(bytes[from..to.max(from)].to_vec())
}

/// `bytes.take(n)` / `bytes.skip(n)` — a prefix and the rest of one.
///
/// A separate rule from `slice`: a *count* is not a position, so a negative one
/// is the loud error the VM gives rather than something measured from the end,
/// and a count past the end clamps. Same shape as `lklist`'s `list_window!`,
/// which is where the wording comes from.
macro_rules! bytes_window {
    ($name:ident, $method:literal, $window:expr, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live `Bytes` handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void, n: i64) -> *mut c_void {
            if n < 0 {
                crate::panic::raise_str(&alloc::format!(
                    concat!("bytes.", $method, "() count must be non-negative, got {}"),
                    n
                ));
            }
            let bytes = bytes_ref(handle);
            let cut = (n as usize).min(bytes.len());
            let window: fn(&[u8], usize) -> &[u8] = $window;
            crate::state::arena_handle(window(bytes, cut).to_vec())
        }
    };
}

bytes_window!(
    lkrt_lkbytes_take,
    "take",
    |bytes, cut| &bytes[..cut],
    "The first `n` bytes, or all of them."
);
bytes_window!(
    lkrt_lkbytes_skip,
    "skip",
    |bytes, cut| &bytes[cut..],
    "Everything after the first `n` bytes."
);

/// `bytes.index_of(v)` — the first position holding `v`, or nil.
///
/// Nil rather than -1, and returned as a `Dyn` rather than converted by the
/// caller, because -1 is a legal position: `xs[xs.index_of(v)]` would quietly
/// answer the *last* byte instead of failing. Same shape as `lklist`'s
/// `list_index_of!`, whose arms this mirrors.
///
/// A needle outside `0..=255` is not a byte, so it is simply absent.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_index_of(handle: *mut c_void, needle: i64) -> crate::lkdyn::LkDyn {
    if !(0..=255).contains(&needle) {
        return crate::lkdyn::LkDyn::NIL;
    }
    let needle = needle as u8;
    match bytes_ref(handle).iter().position(|&b| b == needle) {
        Some(index) => crate::lkdyn::lkrt_dyn_from_i64(index as i64),
        None => crate::lkdyn::LkDyn::NIL,
    }
}

/// `bytes.contains(v)` — 1 when present, 0 when not.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_contains(handle: *mut c_void, needle: i64) -> i64 {
    if !(0..=255).contains(&needle) {
        return 0;
    }
    let needle = needle as u8;
    i64::from(bytes_ref(handle).contains(&needle))
}

/// `bytes.from_list(values)` — a `List<Int>` of byte values.
///
/// Out-of-range values raise, matching the stdlib module: a "byte" that is not
/// one is a mistake, not something to truncate silently.
///
/// # Safety
/// `handle` must be a live `List<i64>` handle, or null (→ empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_from_i64_list(handle: *mut c_void) -> *mut c_void {
    if handle.is_null() {
        return crate::state::arena_handle(LkBytes::new());
    }
    // SAFETY: the caller passes a live `List<i64>` handle.
    let values = unsafe { &*(handle as *const Vec<i64>) };
    let mut bytes = LkBytes::with_capacity(values.len());
    for &value in values {
        match u8::try_from(value) {
            Ok(byte) => bytes.push(byte),
            Err(_) => crate::panic::raise_str(&alloc::format!("bytes.from_list() value {value} is not a byte (0-255)")),
        }
    }
    crate::state::arena_handle(bytes)
}

/// `bytes.to_list(b)` — the byte values as a `List<Int>`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_to_i64_list(handle: *mut c_void) -> *mut c_void {
    let values: Vec<i64> = bytes_ref(handle).iter().map(|&byte| i64::from(byte)).collect();
    crate::state::arena_handle(values)
}

/// The bytes behind a handle — for the host writers (`fs.write`, `tcp.write`),
/// which need the content without taking ownership of it.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
/// `b.reverse()` — the bytes in reverse order, as a new `Bytes`.
///
/// Shape-preserving and element-type-independent, so the answer is a `Bytes`
/// and not a list: the same reading `take`, `skip`, `slice` and `concat` take.
///
/// # Safety
/// `handle` must be a live handle from a `bytes_h` constructor, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_reverse(handle: *mut c_void) -> *mut c_void {
    let mut out = bytes_slice(handle).to_vec();
    out.reverse();
    bytes_handle(out)
}

/// `b.sort()` — the bytes in ascending order, as a new `Bytes`.
///
/// # Safety
/// `handle` must be a live handle from a `bytes_h` constructor, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_sort(handle: *mut c_void) -> *mut c_void {
    let mut out = bytes_slice(handle).to_vec();
    out.sort_unstable();
    bytes_handle(out)
}

/// `b.unique()` — later duplicates dropped, order kept, as a new `Bytes`.
///
/// 256 possible values, so the "seen" set is a bitmap rather than a hash set.
///
/// # Safety
/// `handle` must be a live handle from a `bytes_h` constructor, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_unique(handle: *mut c_void) -> *mut c_void {
    let bytes = bytes_slice(handle);
    let mut seen = [false; 256];
    let mut out = Vec::with_capacity(bytes.len());
    for byte in bytes {
        if !seen[*byte as usize] {
            seen[*byte as usize] = true;
            out.push(*byte);
        }
    }
    bytes_handle(out)
}

/// `b.count(v)` — how many bytes equal `v`. A value no byte can hold counts
/// zero, which is the answer `contains` gives it too.
///
/// # Safety
/// `handle` must be a live handle from a `bytes_h` constructor, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkbytes_count(handle: *mut c_void, value: i64) -> i64 {
    match u8::try_from(value) {
        Ok(needle) => bytes_slice(handle).iter().filter(|byte| **byte == needle).count() as i64,
        Err(_) => 0,
    }
}

pub(crate) fn bytes_slice<'a>(handle: *mut c_void) -> &'a [u8] {
    bytes_ref(handle).as_slice()
}

/// Builds a `Bytes` from a byte slice — the constructor the decoders use.
pub(crate) fn bytes_handle(bytes: Vec<u8>) -> *mut c_void {
    crate::state::arena_handle(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from(text: &str) -> *mut c_void {
        let c = CString::new(text).expect("no interior NUL");
        unsafe { lkrt_lkbytes_from_str(c.as_ptr()) }
    }

    fn rendered(handle: *mut c_void) -> String {
        let ptr = unsafe { lkrt_lkbytes_to_str(handle) };
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }

    #[test]
    fn display_and_equality_match_the_vm() {
        let hi = from("hi");
        assert_eq!(rendered(hi), "Bytes([104,105])");
        assert_eq!(rendered(from("")), "Bytes([])");
        // Content equality, not handle identity.
        assert_eq!(unsafe { lkrt_lkbytes_eq(hi, from("hi")) }, 1);
        assert_eq!(unsafe { lkrt_lkbytes_eq(hi, from("ho")) }, 0);
        assert_eq!(unsafe { lkrt_lkbytes_len(hi) }, 2);
        assert_eq!(unsafe { lkrt_lkbytes_is_empty(from("")) }, 1);
    }

    /// A value you may read twice — the reason this is an arena handle rather
    /// than the one-shot host handle the `tcp` path uses.
    #[test]
    fn a_handle_survives_being_read_twice() {
        let hi = from("hi");
        assert_eq!(unsafe { lkrt_lkbytes_len(hi) }, 2);
        let text = unsafe { lkrt_lkbytes_utf8(hi) };
        assert_eq!(unsafe { CStr::from_ptr(text) }.to_str().expect("utf-8"), "hi");
        assert_eq!(unsafe { lkrt_lkbytes_len(hi) }, 2);
    }

    /// Negative indexes count from the end, and out of range is nil — the one
    /// rule every container's read side follows.
    #[test]
    fn get_counts_from_the_end_and_answers_nil_out_of_range() {
        let hi = from("hi");
        assert_eq!(unsafe { lkrt_lkbytes_get(hi, 0) }.payload, 104);
        assert_eq!(unsafe { lkrt_lkbytes_get(hi, -1) }.payload, 105);
        assert_eq!(unsafe { lkrt_lkbytes_get(hi, 2) }.tag, crate::lkdyn::DYN_NIL);
        assert_eq!(unsafe { lkrt_lkbytes_get(hi, -3) }.tag, crate::lkdyn::DYN_NIL);
    }

    #[test]
    fn concat_joins_and_lossy_never_raises() {
        let joined = unsafe { lkrt_lkbytes_concat(from("hi"), from("!")) };
        assert_eq!(rendered(joined), "Bytes([104,105,33])");
        let invalid = bytes_handle(vec![0xff]);
        let lossy = unsafe { lkrt_lkbytes_utf8_lossy(invalid) };
        assert_eq!(unsafe { CStr::from_ptr(lossy) }.to_str().expect("utf-8"), "\u{fffd}");
    }
}
