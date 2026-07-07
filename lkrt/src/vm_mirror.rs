//! VM map-layout mirror (deep-coverage plan D1, user adjudication: "native
//! replicates the Fx order, the VM is untouched").
//!
//! The VM materializes a map literal in **two stages**
//! (`exec/const_load.rs` + `val/runtime_model.rs::typed_map_from_entries`):
//! stage 1 inserts the serialized entries, in order, into a fresh
//! `FastHashMap<RuntimeMapKey, RuntimeVal>`; stage 2 iterates *that* map (Fx
//! hash order) and inserts into the final typed map keyed by `Arc<str>`.
//! Iteration order of the result is therefore a deterministic function of
//! the key hashes and both insertion sequences — nothing else. This module
//! replays both stages with hash-identical key types, so `for k in m` /
//! `.keys()` iterate in exactly the VM's order.
//!
//! Hash identity argument: [`RtKey`] mirrors `RuntimeMapKey`'s variant order
//! (same `derive(Hash)` discriminants under the same rustc) and field hashing
//! (`MirrorShortStr` = `ShortStr`'s exact field order; `String` hashes its
//! `str` content exactly like `Arc<str>`); the hasher and table
//! implementation are the same `hashbrown + FxBuildHasher` the VM's
//! `fast_map` uses, resolved to one version by the workspace lockfile. The
//! lkrt order-conformance test compares against `lk-core` directly, so a
//! drift in any of these assumptions fails loudly.

use core::ffi::{CStr, c_char, c_void};

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_NIL, DYN_STR, LkDyn};
use crate::lkmap::{FxMap, StrDynMap};
use crate::state::arena_handle;

/// Field-order/type mirror of `lk_values::ShortStr` (`len: u8, data: [u8; 7]`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MirrorShortStr {
    len: u8,
    data: [u8; 7],
}

/// Variant-order mirror of `core::val::RuntimeMapKey`. `Obj` is never
/// constructed here (heap-handle keys are outside the native subset) but
/// keeps the discriminant numbering aligned.
#[derive(Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
enum RtKey {
    Nil,
    Bool(bool),
    Int(i64),
    ShortStr(MirrorShortStr),
    String(String),
    Obj(u64),
}

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
            // The VM's canonical string split: ≤ 7 bytes is always the
            // inline `ShortStr` runtime value, 8+ always a heap string.
            if text.len() <= 7 {
                let mut data = [0u8; 7];
                data[..text.len()].copy_from_slice(text.as_bytes());
                RtKey::ShortStr(MirrorShortStr {
                    len: text.len() as u8,
                    data,
                })
            } else {
                RtKey::String(text.to_owned())
            }
        }
        // Float keys are the VM's loud "cannot be used as a key" error;
        // container keys (heap-handle identity) are outside the subset.
        _ => crate::panic::raise_str("runtime error"),
    }
}

fn key_str(key: &RtKey) -> &str {
    match key {
        RtKey::ShortStr(s) => core::str::from_utf8(&s.data[..s.len as usize]).unwrap_or(""),
        RtKey::String(s) => s.as_str(),
        _ => crate::panic::raise_str("runtime error"),
    }
}

/// Stage-1 literal builder: the mirror of the VM's
/// `FastHashMap<RuntimeMapKey, RuntimeVal>` (values ride along boxed).
type LitBuilder = FxMap<RtKey, LkDyn>;

/// Starts a map-literal build (VM stage 1, zero capacity).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_lit_new() -> *mut c_void {
    arena_handle(LitBuilder::default())
}

/// Inserts one literal entry, in source order (VM `read_map_entries`).
///
/// # Safety
/// `builder` must be a live handle from [`lkrt_lkmap_lit_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_set(builder: *mut c_void, key: LkDyn, value: LkDyn) {
    // SAFETY: `builder` addresses a `LitBuilder` from `lkrt_lkmap_lit_new`.
    let map = unsafe { &mut *(builder as *mut LitBuilder) };
    map.insert(key_from_dyn(key), value);
}

fn builder<'a>(handle: *mut c_void) -> &'a LitBuilder {
    // SAFETY: callers pass a live `LitBuilder` handle.
    unsafe { &*(handle as *mut LitBuilder) }
}

/// Finishes into `Map<str, i64>` (VM stage 2: iterate stage 1 in its hash
/// order, insert into a fresh zero-capacity typed map).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_lit_new`] whose keys are
/// strings and values `I64`-tagged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_str_i64(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<String, i64> = FxMap::default();
    for (key, value) in builder(handle) {
        if value.tag != DYN_I64 {
            crate::panic::raise_str("runtime error");
        }
        out.insert(key_str(key).to_owned(), value.payload);
    }
    arena_handle(out)
}

