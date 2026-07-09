//! `LkDyn` — the boxed dynamic value for natively-lowered mixed-type code
//! (plan M4.2 "deep coverage"). A tagged 2-word carrier passed **by value**
//! across the ABI (LLVM `{ i64, i64 }`, same shape as the `LkMaybe*`
//! carriers): scalars box with zero allocation, `Str`/list payloads are the
//! existing arena pointers reinterpreted as `i64`.
//!
//! Semantics contract: every operation here must match the VM (the
//! differential gates compare stdout byte-for-byte). Type errors are the
//! VM's loud failures — `flush_and_abort()` (the contract compares only
//! `success()` + stdout, not stderr text).

use core::ffi::{c_char, c_void};
use std::ffi::{CStr, CString};

use crate::lkstr::arena_c_string;
use crate::state::arena_handle;

pub const DYN_NIL: i64 = 0;
pub const DYN_BOOL: i64 = 1;
pub const DYN_I64: i64 = 2;
pub const DYN_F64: i64 = 3;
pub const DYN_STR: i64 = 4;
pub const DYN_LIST: i64 = 5;
pub const DYN_MAP: i64 = 6;

/// The by-value dynamic carrier. `payload` holds the value bits: `0`/`1` for
/// Bool, the integer itself for I64, `f64::to_bits` for F64, a `*const
/// c_char` for Str, a list handle (`*mut c_void`) for List — both pointers
/// arena-owned like every other lkrt allocation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LkDyn {
    pub tag: i64,
    pub payload: i64,
}

impl LkDyn {
    pub const NIL: LkDyn = LkDyn {
        tag: DYN_NIL,
        payload: 0,
    };

    fn f64_value(self) -> f64 {
        f64::from_bits(self.payload as u64)
    }

