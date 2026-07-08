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

use crate::lklist::{LkMaybeF64, LkMaybeI64};

// The exact carrier the VM uses (`core::util::fast_map::FastHashMap` =
// `hashbrown::HashMap` + `FxBuildHasher`, fixed seed): iteration order is a
// deterministic function of key hashes + operation sequence, so a native map
// built by the same operation sequence iterates in the *same* order — the
// deep-coverage plan's "mirror the Fx order" adjudication. Do not swap either
// piece independently of `core/src/util/fast_map.rs`.
pub(crate) type FxMap<K, V> = hashbrown::HashMap<K, V, rustc_hash::FxBuildHasher>;
type StrI64Map = FxMap<String, i64>;
type I64I64Map = FxMap<i64, i64>;
type StrF64Map = FxMap<String, f64>;
type I64F64Map = FxMap<i64, f64>;

/// Insert-or-update without allocating when the key is already present: the
/// common map workload pattern is repeated updates of existing keys, and
/// `insert(key.to_string(), ..)` would heap-allocate the key on every call.
fn set_str_key<V>(map: &mut FxMap<String, V>, key: &str, value: V) {
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

/// `{ ..rest }` over a `Map<str, Dyn>` (struct instances / mixed maps). See
/// [`lkrt_lkmap_str_i64_without`].
///
/// # Safety
/// `handle` must be a live `Map<str, Dyn>` handle (or null → empty); `key` a
/// NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_without(handle: *mut c_void, key: *const c_char) -> *mut c_void {
    let mut copy: StrDynMap = if handle.is_null() {
        StrDynMap::default()
    } else {
        // SAFETY: `handle` addresses a `StrDynMap` from `lkrt_lkmap_str_dyn_new`.
        unsafe { (*(handle as *mut StrDynMap)).clone() }
    };
    copy.remove(unsafe { key_str(key) });
    crate::state::arena_handle(copy)
}

/// `Struct { ..base, k: v }` — the VM's `__lk_merge_fields`: a fresh map
/// takes the base entries (base iteration order) whose keys the overlay
/// does not shadow, then every overlay entry (overlay iteration order) —
/// exactly the two-step insertion `merge_field_maps` performs, so the Fx
/// layout matches by the `vm_mirror` argument.
///
/// # Safety
/// Both handles must be live `Map<str, Dyn>` handles, or null (empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_merge(base: *mut c_void, overlay: *mut c_void) -> *mut c_void {
    let empty = StrDynMap::default();
    // SAFETY: caller passes live `StrDynMap` handles (or null).
    let base: &StrDynMap = if base.is_null() {
        &empty
    } else {
        unsafe { &*(base as *mut StrDynMap) }
    };
    // SAFETY: as above.
    let overlay: &StrDynMap = if overlay.is_null() {
        &empty
    } else {
        unsafe { &*(overlay as *mut StrDynMap) }
    };
    let mut out = StrDynMap::default();
    for (key, &value) in base {
        if !overlay.contains_key(key) {
            out.insert(key.clone(), value);
        }
    }
    for (key, &value) in overlay {
        out.insert(key.clone(), value);
    }
    crate::state::arena_handle(out)
}

/// Fresh zero-capacity rebuild in `src`'s iteration order — the VM's
/// `__lk_make_struct` copies the merged field map into the new object
/// (`runtime_object_fields_from_map`), so the native carrier replays the
/// same build to keep the iteration order identical.
///
/// # Safety
/// `src` must be a live `Map<str, Dyn>` handle, or null (empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_rebuild(src: *mut c_void) -> *mut c_void {
    let mut out = StrDynMap::default();
    if !src.is_null() {
        // SAFETY: caller passes a live `StrDynMap` handle.
        let src = unsafe { &*(src as *mut StrDynMap) };
        for (key, &value) in src {
            out.insert(key.clone(), value);
        }
    }
    crate::state::arena_handle(out)
}

// ── Iteration family (order = the VM's, by the layout-mirror argument in
// `vm_mirror.rs`): pair lists for `for pair in map`, keys/values snapshots,
// and delete-with-removed-value. Every produced list is a dyn list (the VM
// returns Mixed lists — bare-text display).

