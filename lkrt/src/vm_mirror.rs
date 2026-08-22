//! VM map-layout mirror (deep-coverage plan D1, user adjudication: "native
//! replicates the Fx order, the VM is untouched").
//!
//! The VM materializes a map literal in two stages (`exec/const_load.rs` +
//! `val/runtime_model.rs::typed_map_from_entries`): stage 1 inserts the
//! serialized entries, in order, into a `ValueMap<RuntimeMapKey, RuntimeVal>`;
//! stage 2 iterates that and inserts into the final typed map keyed by
//! `Arc<str>`. Both carriers are insertion-ordered, so the result iterates in
//! the order the literal was *written*. This module replays the same two
//! stages.
//!
//! **The hash-identity argument is retired.** Both sides now carry a map's
//! entries in a vector and iterate it, so `for k in m` agrees between the two
//! back ends because both append — not because both land on the same hash
//! layout. What that used to rest on is worth recording, since it is the kind
//! of invariant that holds until it silently does not: `RtKey` had to mirror
//! `RuntimeMapKey`'s `derive(Hash)` discriminants under the same rustc, and
//! both builds had to resolve to one `hashbrown` with one fixed seed. `RtKey`
//! now only has to be *self*-consistent — equal keys hash equally — which is
//! an ordinary requirement rather than a coincidence to defend.
//!
//! The order-conformance test stays: it compares against `lk-core` directly,
//! so a divergence still fails loudly.

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

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_NIL, DYN_STR, LkDyn};
use crate::lkmap::{FxMap, StrDynMap};
use crate::state::arena_handle;

/// Field-order/type mirror of `lk_values::ShortStr` (`len: u8, data: [u8; 7]`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MirrorShortStr {
    len: u8,
    data: [u8; 7],
}

/// Variant-order mirror of `core::val::RuntimeMapKey`. `Obj` is never
/// constructed here (heap-handle keys are outside the native subset) but
/// keeps the discriminant numbering aligned.
#[derive(Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) enum RtKey {
    Nil,
    Bool(bool),
    Int(i64),
    ShortStr(MirrorShortStr),
    String(String),
    Obj(u64),
}

/// The VM's canonical string key: ≤ 7 bytes is always the inline `ShortStr`
/// runtime value, 8+ always a heap string. The split is by length alone, so it
/// is deterministic — and it is *load-bearing for the hash*, which is why a
/// set cannot keep its own one-variant version of this and still iterate in the
/// VM's order.
pub(crate) fn str_key(text: &str) -> RtKey {
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

pub(crate) fn key_from_dyn(v: LkDyn) -> RtKey {
    key_from_dyn_in(v, "")
}

/// The key a value would be, or `None` when it cannot be one.
///
/// For *membership* only: `1.5 in s` is `false` rather than a refusal, because
/// a value that cannot be a key is not a member and `in` is a predicate. See
/// the interpreter's `map_contains`, which says the same thing at more length —
/// building the key and propagating its failure made the answer depend on the
/// map's internal carrier, which no program can see.
pub(crate) fn key_from_dyn_opt(v: LkDyn) -> Option<RtKey> {
    match v.tag {
        DYN_NIL | DYN_BOOL | DYN_I64 | DYN_STR => Some(key_from_dyn(v)),
        _ => None,
    }
}

/// [`key_from_dyn`] with the call named, for the paths where the interpreter
/// prefixes the refusal with it (`Set() item: …`, `set.add() value: …`). A
/// caught error is printed output, so the prefix is part of the answer.
pub(crate) fn key_from_dyn_in(v: LkDyn, context: &str) -> RtKey {
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
            str_key(text)
        }
        // Everything else is the VM's loud "cannot be used as a key" error, and
        // it has **two** wordings: a `Float` says only that, because the reason
        // is the float itself (`0.0` and `-0.0` are equal and hash apart, and
        // `NaN` is not equal to itself), while any other value names its type
        // and lists what may be a key (`RuntimeMapKey::from_value`). One
        // wording for both said `Float` about a `Bytes` and about a function —
        // a caught error is printed output, so it was a wrong answer, not just
        // a poor message.
        crate::lkdyn::DYN_F64 => crate::panic::raise_str(&alloc::format!(
            "{}Float cannot be a map key or set member",
            prefix(context)
        )),
        _ => crate::panic::raise_str(&alloc::format!(
            "{}{} cannot be a map key or set member: only nil, Bool, Int and String can",
            prefix(context),
            crate::lkdyn::kind_name_of(v)
        )),
    }
}