    /// Numeric view for mixed-type arithmetic/compares; `None` when not
    /// numeric.
    fn as_numeric(self) -> Option<Numeric> {
        match self.tag {
            DYN_I64 => Some(Numeric::Int(self.payload)),
            DYN_F64 => Some(Numeric::Float(self.f64_value())),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Numeric {
    Int(i64),
    Float(f64),
}

impl Numeric {
    fn as_f64(self) -> f64 {
        match self {
            Numeric::Int(v) => v as f64,
            Numeric::Float(v) => v,
        }
    }
}

fn from_f64(x: f64) -> LkDyn {
    LkDyn {
        tag: DYN_F64,
        payload: x.to_bits() as i64,
    }
}

fn from_i64(v: i64) -> LkDyn {
    LkDyn {
        tag: DYN_I64,
        payload: v,
    }
}

unsafe fn dyn_str<'a>(v: LkDyn) -> &'a str {
    let ptr = v.payload as *const c_char;
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

fn dyn_list<'a>(v: LkDyn) -> &'a [LkDyn] {
    let handle = v.payload as *mut c_void;
    if handle.is_null() {
        return &[];
    }
    unsafe { &*(handle as *mut Vec<LkDyn>) }
}

// ── Boxing ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_nil() -> LkDyn {
    LkDyn::NIL
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_bool(v: i64) -> LkDyn {
    LkDyn {
        tag: DYN_BOOL,
        payload: i64::from(v != 0),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_i64(v: i64) -> LkDyn {
    from_i64(v)
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_f64(v: f64) -> LkDyn {
    from_f64(v)
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_str(s: *const c_char) -> LkDyn {
    LkDyn {
        tag: DYN_STR,
        payload: s as i64,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_list(handle: *mut c_void) -> LkDyn {
    LkDyn {
        tag: DYN_LIST,
        payload: handle as i64,
    }
}

// Nullable-carrier boxing: the lowering passes the `Maybe` struct's two words
// (`value`, `present`) separately so the ABI stays within the scalar
// vocabulary. Absent boxes nil (payload zeroed — identical to `from_nil`).

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_maybe_i64(value: i64, present: i64) -> LkDyn {
    if present != 0 { from_i64(value) } else { LkDyn::NIL }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_maybe_f64(value: f64, present: i64) -> LkDyn {
    if present != 0 { from_f64(value) } else { LkDyn::NIL }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_maybe_str(value: *const c_char, present: i64) -> LkDyn {
    if present != 0 {
        LkDyn {
            tag: DYN_STR,
            payload: value as i64,
        }
    } else {
        LkDyn::NIL
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_maybe_bool(value: i64, present: i64) -> LkDyn {
    if present != 0 {
        LkDyn {
            tag: DYN_BOOL,
            payload: i64::from(value != 0),
        }
    } else {
        LkDyn::NIL
    }
}

// ── Guarded unboxing (VM loud failure on tag mismatch) ─────────────────

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_tag(v: LkDyn) -> i64 {
    v.tag
}

/// VM truthiness (`truthy_unchecked`): only nil and `false` are falsy —
/// every number (including 0), string, and container is truthy.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_truthy(v: LkDyn) -> i64 {
    i64::from(!(v.tag == DYN_NIL || (v.tag == DYN_BOOL && v.payload == 0)))
}

/// `!x`: a Bool negates, Nil is `true`, anything else is the VM's loud
/// type error.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_not(v: LkDyn) -> i64 {
    match v.tag {
        DYN_NIL => 1,
        DYN_BOOL => i64::from(v.payload == 0),
        _ => crate::panic::raise_str("runtime type error"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_i64(v: LkDyn) -> i64 {
    if v.tag != DYN_I64 {
        crate::panic::raise_str("runtime type error");
    }
    v.payload
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_f64(v: LkDyn) -> f64 {
    match v.tag {
        DYN_F64 => v.f64_value(),
        DYN_I64 => v.payload as f64,
        _ => crate::panic::raise_str("runtime type error"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_str(v: LkDyn) -> *const c_char {
    if v.tag != DYN_STR {
        crate::panic::raise_str("runtime type error");
    }
    v.payload as *const c_char
}

/// Unboxes a map handle; a non-map tag is the VM's loud type error.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_map(v: LkDyn) -> *mut c_void {
    if v.tag != DYN_MAP {
        crate::panic::raise_str("runtime type error");
    }
    v.payload as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_bool(v: LkDyn) -> i64 {
    if v.tag != DYN_BOOL {
        crate::panic::raise_str("runtime type error");
    }
    v.payload
}

// ── Trait-method dispatch marks (plan J1) ──────────────────────────────
//
// A struct instance is carried as a plain string-keyed map (no hidden
// "$type" key — `len`/iteration/display stay exact); its *runtime* type
// identity lives in a side registry keyed by the arena handle. Handles are
// never freed before process exit, so a mark can't dangle or alias.

std::thread_local! {
    static OBJ_TYPE_MARKS: core::cell::RefCell<crate::lkmap::FxMap<usize, i64>> =
        core::cell::RefCell::new(crate::lkmap::FxMap::default());
}

/// Marks a freshly built struct-instance map with its lowering-assigned
/// type id (`NewObject` of a type that has trait impls).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_obj_mark(handle: *mut c_void, type_id: i64) {
    OBJ_TYPE_MARKS.with(|marks| marks.borrow_mut().insert(handle as usize, type_id));
}

/// Reads a boxed value's struct type mark; `0` = unmarked (not a struct
/// instance, or a type with no trait impls).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_obj_type_id(v: LkDyn) -> i64 {
    if v.tag != DYN_MAP {
        return 0;
    }
    OBJ_TYPE_MARKS.with(|marks| marks.borrow().get(&(v.payload as usize)).copied().unwrap_or(0))
}

/// Dispatch fall-through: no registered impl matched the receiver's mark —
/// the VM's unknown-method error is a catchable raise.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_method_missing() {
    crate::panic::raise_str("runtime type error");
}

// ── Arithmetic (VM promotion rules; type errors are loud failures) ─────

/// # Safety
/// Str payloads must be live NUL-terminated strings (arena or interned).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_add(a: LkDyn, b: LkDyn) -> LkDyn {
    if a.tag == DYN_STR && b.tag == DYN_STR {
        let joined = format!("{}{}", unsafe { dyn_str(a) }, unsafe { dyn_str(b) });
        let ptr = arena_c_string(CString::new(joined).unwrap_or_default());
        return LkDyn {
            tag: DYN_STR,
            payload: ptr as i64,
        };
    }
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_add(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() + y.as_f64()),
        _ => crate::panic::raise_str("runtime type error"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_sub(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_sub(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() - y.as_f64()),
        _ => crate::panic::raise_str("runtime type error"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_mul(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_mul(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() * y.as_f64()),
        _ => crate::panic::raise_str("runtime type error"),
    }
}

/// `/` always produces Float in LK (docs/semantics.md 数值), zero divisor is
/// a loud failure.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_div(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(x), Some(y)) => {
            let rhs = y.as_f64();
            if rhs == 0.0 {
                crate::panic::raise_str("runtime type error");
            }
            from_f64(x.as_f64() / rhs)
        }
        _ => crate::panic::raise_str("runtime type error"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_mod(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => {
            if y == 0 {
                crate::panic::raise_str("runtime type error");
            }
            from_i64(x.wrapping_rem(y))
        }
        (Some(x), Some(y)) => {
            let rhs = y.as_f64();
            if rhs == 0.0 {
                crate::panic::raise_str("runtime type error");
            }
            from_f64(x.as_f64() % rhs)
        }
        _ => crate::panic::raise_str("runtime type error"),
    }
}

// ── Equality / ordering ────────────────────────────────────────────────

/// VM equality: Int/Float compare numerically across tags (`1 == 1.0`),
/// strings by content, lists elementwise; distinct non-numeric tags are
/// simply unequal (not an error).
/// # Safety
/// Str/list payloads must be live arena pointers, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_eq(a: LkDyn, b: LkDyn) -> i64 {
    i64::from(dyn_eq_inner(a, b))
}

fn dyn_eq_inner(a: LkDyn, b: LkDyn) -> bool {
    if let (Some(x), Some(y)) = (a.as_numeric(), b.as_numeric()) {
        return match (x, y) {
            (Numeric::Int(x), Numeric::Int(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        };
    }
    if a.tag != b.tag {
        return false;
    }
    match a.tag {
        DYN_NIL => true,
        DYN_BOOL => a.payload == b.payload,
        DYN_STR => unsafe { dyn_str(a) == dyn_str(b) },
        DYN_LIST => {
            let (xs, ys) = (dyn_list(a), dyn_list(b));
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(&x, &y)| dyn_eq_inner(x, y))
        }
        DYN_MAP => {
            if (a.payload as *mut c_void).is_null() || (b.payload as *mut c_void).is_null() {
                return a.payload == b.payload;
            }
            let (xs, ys) = (dyn_map(a), dyn_map(b));
            // Structural, order-free (hash iteration order is not portable,
            // but key-lookup equality is).
            xs.len() == ys.len() && xs.iter().all(|(k, &v)| ys.get(k).is_some_and(|&w| dyn_eq_inner(v, w)))
        }
        _ => false,
    }
}

macro_rules! dyn_ord {
    ($name:ident, $op:tt) => {
        /// # Safety
        /// Str payloads must be live NUL-terminated strings.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(a: LkDyn, b: LkDyn) -> i64 {
            // Two strings order lexicographically (the VM's
            // `number_compare_value` string arm — Rust byte order, which is
            // code-point order for UTF-8); mixed string/number is its error.
            if a.tag == DYN_STR && b.tag == DYN_STR {
                return i64::from(unsafe { dyn_str(a) } $op unsafe { dyn_str(b) });
            }
            match (a.as_numeric(), b.as_numeric()) {
                (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => i64::from(x $op y),
                (Some(x), Some(y)) => i64::from(x.as_f64() $op y.as_f64()),
                _ => crate::panic::raise_str("runtime type error"),
            }
        }
    };
}
dyn_ord!(lkrt_dyn_lt, <);
dyn_ord!(lkrt_dyn_le, <=);
dyn_ord!(lkrt_dyn_gt, >);
dyn_ord!(lkrt_dyn_ge, >=);

// ── Display (two modes, matching the VM's two display paths) ───────────

/// Diagnostics rendering for the uncaught-error path (`panic.rs`): the plain
/// display with raising disabled *recursively* — an unknown tag anywhere in a
/// nested container renders a placeholder instead of re-entering raise while
/// an uncaught error is already being reported.
pub(crate) fn display_for_diagnostics(v: LkDyn) -> String {
    let mut out = String::new();
    display_into_impl(&mut out, v, false, false);
    out
}

fn display_into(out: &mut String, v: LkDyn, quoted: bool) {
    display_into_impl(out, v, quoted, true)
}

fn display_into_impl(out: &mut String, v: LkDyn, quoted: bool, raise_on_unknown: bool) {
    match v.tag {
        DYN_NIL => out.push_str("nil"),
        DYN_BOOL => out.push_str(if v.payload != 0 { "true" } else { "false" }),
        DYN_I64 => {
            let mut digits = [0u8; 20];
            out.push_str(core::str::from_utf8(crate::lkstr::i64_decimal(v.payload, &mut digits)).unwrap_or("0"));
        }
        DYN_F64 => out.push_str(&v.f64_value().to_string()),
        DYN_STR => {
            let s = unsafe { dyn_str(v) };
            if quoted {
                // Rust `{:?}` quoting/escaping — the VM's in-list string format.
                out.push_str(&format!("{s:?}"));
            } else {
                out.push_str(s);
            }
        }
        DYN_LIST => {
            // VM quirk pinned by the differential gate: *mixed* lists render
            // their string elements bare (`[1,a b,2]`), unlike typed string
            // lists (`["a","b c"]` via the `{:?}` path). VM is the reference.
            out.push('[');
            for (i, &e) in dyn_list(v).iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                display_into_impl(out, e, false, raise_on_unknown);
            }
            out.push(']');
        }
        DYN_MAP => {
            // VM format: quoted keys, bare values (`{"k":1,"s":txt}`). The
            // entry order is the Fx layout order — the mirror discipline
            // (vm_mirror + insert-order replay) makes it the VM's own order,
            // for bridged returns and mirror-built maps alike. Statically
            // typed map display stays *out of the lowering subset*
            // (docs/semantics.md): this arm only serves runtime-tagged Dyn
            // values, where the alternative would be a raise the VM does not
            // have.
            out.push('{');
            if !(v.payload as *mut c_void).is_null() {
                for (i, (k, &e)) in dyn_map(v).iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!("{k:?}"));
                    out.push(':');
                    display_into_impl(out, e, false, raise_on_unknown);
                }
            }
            out.push('}');
        }
        other => {
            if raise_on_unknown {
                crate::panic::raise_str("runtime type error");
            }
            out.push_str(&format!("<unrenderable value, tag {other}>"));
        }
    }
}

/// Bare display: strings render as-is (print/template scalar path).
/// # Safety
/// Str/list payloads must be live arena pointers, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_display(v: LkDyn) -> *mut c_char {
    let mut out = String::new();
    display_into(&mut out, v, false);
    arena_c_string(CString::new(out).unwrap_or_default())
}

/// Quoted display: strings render Rust-`{:?}`-style (in-list element path).
/// # Safety
/// Str/list payloads must be live arena pointers, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_display_quoted(v: LkDyn) -> *mut c_char {
    let mut out = String::new();
    display_into(&mut out, v, true);
    arena_c_string(CString::new(out).unwrap_or_default())
}

/// `len` of a Dyn by runtime tag: list length, map entry count, string
/// Unicode scalar count; scalars are the VM's loud failure.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_len_of(v: LkDyn) -> i64 {
    match v.tag {
        DYN_LIST => dyn_list(v).len() as i64,
        DYN_MAP => {
            if (v.payload as *mut c_void).is_null() {
                0
            } else {
                dyn_map(v).len() as i64
            }
        }
        DYN_STR => unsafe { dyn_str(v) }.chars().count() as i64,
        _ => crate::panic::raise_str("runtime type error"),
    }
}

/// Guarded list unboxing: the handle behind a `DYN_LIST` tag (loud failure
/// otherwise — iterating a non-container is a VM error).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_list(v: LkDyn) -> *mut c_void {
    if v.tag != DYN_LIST {
        crate::panic::raise_str("runtime type error");
    }
    v.payload as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_map(handle: *mut c_void) -> LkDyn {
    LkDyn {
        tag: DYN_MAP,
        payload: handle as i64,
    }
}

fn dyn_map<'a>(v: LkDyn) -> &'a crate::lkmap::StrDynMap {
    let handle = v.payload as *mut c_void;
    debug_assert!(!handle.is_null());
    unsafe { &*(handle as *mut crate::lkmap::StrDynMap) }
}

/// Constant-string field read on a Dyn: a Map tag looks the key up (missing
/// key → Nil, the VM's nil-on-missing); any non-map tag is the VM's loud
/// failure on member access.
///
/// # Safety
/// `key` must be a NUL-terminated string; a Map payload must be a live
/// `map_h str_dyn` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_field(v: LkDyn, key: *const c_char) -> LkDyn {
    if v.tag != DYN_MAP || (v.payload as *mut c_void).is_null() {
        crate::panic::raise_str("runtime type error");
    }
    let key = if key.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(key) }.to_str().unwrap_or("")
    };
    dyn_map(v).get(key).copied().unwrap_or(LkDyn::NIL)
}

/// Index into a Dyn: a List tag indexes like `lkrt_lklist_dyn_at`
/// (negative-from-tail, OOB → Nil); any non-container tag is the VM's
/// "index on a non-container" loud failure.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_index(v: LkDyn, index: i64) -> LkDyn {
    if v.tag != DYN_LIST {
        crate::panic::raise_str("runtime type error");
    }
    let values = dyn_list(v);
    let len = values.len() as i64;
    let idx = if index < 0 { len + index } else { index };
    if idx < 0 || idx >= len {
        return LkDyn::NIL;
    }
    values[idx as usize]
}

/// Converts a typed `List<i64>` handle into a fresh dyn-list handle (each
/// element boxed). Cold-path only — emitted when a typed list meets a Dyn
/// in a comparison or a mixed construction.
///
/// # Safety
/// `handle` must be a live `List<i64>` handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_i64_to_dyn(handle: *mut c_void) -> *mut c_void {
    let values: &[i64] = if handle.is_null() {
        &[]
    } else {
        unsafe { &*(handle as *mut Vec<i64>) }
    };
    arena_handle(values.iter().map(|&v| from_i64(v)).collect::<Vec<LkDyn>>())
}

/// The `f64` analogue of [`lkrt_lklist_i64_to_dyn`].
///
/// # Safety
/// `handle` must be a live `List<f64>` handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_f64_to_dyn(handle: *mut c_void) -> *mut c_void {
    let values: &[f64] = if handle.is_null() {
        &[]
    } else {
        unsafe { &*(handle as *mut Vec<f64>) }
    };
    arena_handle(values.iter().map(|&v| from_f64(v)).collect::<Vec<LkDyn>>())
}

/// The `str` analogue of [`lkrt_lklist_i64_to_dyn`] (element pointers are
/// shared, arena-owned).
///
/// # Safety
/// `handle` must be a live `List<str>` handle, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_str_to_dyn(handle: *mut c_void) -> *mut c_void {
    let values: &[*const c_char] = if handle.is_null() {
        &[]
    } else {
        unsafe { &*(handle as *mut Vec<*const c_char>) }
    };
    arena_handle(values.iter().map(|&p| lkrt_dyn_from_str(p)).collect::<Vec<LkDyn>>())
}

// ── Mixed list (`Box<Vec<LkDyn>>` behind the usual arena handle) ───────

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lklist_dyn_new() -> *mut c_void {
    arena_handle(Vec::<LkDyn>::new())
}

/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_push(handle: *mut c_void, value: LkDyn) {
    if handle.is_null() {
        return;
    }
    unsafe { (*(handle as *mut Vec<LkDyn>)).push(value) };
}

/// VM indexing semantics: negative counts from the tail, out-of-bounds reads
/// yield nil (not an error) — the Dyn carrier holds the nil itself.
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_at(handle: *mut c_void, index: i64) -> LkDyn {
    if handle.is_null() {
        return LkDyn::NIL;
    }
    let values = unsafe { &*(handle as *mut Vec<LkDyn>) };
    let len = values.len() as i64;
    let idx = if index < 0 { len + index } else { index };
    if idx < 0 || idx >= len {
        return LkDyn::NIL;
    }
    values[idx as usize]
}

/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_set(handle: *mut c_void, index: i64, value: LkDyn) {
    if handle.is_null() {
        return;
    }
    let values = unsafe { &mut *(handle as *mut Vec<LkDyn>) };
    let len = values.len() as i64;
    let idx = if index < 0 { len + index } else { index };
    if idx < 0 {
        return;
    }
    let idx = idx as usize;
    if idx >= values.len() {
        values.resize(idx + 1, LkDyn::NIL);
    }
    values[idx] = value;
}

/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_len(handle: *mut c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    unsafe { &*(handle as *mut Vec<LkDyn>) }.len() as i64
}

/// # Safety
/// Both handles must be live handles from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_eq(a: *mut c_void, b: *mut c_void) -> i64 {
    let lhs: &[LkDyn] = if a.is_null() {
        &[]
    } else {
        unsafe { &*(a as *mut Vec<LkDyn>) }
    };
    let rhs: &[LkDyn] = if b.is_null() {
        &[]
    } else {
        unsafe { &*(b as *mut Vec<LkDyn>) }
    };
    i64::from(lhs.len() == rhs.len() && lhs.iter().zip(rhs).all(|(&x, &y)| dyn_eq_inner(x, y)))
}

/// The VM's `Contains` (`in`) equality on a Mixed list is `RuntimeVal`'s
/// *derived* `PartialEq` — strictly same-variant: no Int/Float coercion
/// (`1.0 in [1, 2]` is false, unlike `==`), floats by value (`0.0 == -0.0`,
/// `NaN != NaN`, unlike `unique()`'s to_bits), ShortStr (≤7 bytes) by
/// content, heap objects (lists/maps/longer strings) by handle.
fn contains_eq(a: LkDyn, b: LkDyn) -> bool {
    if a.tag != b.tag {
        return false;
    }
    match a.tag {
        DYN_NIL => true,
        DYN_BOOL | DYN_I64 => a.payload == b.payload,
        DYN_F64 => a.f64_value() == b.f64_value(),
        DYN_STR => {
            let (sa, sb) = unsafe { (dyn_str(a), dyn_str(b)) };
            if sa.len() <= 7 && sb.len() <= 7 {
                sa == sb
            } else {
                a.payload == b.payload
            }
        }
        DYN_LIST | DYN_MAP => a.payload == b.payload,
        _ => false,
    }
}

/// `needle in xs` under [`contains_eq`] (the `in` operator's semantics —
/// *not* `dyn_eq_inner`, which is the `==` operator's).
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_contains(handle: *mut c_void, value: LkDyn) -> i64 {
    if handle.is_null() {
        return 0;
    }
    let values = unsafe { &*(handle as *mut Vec<LkDyn>) };
    i64::from(values.iter().any(|&e| contains_eq(e, value)))
}

fn dyn_slice<'a>(handle: *mut c_void) -> &'a [LkDyn] {
    if handle.is_null() {
        &[]
    } else {
        unsafe { &*(handle as *mut Vec<LkDyn>) }
    }
}

/// `xs[start..]` over a mixed list (the VM's `slice_from`): negative
/// `start` aborts, `start >= len` yields a fresh empty list.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_slice_from(handle: *mut c_void, start: i64) -> *mut c_void {
    if start < 0 {
        crate::panic::raise_str("runtime type error");
    }
    let tail: Vec<LkDyn> = dyn_slice(handle).iter().copied().skip(start as usize).collect();
    arena_handle(tail)
}

/// `xs.take(n)` — the first `n` elements (mirrors `lkrt_lklist_i64_take`).
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_take(handle: *mut c_void, n: i64) -> *mut c_void {
    let values = dyn_slice(handle);
    let count = (n as usize).min(values.len());
    arena_handle(values[..count].to_vec())
}

/// `xs.skip(n)` — without the first `n` (zero/negative copies everything).
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_skip(handle: *mut c_void, n: i64) -> *mut c_void {
    let values = dyn_slice(handle);
    let start = if n > 0 { (n as usize).min(values.len()) } else { 0 };
    arena_handle(values[start..].to_vec())
}

/// `xs.chain(ys)` / `xs.concat(ys)` — a fresh concatenation.
/// # Safety
/// Both handles must be live dyn-list handles, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_chain(a: *mut c_void, b: *mut c_void) -> *mut c_void {
    let lhs = dyn_slice(a);
    let rhs = dyn_slice(b);
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    out.extend_from_slice(lhs);
    out.extend_from_slice(rhs);
    arena_handle(out)
}

/// `xs.map(f)` over boxed elements (`fn(LkDyn) -> LkDyn` callback).
/// # Safety
/// `handle` must be a live dyn-list handle (or null); `f` a compiled lambda.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_map_fn(handle: *mut c_void, f: extern "C" fn(LkDyn) -> LkDyn) -> *mut c_void {
    let mapped: Vec<LkDyn> = dyn_slice(handle).iter().map(|&v| f(v)).collect();
    arena_handle(mapped)
}

