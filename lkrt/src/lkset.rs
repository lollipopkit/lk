//! Native `Set` handles (plan deep-coverage B1): mirrors the VM's
//! `RuntimeSet` — a hash set of map keys (`Nil`/`Bool`/`Int`/strings; a
//! `Float` key is the VM's loud error, containers would need heap-handle
//! identity and stay out of the native subset). Elements arrive as boxed
//! `LkDyn` values.
//!
//! Iteration *is* exposed, and the reason it was not is worth keeping: this
//! module used to define its own key type. See the `use` below.

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

use crate::lkdyn::LkDyn;
// The *same* key type the map mirror uses, not a second one.
//
// This module used to define its own four-variant `RtKey` that folded both
// string shapes into one `Str(String)`, on the argument that the VM's split is
// by length and so equality is unaffected. True for equality — and false for
// the **hash**, which is what a set's iteration order is made of. So the two
// definitions agreed about membership and disagreed about order, and the way
// that showed up was `for x in s` never being lowered at all (this module's own
// header said "iteration is not exposed (hash order)").
//
// One key type, one hash, and the order conformance test can then say something.
use crate::vm_mirror::{RtKey, key_from_dyn, key_from_dyn_in, key_str, str_key};

type LkSet = FxSet<RtKey>;

fn set_mut<'a>(handle: *mut c_void) -> &'a mut LkSet {
    // SAFETY: `handle` addresses an `LkSet` from `lkrt_lkset_new`/`from_*`.
    unsafe { &mut *(handle as *mut LkSet) }
}

/// The set operations, with the **insertion sequence** the VM states: the
/// receiver's members in its own order, then the argument's in its own order.
///
/// A set's iteration order is its hash order, so filling the answer in another
/// sequence gives the same members in a different order — and a set printed one
/// way here and another way there is a wrong answer under the mirror
/// discipline, not a cosmetic difference. `kind` selects the operation;
/// `SET_OP_*` names the numbering.
///
/// # Safety
/// Both handles must be live `Set` handles from this module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_combine(a: *mut c_void, b: *mut c_void, kind: i64) -> *mut c_void {
    let (mine, theirs) = (set_mut(a), set_mut(b));
    let mut out = LkSet::default();
    match kind {
        SET_OP_UNION => {
            out.extend(mine.iter().cloned());
            out.extend(theirs.iter().cloned());
        }
        SET_OP_INTERSECTION => out.extend(mine.iter().filter(|key| theirs.contains(*key)).cloned()),
        SET_OP_DIFFERENCE => out.extend(mine.iter().filter(|key| !theirs.contains(*key)).cloned()),
        SET_OP_SYMMETRIC_DIFFERENCE => {
            out.extend(mine.iter().filter(|key| !theirs.contains(*key)).cloned());
            out.extend(theirs.iter().filter(|key| !mine.contains(*key)).cloned());
        }
        _ => crate::panic::raise_str("runtime type error"),
    }
    crate::state::arena_handle(out)
}

/// The three predicates. `is_disjoint` stops at the first shared member and
/// allocates nothing, which is why it is not `intersection().is_empty()`.
///
/// # Safety
/// As [`lkrt_lkset_combine`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_relate(a: *mut c_void, b: *mut c_void, kind: i64) -> i64 {
    let (mine, theirs) = (set_mut(a), set_mut(b));
    let answer = match kind {
        SET_REL_SUBSET => mine.iter().all(|key| theirs.contains(key)),
        SET_REL_SUPERSET => theirs.iter().all(|key| mine.contains(key)),
        SET_REL_DISJOINT => !mine.iter().any(|key| theirs.contains(key)),
        _ => crate::panic::raise_str("runtime type error"),
    };
    i64::from(answer)
}

/// `union`. See [`lkrt_lkset_combine`].
pub const SET_OP_UNION: i64 = 0;
/// `intersection`.
pub const SET_OP_INTERSECTION: i64 = 1;
/// `difference`.
pub const SET_OP_DIFFERENCE: i64 = 2;
/// `symmetric_difference`.
pub const SET_OP_SYMMETRIC_DIFFERENCE: i64 = 3;
/// `is_subset`. See [`lkrt_lkset_relate`].
pub const SET_REL_SUBSET: i64 = 0;
/// `is_superset`.
pub const SET_REL_SUPERSET: i64 = 1;
/// `is_disjoint`.
pub const SET_REL_DISJOINT: i64 = 2;

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
            set.insert(str_key(text));
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
            set.insert(key_from_dyn_in(item, "Set() item"));
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
    let key = key_from_dyn_in(value, "set.add() value");
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