fn prefix(context: &str) -> alloc::string::String {
    if context.is_empty() {
        alloc::string::String::new()
    } else {
        alloc::format!("{context}: ")
    }
}

pub(crate) fn key_str(key: &RtKey) -> &str {
    match key {
        RtKey::ShortStr(s) => core::str::from_utf8(&s.data[..s.len as usize]).unwrap_or(""),
        RtKey::String(s) => s.as_str(),
        _ => crate::panic::raise_str("runtime error"),
    }
}

/// Builds a `Map<str, Dyn>` through the two-stage mirror from already-owned
/// pairs, in the given order (the decoders' path: serde's document/sorted
/// order plays the VM's stage-1 insertion order).
pub(crate) fn str_dyn_map_mirrored(pairs: Vec<(String, LkDyn)>) -> *mut c_void {
    let mut stage1: FxMap<RtKey, LkDyn> = FxMap::default();
    for (key, value) in pairs {
        stage1.insert(str_key(&key), value);
    }
    let mut out = StrDynMap::default();
    for (key, value) in &stage1 {
        out.insert(crate::lkmap::StrKey::Owned(key_str(key).to_owned()), *value);
    }
    arena_handle(out)
}

/// A map key that is an `i64`, hashing exactly as [`RtKey::Int`] does.
///
/// The int-keyed carriers are keyed by this rather than by a bare `i64`
/// because the VM never re-keys them: `typed_map_from_entries` returns
/// `Mixed` for a non-string key, and `Mixed` *is* the stage-1
/// `FastHashMap<RuntimeMapKey, RuntimeVal>`. A native `FxMap<i64, _>` hashes
/// the key differently (no discriminant) and is filled by a second insertion
/// sequence, so it iterates in a different order — `{1: 1.5, 2: 2.5}` came out
/// `2,1` in the VM and `1,2` natively.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct IntKey(pub(crate) i64);

/// `RtKey`'s derived `Hash` writes the discriminant first. `Int` is the third
/// variant, and a repr-less enum's discriminant is an `isize`.
///
/// Written out rather than delegating to `RtKey::Int(k).hash(state)` so a map
/// lookup does not build the (String-carrying, 32-byte) enum;
/// `int_key_hashes_like_the_mirror_enum` is what keeps the two in agreement.
const RTKEY_INT_DISCRIMINANT: isize = 2;

impl core::hash::Hash for IntKey {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        RTKEY_INT_DISCRIMINANT.hash(state);
        self.0.hash(state);
    }
}

/// Stage-1 literal builder: the mirror of the VM's
/// `FastHashMap<RuntimeMapKey, RuntimeVal>` (values ride along boxed), plus
/// the order the entries were written in.
///
/// One field, now. There used to be a second — an explicit log of first-
/// occurrence order — because the table's own iteration was hash order and a
/// non-string-keyed literal (which gets no stage 2 in the VM) had to replay the
/// *written* sequence instead. The table iterates in that sequence itself now,
/// so the log was a copy of it.
#[derive(Default)]
struct LitBuilder {
    stage1: FxMap<RtKey, LkDyn>,
}

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
    let lit = unsafe { &mut *(builder as *mut LitBuilder) };
    // A repeated key updates in place and keeps its original position, which
    // is `IndexMap::insert`'s own behaviour.
    lit.stage1.insert(key_from_dyn(key), value);
}

fn builder<'a>(handle: *mut c_void) -> &'a FxMap<RtKey, LkDyn> {
    // SAFETY: callers pass a live `LitBuilder` handle.
    &unsafe { &*(handle as *mut LitBuilder) }.stage1
}

/// The literal's entries in written order, which is what the table gives.
fn literal_order<'a>(handle: *mut c_void) -> impl Iterator<Item = (&'a RtKey, &'a LkDyn)> {
    builder(handle).iter()
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
        out.insert(crate::lkmap::StrKey::Owned(key_str(key).to_owned()), *value);
    }
    arena_handle(out)
}