/// `xs.filter(p)` over boxed elements (`fn(LkDyn) -> bool` callback).
/// # Safety
/// `handle` must be a live dyn-list handle (or null); `p` a compiled lambda.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_filter_fn(
    handle: *mut c_void,
    p: extern "C" fn(LkDyn) -> bool,
) -> *mut c_void {
    let kept: Vec<LkDyn> = dyn_slice(handle).iter().copied().filter(|&v| p(v)).collect();
    arena_handle(kept)
}

/// `xs.reduce(init, f)` over boxed elements (`fn(acc, x) -> LkDyn` callback).
/// # Safety
/// `handle` must be a live dyn-list handle (or null); `f` a compiled lambda.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_reduce_fn(
    handle: *mut c_void,
    init: LkDyn,
    f: extern "C" fn(LkDyn, LkDyn) -> LkDyn,
) -> LkDyn {
    dyn_slice(handle).iter().fold(init, |acc, &v| f(acc, v))
}

/// `xs.chunk(size)` — split into `size`-element groups, last group short.
/// `size <= 0` is a VM error (loud failure).
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_chunk(handle: *mut c_void, size: i64) -> *mut c_void {
    if size <= 0 {
        eprintln!("list.chunk() size must be positive");
        crate::panic::raise_str("runtime type error");
    }
    let chunks: Vec<LkDyn> = dyn_slice(handle)
        .chunks(size as usize)
        .map(|chunk| lkrt_dyn_from_list(arena_handle(chunk.to_vec())))
        .collect();
    arena_handle(chunks)
}