/// Finishes into `Map<str, f64>`. See [`lkrt_lkmap_lit_finish_str_i64`].
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`], with `F64`-tagged values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_str_f64(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<String, f64> = FxMap::default();
    for (key, value) in builder(handle) {
        if value.tag != DYN_F64 {
            crate::panic::raise_str("runtime error");
        }
        out.insert(key_str(key).to_owned(), f64::from_bits(value.payload as u64));
    }
    arena_handle(out)
}

/// Finishes into the `Map<str, bool>` carrier (values 0/1 on the i64 map,
/// like the rest of the `str_i64` bool ABI). Layout only depends on keys.
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`], with `Bool`-tagged values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_str_bool(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<String, i64> = FxMap::default();
    for (key, value) in builder(handle) {
        if value.tag != DYN_BOOL {
            crate::panic::raise_str("runtime error");
        }
        out.insert(key_str(key).to_owned(), value.payload);
    }
    arena_handle(out)
}

/// Finishes into `Map<str, Dyn>` (mixed values stay boxed).
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`]; any boxed value is fine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_str_dyn(handle: *mut c_void) -> *mut c_void {
    let mut out: StrDynMap = StrDynMap::default();
    for (key, value) in builder(handle) {
        out.insert(key_str(key).to_owned(), *value);
    }
    arena_handle(out)
}

/// Finishes into `Map<i64, i64>`.
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`], with `Int` keys and values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_i64_i64(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<i64, i64> = FxMap::default();
    for (key, value) in builder(handle) {
        let RtKey::Int(k) = key else {
            crate::panic::raise_str("runtime error")
        };
        if value.tag != DYN_I64 {
            crate::panic::raise_str("runtime error");
        }
        out.insert(*k, value.payload);
    }
    arena_handle(out)
}

/// Finishes into `Map<i64, f64>`.
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`], with `Int` keys, `F64` values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_i64_f64(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<i64, f64> = FxMap::default();
    for (key, value) in builder(handle) {
        let RtKey::Int(k) = key else {
            crate::panic::raise_str("runtime error")
        };
        if value.tag != DYN_F64 {
            crate::panic::raise_str("runtime error");
        }
        out.insert(*k, f64::from_bits(value.payload as u64));
    }
    arena_handle(out)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::lkdyn::{lkrt_dyn_from_i64, lkrt_dyn_from_str};
    use crate::lkstr::arena_c_string;
    use std::ffi::CString;

    fn str_key(text: &str) -> LkDyn {
        let ptr = arena_c_string(CString::new(text).unwrap());
        lkrt_dyn_from_str(ptr)
    }

    /// The load-bearing conformance check: a map literal built through the
    /// lit protocol iterates in exactly the order the VM's two-stage
    /// construction produces (`typed_map_from_entries` over the same keys in
    /// the same insertion order). Any drift — hashbrown version split,
    /// `RuntimeMapKey`/`ShortStr` shape change, hasher change — fails here
    /// before it can reach the byte-exact differential gates.
    #[test]
    fn lit_protocol_matches_vm_iteration_order() {
        use lk_core::val::{RuntimeMapKey, RuntimeVal, typed_map_iteration_keys};

        // Mixed short (≤7B, inline) and long (heap) keys, plus enough of
        // them to force several table growths on both sides.
        let keys: Vec<String> = (0..64)
            .map(|i| {
                if i % 3 == 0 {
                    format!("key_number_{i}")
                } else {
                    format!("k{i}")
                }
            })
            .collect();

        let vm_order = typed_map_iteration_keys(keys.iter().map(|k| (k.as_str(), 1i64)));

        let builder_handle = lkrt_lkmap_lit_new();
        for key in &keys {
            unsafe { lkrt_lkmap_lit_set(builder_handle, str_key(key), lkrt_dyn_from_i64(1)) };
        }
        let map_handle = unsafe { lkrt_lkmap_lit_finish_str_i64(builder_handle) };
        // SAFETY: just built by the finisher above.
        let native = unsafe { &*(map_handle as *mut FxMap<String, i64>) };
        let native_order: Vec<String> = native.keys().cloned().collect();

        assert_eq!(
            native_order, vm_order,
            "lit-protocol iteration order drifted from the VM's typed_map_from_entries"
        );
        let _ = RuntimeMapKey::Nil;
        let _ = RuntimeVal::Nil;
    }
}