/// Finishes into `Map<i64, i64>`.
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`], with `Int` keys and values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_i64_i64(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<IntKey, i64> = FxMap::default();
    for (key, value) in literal_order(handle) {
        let RtKey::Int(k) = key else {
            crate::panic::raise_str("runtime error")
        };
        if value.tag != DYN_I64 {
            crate::panic::raise_str("runtime error");
        }
        out.insert(IntKey(*k), value.payload);
    }
    arena_handle(out)
}

/// Finishes into `Map<i64, f64>`.
///
/// # Safety
/// As [`lkrt_lkmap_lit_finish_str_i64`], with `Int` keys, `F64` values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkmap_lit_finish_i64_f64(handle: *mut c_void) -> *mut c_void {
    let mut out: FxMap<IntKey, f64> = FxMap::default();
    for (key, value) in literal_order(handle) {
        let RtKey::Int(k) = key else {
            crate::panic::raise_str("runtime error")
        };
        if value.tag != DYN_F64 {
            crate::panic::raise_str("runtime error");
        }
        out.insert(IntKey(*k), f64::from_bits(value.payload as u64));
    }
    arena_handle(out)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::lkdyn::{lkrt_dyn_from_i64, lkrt_dyn_from_str};
    use crate::lkstr::arena_c_string;
    use alloc::ffi::CString;

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

    fn fx_hash(value: impl core::hash::Hash) -> u64 {
        use core::hash::BuildHasher;
        rustc_hash::FxBuildHasher.hash_one(value)
    }

    /// [`IntKey`] exists to hash exactly like [`RtKey::Int`], and it writes the
    /// discriminant out by hand rather than building the enum. This is what
    /// says the hand-written version is the same one — including the
    /// assumption that a repr-less enum's discriminant hashes as an `isize`.
    #[test]
    fn int_key_hashes_like_the_mirror_enum() {
        for k in [0i64, 1, -1, 2, 42, -9999, i64::MAX, i64::MIN] {
            assert_eq!(
                fx_hash(IntKey(k)),
                fx_hash(RtKey::Int(k)),
                "IntKey({k}) must hash as RtKey::Int({k})"
            );
        }
    }

    /// The int-keyed counterpart of the load-bearing check above, and the one
    /// that would have caught the divergence: the VM runs *no* stage 2 for a
    /// non-string key (`typed_map_from_entries` hands back the stage-1 table),
    /// so the finisher replays the literal insertion sequence instead of
    /// iterating stage 1. Rehashing into an `FxMap<i64, _>` — which is what it
    /// used to do — made `{1: 1.5, 2: 2.5}` iterate `1,2` against the VM's
    /// `2,1`.
    #[test]
    fn int_lit_protocol_matches_vm_iteration_order() {
        use lk_core::val::typed_map_iteration_int_keys;

        // Small literals (where the divergence first showed) and a large one
        // that forces several table growths.
        for keys in [
            vec![1i64, 2],
            vec![1, 3],
            vec![1, 2, 5, 9],
            vec![-3, 7, 0, 12, -100],
            (0..64).map(|i| i * 7 - 13).collect::<Vec<_>>(),
        ] {
            let vm_order = typed_map_iteration_int_keys(keys.iter().map(|&k| (k, k * 2)));

            let builder_handle = lkrt_lkmap_lit_new();
            for &k in &keys {
                unsafe { lkrt_lkmap_lit_set(builder_handle, lkrt_dyn_from_i64(k), lkrt_dyn_from_i64(k * 2)) };
            }
            let map_handle = unsafe { lkrt_lkmap_lit_finish_i64_i64(builder_handle) };
            // SAFETY: just built by the finisher above.
            let native = unsafe { &*(map_handle as *mut FxMap<IntKey, i64>) };
            let native_order: Vec<i64> = native.keys().map(|k| k.0).collect();

            assert_eq!(
                native_order, vm_order,
                "int-keyed iteration order drifted from the VM's typed_map_from_entries for {keys:?}"
            );
        }
    }
}