/// `xs.enumerate()` — `[[0, x0], [1, x1], …]` pairs.
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_enumerate(handle: *mut c_void) -> *mut c_void {
    let pairs: Vec<LkDyn> = dyn_slice(handle)
        .iter()
        .enumerate()
        .map(|(i, &v)| lkrt_dyn_from_list(arena_handle(vec![from_i64(i as i64), v])))
        .collect();
    arena_handle(pairs)
}

/// `xs.zip(ys)` — `[[a0, b0], …]`, truncated to the shorter side.
/// # Safety
/// Both handles must be live handles from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_zip(a: *mut c_void, b: *mut c_void) -> *mut c_void {
    let pairs: Vec<LkDyn> = dyn_slice(a)
        .iter()
        .zip(dyn_slice(b))
        .map(|(&x, &y)| lkrt_dyn_from_list(arena_handle(vec![x, y])))
        .collect();
    arena_handle(pairs)
}

/// The VM's `runtime_values_equal` (core_methods.rs) — used by `unique()`,
/// and deliberately *not* `dyn_eq_inner`: numerics compare by `to_bits`
/// (`0.0 != -0.0`, `1 == 1.0`), strings compare by content only when both
/// fit the VM's 7-byte `ShortStr` inline form (longer strings are heap
/// objects there and compare by handle), lists/maps compare by handle.
fn unique_eq(a: LkDyn, b: LkDyn) -> bool {
    match (a.tag, b.tag) {
        (DYN_NIL, DYN_NIL) => true,
        (DYN_BOOL, DYN_BOOL) | (DYN_I64, DYN_I64) | (DYN_F64, DYN_F64) => a.payload == b.payload,
        (DYN_I64, DYN_F64) => (a.payload as f64).to_bits() == b.payload as u64,
        (DYN_F64, DYN_I64) => a.payload as u64 == (b.payload as f64).to_bits(),
        (DYN_STR, DYN_STR) => {
            // Longer strings are heap objects in the VM with no stable
            // identity across list representations (typed String lists
            // re-alloc every element on read, so `[s, s].unique()` keeps
            // both) — and native constants intern, so pointer identity
            // over-merges literals. "Never equal" matches the VM on every
            // shape except a Mixed-list variable repeat (docs/semantics.md).
            let (sa, sb) = unsafe { (dyn_str(a), dyn_str(b)) };
            sa.len() <= 7 && sb.len() <= 7 && sa == sb
        }
        (DYN_LIST, DYN_LIST) | (DYN_MAP, DYN_MAP) => a.payload == b.payload,
        _ => false,
    }
}

