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

// ── Guarded unboxing (VM loud failure on tag mismatch) ─────────────────

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_tag(v: LkDyn) -> i64 {
    v.tag
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_i64(v: LkDyn) -> i64 {
    if v.tag != DYN_I64 {
        crate::abi::flush_and_abort();
    }
    v.payload
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_f64(v: LkDyn) -> f64 {
    match v.tag {
        DYN_F64 => v.f64_value(),
        DYN_I64 => v.payload as f64,
        _ => crate::abi::flush_and_abort(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_str(v: LkDyn) -> *const c_char {
    if v.tag != DYN_STR {
        crate::abi::flush_and_abort();
    }
    v.payload as *const c_char
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_bool(v: LkDyn) -> i64 {
    if v.tag != DYN_BOOL {
        crate::abi::flush_and_abort();
    }
    v.payload
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
        _ => crate::abi::flush_and_abort(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_sub(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_sub(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() - y.as_f64()),
        _ => crate::abi::flush_and_abort(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_mul(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_mul(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() * y.as_f64()),
        _ => crate::abi::flush_and_abort(),
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
                crate::abi::flush_and_abort();
            }
            from_f64(x.as_f64() / rhs)
        }
        _ => crate::abi::flush_and_abort(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_mod(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => {
            if y == 0 {
                crate::abi::flush_and_abort();
            }
            from_i64(x.wrapping_rem(y))
        }
        (Some(x), Some(y)) => {
            let rhs = y.as_f64();
            if rhs == 0.0 {
                crate::abi::flush_and_abort();
            }
            from_f64(x.as_f64() % rhs)
        }
        _ => crate::abi::flush_and_abort(),
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
        _ => false,
    }
}

macro_rules! dyn_ord {
    ($name:ident, $op:tt) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(a: LkDyn, b: LkDyn) -> i64 {
            match (a.as_numeric(), b.as_numeric()) {
                (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => i64::from(x $op y),
                (Some(x), Some(y)) => i64::from(x.as_f64() $op y.as_f64()),
                _ => crate::abi::flush_and_abort(),
            }
        }
    };
}
dyn_ord!(lkrt_dyn_lt, <);
dyn_ord!(lkrt_dyn_le, <=);
dyn_ord!(lkrt_dyn_gt, >);
dyn_ord!(lkrt_dyn_ge, >=);

// ── Display (two modes, matching the VM's two display paths) ───────────

fn display_into(out: &mut String, v: LkDyn, quoted: bool) {
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
                display_into(out, e, false);
            }
            out.push(']');
        }
        _ => crate::abi::flush_and_abort(),
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

/// Index into a Dyn: a List tag indexes like `lkrt_lklist_dyn_at`
/// (negative-from-tail, OOB → Nil); any non-container tag is the VM's
/// "index on a non-container" loud failure.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_index(v: LkDyn, index: i64) -> LkDyn {
    if v.tag != DYN_LIST {
        crate::abi::flush_and_abort();
    }
    let values = dyn_list(v);
    let len = values.len() as i64;
    let idx = if index < 0 { len + index } else { index };
    if idx < 0 || idx >= len {
        return LkDyn::NIL;
    }
    values[idx as usize]
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

/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_contains(handle: *mut c_void, value: LkDyn) -> i64 {
    if handle.is_null() {
        return 0;
    }
    let values = unsafe { &*(handle as *mut Vec<LkDyn>) };
    i64::from(values.iter().any(|&e| dyn_eq_inner(e, value)))
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