/// `for x in s` — a snapshot of the members as a dyn list, in the set's own
/// iteration order.
///
/// The order is the hash layout's, and it is the VM's because both sides key by
/// the *same* [`RtKey`] and fill by the same insertion sequence — a set has no
/// second stage, so there is nothing else in the order. This could not be
/// exposed while this module kept its own one-variant string key: membership
/// agreed, the hash did not.
///
/// # Safety
/// `handle` must be a live `Set` handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lkset_iter(handle: *mut c_void) -> *mut c_void {
    let empty = LkSet::default();
    // SAFETY: caller passes a live `LkSet` handle.
    let set: &LkSet = if handle.is_null() {
        &empty
    } else {
        unsafe { &*(handle as *mut LkSet) }
    };
    let members: Vec<LkDyn> = set.iter().map(member_dyn).collect();
    crate::state::arena_handle(members)
}

/// One member, boxed back into the value it was made from.
fn member_dyn(key: &RtKey) -> LkDyn {
    match key {
        RtKey::Nil => LkDyn::NIL,
        RtKey::Bool(v) => crate::lkdyn::lkrt_dyn_from_bool(i64::from(*v)),
        RtKey::Int(v) => crate::lkdyn::lkrt_dyn_from_i64(*v),
        other => {
            let text = alloc::ffi::CString::new(key_str(other)).unwrap_or_default();
            crate::lkdyn::lkrt_dyn_from_str(crate::lkstr::arena_c_string(text))
        }
    }
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
            _ => 3,
        }
    }
    // Reached only for two members of the same kind, so the string arm is the
    // one place `key_str` is called — and there both sides are strings.
    kind(a).cmp(&kind(b)).then_with(|| match (a, b) {
        (RtKey::Bool(x), RtKey::Bool(y)) => x.cmp(y),
        (RtKey::Int(x), RtKey::Int(y)) => x.cmp(y),
        _ if kind(a) == 3 => key_str(a).cmp(key_str(b)),
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
        _ => format!("{:?}", key_str(key)),
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

    /// The order-conformance check that the single `RtKey` makes possible: a
    /// set iterates in exactly the order the VM's `FastHashSet<RuntimeMapKey>`
    /// does, for the same members inserted in the same sequence.
    ///
    /// This is what the module could not say while it kept its own key type —
    /// membership agreed and the hash did not, so `for x in s` was simply left
    /// out of the native subset rather than being wrong.
    #[test]
    fn set_iteration_order_matches_the_vm() {
        use lk_core::val::{MirrorMember, set_iteration_order};

        let cases: alloc::vec::Vec<alloc::vec::Vec<MirrorMember>> = vec![
            (0..64).map(|i| MirrorMember::Int(i * 3 - 7)).collect(),
            // Short (inline) and long (heap) keys mixed: the two shapes hash
            // differently, which is the whole reason one key type is required.
            (0..48)
                .map(|i| {
                    if i % 3 == 0 {
                        MirrorMember::Str(alloc::format!("member_number_{i}"))
                    } else {
                        MirrorMember::Str(alloc::format!("m{i}"))
                    }
                })
                .collect(),
        ];
        for members in cases {
            let vm_order = set_iteration_order(members.iter().cloned());

            let handle = lkrt_lkset_new();
            for member in &members {
                let boxed = match member {
                    MirrorMember::Int(v) => crate::lkdyn::lkrt_dyn_from_i64(*v),
                    MirrorMember::Str(v) => s(v),
                };
                unsafe { lkrt_lkset_add(handle, boxed) };
            }
            // SAFETY: just built above.
            let native = unsafe { &*(handle as *mut LkSet) };
            let native_order: alloc::vec::Vec<MirrorMember> = native
                .iter()
                .map(|k| match k {
                    RtKey::Int(v) => MirrorMember::Int(*v),
                    other => MirrorMember::Str(key_str(other).to_string()),
                })
                .collect();

            assert_eq!(
                native_order, vm_order,
                "set iteration order drifted from the VM's RuntimeSet"
            );
        }
    }
}