/// `xs.unique()` — order-preserving dedup under [`unique_eq`]. O(n²) like
/// the VM.
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_unique(handle: *mut c_void) -> *mut c_void {
    let mut unique: Vec<LkDyn> = Vec::new();
    for &item in dyn_slice(handle) {
        if !unique.iter().any(|&seen| unique_eq(seen, item)) {
            unique.push(item);
        }
    }
    arena_handle(unique)
}

/// `xs.flatten()` — one level: list elements splice, everything else passes
/// through unchanged.
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_flatten(handle: *mut c_void) -> *mut c_void {
    let mut flat: Vec<LkDyn> = Vec::new();
    for &item in dyn_slice(handle) {
        if item.tag == DYN_LIST {
            flat.extend_from_slice(dyn_list(item));
        } else {
            flat.push(item);
        }
    }
    arena_handle(flat)
}

/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_display(handle: *mut c_void) -> *mut c_char {
    let dyn_v = LkDyn {
        tag: DYN_LIST,
        payload: handle as i64,
    };
    let mut out = String::new();
    display_into(&mut out, dyn_v, true);
    arena_c_string(CString::new(out).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> LkDyn {
        let ptr = arena_c_string(CString::new(text).unwrap());
        lkrt_dyn_from_str(ptr)
    }

    fn text(ptr: *mut c_char) -> String {
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }

    #[test]
    fn from_maybe_boxes_present_and_nil() {
        let present = lkrt_dyn_from_maybe_i64(7, 1);
        assert_eq!((present.tag, present.payload), (DYN_I64, 7));
        let absent = lkrt_dyn_from_maybe_i64(7, 0);
        assert_eq!((absent.tag, absent.payload), (DYN_NIL, 0), "absent == from_nil");
        let f = lkrt_dyn_from_maybe_f64(1.5, 1);
        assert_eq!(f.tag, DYN_F64);
        assert_eq!(f.f64_value(), 1.5);
        assert_eq!(lkrt_dyn_from_maybe_str(std::ptr::null(), 0).tag, DYN_NIL);
        let b = lkrt_dyn_from_maybe_bool(1, 1);
        assert_eq!((b.tag, b.payload), (DYN_BOOL, 1));
    }

    #[test]
    fn truthy_matches_vm_semantics() {
        // Only nil and false are falsy; 0/0.0/"" are truthy.
        assert_eq!(lkrt_dyn_truthy(lkrt_dyn_from_nil()), 0);
        assert_eq!(lkrt_dyn_truthy(lkrt_dyn_from_bool(0)), 0);
        assert_eq!(lkrt_dyn_truthy(lkrt_dyn_from_bool(1)), 1);
        assert_eq!(lkrt_dyn_truthy(lkrt_dyn_from_i64(0)), 1);
        assert_eq!(lkrt_dyn_truthy(lkrt_dyn_from_f64(0.0)), 1);
        assert_eq!(lkrt_dyn_truthy(s("")), 1);
    }

    #[test]
    fn arithmetic_follows_vm_promotion() {
        let add = unsafe { lkrt_dyn_add(lkrt_dyn_from_i64(2), lkrt_dyn_from_i64(3)) };
        assert_eq!((add.tag, add.payload), (DYN_I64, 5));
        let mixed = unsafe { lkrt_dyn_add(lkrt_dyn_from_i64(2), lkrt_dyn_from_f64(0.5)) };
        assert_eq!(mixed.tag, DYN_F64);
        assert_eq!(mixed.f64_value(), 2.5);
        // `/` always yields Float (semantics.md 数值).
        let div = lkrt_dyn_div(lkrt_dyn_from_i64(20), lkrt_dyn_from_i64(4));
        assert_eq!(div.tag, DYN_F64);
        assert_eq!(div.f64_value(), 5.0);
        let cat = unsafe { lkrt_dyn_add(s("foo"), s("bar")) };
        assert_eq!(cat.tag, DYN_STR);
        assert_eq!(text(cat.payload as *mut c_char), "foobar");
    }

    #[test]
    fn equality_is_numeric_across_tags_and_structural_for_lists() {
        assert_eq!(unsafe { lkrt_dyn_eq(lkrt_dyn_from_i64(1), lkrt_dyn_from_f64(1.0)) }, 1);
        assert_eq!(unsafe { lkrt_dyn_eq(lkrt_dyn_from_i64(1), s("1")) }, 0);
        assert_eq!(unsafe { lkrt_dyn_eq(s("a"), s("a")) }, 1);
        assert_eq!(unsafe { lkrt_dyn_eq(lkrt_dyn_from_nil(), lkrt_dyn_from_nil()) }, 1);
        let xs = lkrt_lklist_dyn_new();
        let ys = lkrt_lklist_dyn_new();
        unsafe {
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_i64(1));
            lkrt_lklist_dyn_push(xs, s("a"));
            lkrt_lklist_dyn_push(ys, lkrt_dyn_from_f64(1.0));
            lkrt_lklist_dyn_push(ys, s("a"));
        }
        assert_eq!(unsafe { lkrt_lklist_dyn_eq(xs, ys) }, 1);
    }

    #[test]
    fn display_matches_vm_list_format() {
        let xs = lkrt_lklist_dyn_new();
        unsafe {
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_i64(1));
            lkrt_lklist_dyn_push(xs, s("b c"));
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_f64(2.0));
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_bool(1));
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_nil());
        }
        // Comma-separated no spaces; strings {:?}-quoted; 2.0 → "2" (Rust
        // to_string); bare-vs-quoted only differs for strings.
        // Mixed lists render string elements bare (VM's Mixed-list path).
        assert_eq!(text(unsafe { lkrt_lklist_dyn_display(xs) }), "[1,b c,2,true,nil]");
        assert_eq!(text(unsafe { lkrt_dyn_display(s("b c")) }), "b c");
        assert_eq!(text(unsafe { lkrt_dyn_display_quoted(s("b c")) }), "\"b c\"");
    }

    #[test]
    fn unique_eq_is_vm_handle_semantics() {
        // Numerics by to_bits: 1 == 1.0 dedups, 0.0 vs -0.0 does not.
        assert!(unique_eq(lkrt_dyn_from_i64(1), lkrt_dyn_from_f64(1.0)));
        assert!(!unique_eq(lkrt_dyn_from_f64(0.0), lkrt_dyn_from_f64(-0.0)));
        // ShortStr (≤7 bytes) by content; longer strings never dedup
        // (docs/semantics.md unique() 裁决).
        assert!(unique_eq(s("ab"), s("ab")));
        assert!(!unique_eq(s("longer-than-seven"), s("longer-than-seven")));
        // Lists by handle, not structure.
        let xs = lkrt_lklist_dyn_new();
        let ys = lkrt_lklist_dyn_new();
        unsafe {
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_i64(7));
            lkrt_lklist_dyn_push(ys, lkrt_dyn_from_i64(7));
        }
        assert!(unique_eq(lkrt_dyn_from_list(xs), lkrt_dyn_from_list(xs)));
        assert!(!unique_eq(lkrt_dyn_from_list(xs), lkrt_dyn_from_list(ys)));
        // The chunk/enumerate/zip/flatten family (VM core_methods shapes).
        let src = lkrt_lklist_dyn_new();
        unsafe {
            for v in [1, 2, 3] {
                lkrt_lklist_dyn_push(src, lkrt_dyn_from_i64(v));
            }
            let chunks = lkrt_lklist_dyn_chunk(src, 2);
            assert_eq!(text(lkrt_lklist_dyn_display(chunks)), "[[1,2],[3]]");
            let pairs = lkrt_lklist_dyn_enumerate(src);
            assert_eq!(text(lkrt_lklist_dyn_display(pairs)), "[[0,1],[1,2],[2,3]]");
            let zipped = lkrt_lklist_dyn_zip(src, chunks);
            assert_eq!(text(lkrt_lklist_dyn_display(zipped)), "[[1,[1,2]],[2,[3]]]");
            let flat = lkrt_lklist_dyn_flatten(zipped);
            assert_eq!(text(lkrt_lklist_dyn_display(flat)), "[1,[1,2],2,[3]]");
        }
    }

    #[test]
    fn indexing_is_vm_shaped() {
        let xs = lkrt_lklist_dyn_new();
        unsafe {
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_i64(10));
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_i64(20));
        }
        assert_eq!(unsafe { lkrt_lklist_dyn_at(xs, 1) }.payload, 20);
        assert_eq!(unsafe { lkrt_lklist_dyn_at(xs, -1) }.payload, 20); // tail
        assert_eq!(unsafe { lkrt_lklist_dyn_at(xs, 9) }.tag, DYN_NIL); // OOB → nil
    }
}