fn boxed_str_key(key: &str) -> crate::lkdyn::LkDyn {
    let owned = crate::lkstr::arena_c_string(std::ffi::CString::new(key).unwrap_or_default());
    crate::lkdyn::LkDyn {
        tag: crate::lkdyn::DYN_STR,
        payload: owned as i64,
    }
}

fn pair_list(entries: Vec<(crate::lkdyn::LkDyn, crate::lkdyn::LkDyn)>) -> *mut c_void {
    let pairs: Vec<crate::lkdyn::LkDyn> = entries
        .into_iter()
        .map(|(k, v)| {
            let pair: Vec<crate::lkdyn::LkDyn> = vec![k, v];
            crate::lkdyn::LkDyn {
                tag: crate::lkdyn::DYN_LIST,
                payload: crate::state::arena_handle(pair) as i64,
            }
        })
        .collect();
    crate::state::arena_handle(pairs)
}

macro_rules! map_iter_family {
    ($carrier:ty, $iter:ident, $keys:ident, $values:ident, $delete:ident, $box_val:expr, $unbox_doc:literal) => {
        #[doc = $unbox_doc]
        /// # Safety
        /// `handle` must be a live map handle of the matching carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $iter(handle: *mut c_void) -> *mut c_void {
            // SAFETY: `handle` addresses the matching carrier map.
            let map = unsafe { &*(handle as *mut $carrier) };
            #[allow(clippy::redundant_closure_call)]
            pair_list(
                map.iter()
                    .map(|(k, v)| (boxed_str_key(k), ($box_val)(v)))
                    .collect(),
            )
        }

        /// `.keys()` — a dyn list in iteration order.
        /// # Safety
        /// `handle` must be a live map handle of the matching carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $keys(handle: *mut c_void) -> *mut c_void {
            // SAFETY: as above.
            let map = unsafe { &*(handle as *mut $carrier) };
            let keys: Vec<crate::lkdyn::LkDyn> = map.keys().map(|k| boxed_str_key(k)).collect();
            crate::state::arena_handle(keys)
        }

        /// `.values()` — a dyn list in iteration order.
        /// # Safety
        /// `handle` must be a live map handle of the matching carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $values(handle: *mut c_void) -> *mut c_void {
            // SAFETY: as above.
            let map = unsafe { &*(handle as *mut $carrier) };
            #[allow(clippy::redundant_closure_call)]
            let values: Vec<crate::lkdyn::LkDyn> = map.values().map(|v| ($box_val)(v)).collect();
            crate::state::arena_handle(values)
        }

        /// `.delete(k)` — removes and returns the value, or nil when absent.
        /// # Safety
        /// `handle` must be a live map handle; `key` a NUL-terminated string.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $delete(handle: *mut c_void, key: *const c_char) -> crate::lkdyn::LkDyn {
            // SAFETY: as above.
            let map = unsafe { &mut *(handle as *mut $carrier) };
            #[allow(clippy::redundant_closure_call)]
            match map.remove(unsafe { key_str(key) }) {
                Some(v) => ($box_val)(&v),
                None => crate::lkdyn::LkDyn::NIL,
            }
        }
    };
}

map_iter_family!(
    StrI64Map,
    lkrt_lkmap_str_i64_iter_pairs,
    lkrt_lkmap_str_i64_keys,
    lkrt_lkmap_str_i64_values,
    lkrt_lkmap_str_i64_delete,
    |v: &i64| crate::lkdyn::lkrt_dyn_from_i64(*v),
    "`for pair in m` snapshot over `Map<str, i64>`."
);
map_iter_family!(
    StrF64Map,
    lkrt_lkmap_str_f64_iter_pairs,
    lkrt_lkmap_str_f64_keys,
    lkrt_lkmap_str_f64_values,
    lkrt_lkmap_str_f64_delete,
    |v: &f64| crate::lkdyn::lkrt_dyn_from_f64(*v),
    "`for pair in m` snapshot over `Map<str, f64>`."
);
// `Map<str, bool>` rides the i64 carrier; values box as Bool.
map_iter_family!(
    StrI64Map,
    lkrt_lkmap_str_bool_iter_pairs,
    lkrt_lkmap_str_bool_keys,
    lkrt_lkmap_str_bool_values,
    lkrt_lkmap_str_bool_delete,
    |v: &i64| crate::lkdyn::lkrt_dyn_from_bool(*v),
    "`for pair in m` snapshot over the bool map carrier."
);
map_iter_family!(
    StrDynMap,
    lkrt_lkmap_str_dyn_iter_pairs,
    lkrt_lkmap_str_dyn_keys,
    lkrt_lkmap_str_dyn_values,
    lkrt_lkmap_str_dyn_delete,
    |v: &crate::lkdyn::LkDyn| *v,
    "`for pair in m` snapshot over `Map<str, Dyn>`."
);

macro_rules! map_to_dyn {
    ($name:ident, $carrier:ty, $box_val:expr, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `handle` must be a live map handle of the matching carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handle: *mut c_void) -> *mut c_void {
            // SAFETY: `handle` addresses the matching carrier map.
            let map = unsafe { &*(handle as *mut $carrier) };
            let mut out = StrDynMap::default();
            for (k, v) in map.iter() {
                #[allow(clippy::redundant_closure_call)]
                out.insert(k.clone(), ($box_val)(v));
            }
            crate::state::arena_handle(out)
        }
    };
}

map_to_dyn!(
    lkrt_lkmap_str_i64_to_dyn,
    StrI64Map,
    |v: &i64| crate::lkdyn::lkrt_dyn_from_i64(*v),
    "`Map<str, i64>` → boxed-value map (iteration-order-preserving)."
);
map_to_dyn!(
    lkrt_lkmap_str_f64_to_dyn,
    StrF64Map,
    |v: &f64| crate::lkdyn::lkrt_dyn_from_f64(*v),
    "`Map<str, f64>` → boxed-value map."
);
map_to_dyn!(
    lkrt_lkmap_str_bool_to_dyn,
    StrI64Map,
    |v: &i64| crate::lkdyn::lkrt_dyn_from_bool(*v),
    "The bool map carrier → boxed-value map."
);

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

// ── Mixed-value map (`Map<str, LkDyn>`, plan M4.2 Dyn) ────────────────

pub(crate) type StrDynMap = FxMap<String, crate::lkdyn::LkDyn>;

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_str_dyn_new() -> *mut c_void {
    crate::state::arena_handle(StrDynMap::default())
}

/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_str_dyn_new`], or null;
/// `key` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_set(handle: *mut c_void, key: *const c_char, value: crate::lkdyn::LkDyn) {
    if handle.is_null() {
        return;
    }
    let map = unsafe { &mut *(handle as *mut StrDynMap) };
    set_str_key(map, unsafe { key_str(key) }, value);
}

/// A missing key is `nil` — the Dyn carrier's Nil tag *is* the absent case,
/// so no `Maybe` wrapper is needed (matches the VM's nil-on-missing-key).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_str_dyn_new`], or null;
/// `key` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_get(handle: *mut c_void, key: *const c_char) -> crate::lkdyn::LkDyn {
    if handle.is_null() {
        return crate::lkdyn::LkDyn::NIL;
    }
    let map = unsafe { &*(handle as *mut StrDynMap) };
    map.get(unsafe { key_str(key) })
        .copied()
        .unwrap_or(crate::lkdyn::LkDyn::NIL)
}

/// Key membership (distinct from `get`: a stored-nil value still counts).
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_str_dyn_new`], or null;
/// `key` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_has(handle: *mut c_void, key: *const c_char) -> i64 {
    if handle.is_null() {
        return 0;
    }
    let map = unsafe { &*(handle as *mut StrDynMap) };
    i64::from(map.contains_key(unsafe { key_str(key) }))
}

/// # Safety
/// `handle` must be a live handle from [`lkrt_lkmap_str_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_str_dyn_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    unsafe { &*(handle as *mut StrDynMap) }.len() as i64
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
