//! `LkDyn` — the boxed dynamic value for natively-lowered mixed-type code
//! (plan M4.2 "deep coverage"). A tagged 2-word carrier passed **by value**
//! across the ABI (LLVM `{ i64, i64 }`, same shape as the `LkMaybe*`
//! carriers): scalars box with zero allocation, `Str`/list payloads are the
//! existing arena pointers reinterpreted as `i64`.
//!
//! Semantics contract: every operation here must match the VM (the
//! differential gates compare stdout byte-for-byte). Type errors are the
//! VM's loud failures — a raise that, uncaught, exits 1 (the contract
//! compares only `success()` + stdout, not stderr text).

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
use core::ffi::CStr;
use core::ffi::{c_char, c_void};

use crate::lkstr::arena_c_string;
use crate::state::arena_handle;

pub const DYN_NIL: i64 = 0;
pub const DYN_BOOL: i64 = 1;
pub const DYN_I64: i64 = 2;
pub const DYN_F64: i64 = 3;
pub const DYN_STR: i64 = 4;
pub const DYN_LIST: i64 = 5;
pub const DYN_MAP: i64 = 6;
/// A **raw handle** parked in a cell — not a value, and never produced by
/// boxing.
///
/// A `try` region carries a register the body assigns back out through a cell,
/// and a cell holds an `LkDyn`. That works by *boxing*, which for a typed
/// container is an element-wise conversion: the round trip would hand back a
/// copy and lose the body's writes. So a typed handle is parked as-is under this
/// tag instead, and the two cell families (`cell_get` / `cell_get_raw`) check
/// the tag rather than trusting the caller — reading a raw handle as a value, or
/// the reverse, is a *loud* failure and not a `Vec<i64>` walked as
/// `Vec<LkDyn>`.
pub const DYN_RAW: i64 = 7;

/// A `Set` handle, boxed.
///
/// `Set` and `Bytes` had no tag, so they could not be *boxed* at all — and
/// boxing is how a value enters a mixed container, a struct field, a bridged
/// return, or anything else that holds `LkDyn`. `[s]` and `{"k": s}` therefore
/// had no lowering, for a reason that had nothing to do with sets: the dynamic
/// carrier simply did not cover every value the language has.
pub const DYN_SET: i64 = 8;
/// A `Bytes` handle, boxed. See [`DYN_SET`].
pub const DYN_BYTES: i64 = 9;

/// A **typed map** handle, boxed in place — one tag per carrier.
///
/// `DYN_MAP` means a `str -> Dyn` map, so a typed carrier used to box by
/// *rebuilding* into one. That is a re-representation, and the fresh table's
/// iteration order is not the original's once the history includes deletions:
/// `println([m])` printed entries in an order the VM never would. A wrong
/// answer, not a fallback — and the rule against it was already written down on
/// [`DYN_RAW`].
///
/// Five tags rather than one because there are five carriers; the tag is the
/// only thing that says which. `lkmap::KIND_*` is the same numbering, minus the
/// base.
pub const DYN_TMAP_BASE: i64 = 10;
/// One past the last typed-map tag.
pub const DYN_TMAP_END: i64 = 15;

/// A **typed list** handle, boxed in place — one tag per carrier.
///
/// The same rule [`DYN_TMAP_BASE`] states, for the other container: boxing must
/// not re-represent. A typed list used to box by rebuilding element-wise into a
/// `Vec<LkDyn>`, and that copy is a *different list*, so both directions of
/// aliasing broke — `let xs = [1]; let c = [xs]; xs.push(2); c[0].len()` answered
/// 1 where the VM answers 2, and `c[0].push(9)` appended to the copy. Wrong
/// answers on programs that compiled fully native.
///
/// Three tags rather than one because there are three carriers; the tag is the
/// only thing that says which. The numbering below is the `kind` argument of
/// [`lkrt_dyn_from_typed_list`], and matches the lowering's carrier order.
pub const DYN_TLIST_BASE: i64 = 16;
/// `Vec<i64>` — `DYN_TLIST_BASE + 0`.
pub const TLIST_I64: i64 = 0;
/// `Vec<f64>` — `DYN_TLIST_BASE + 1`.
pub const TLIST_F64: i64 = 1;
/// `Vec<*const c_char>` — `DYN_TLIST_BASE + 2`.
pub const TLIST_STR: i64 = 2;
/// One past the last typed-list tag.
pub const DYN_TLIST_END: i64 = 19;

/// A **window** handle (`xs.slice(a, b)`), boxed in place. See [`DYN_SET`] for
/// why a carrier without a tag cannot be boxed at all, and therefore cannot
/// enter a list, a map, a struct field, or a `try` region's value.
///
/// In place, not materialized: a window *is* a range of its source, and boxing
/// it by copying would make `[w]` hold something that stops tracking the list
/// it windows — which the VM's `HeapValue::Slice` does not do either.
pub const DYN_SLICE: i64 = DYN_TMAP_END;

/// A closure as a **runtime value**: the payload is an `LkClosure` handle (see
/// `lkclosure`). Every other closure in the native build is a compile-time
/// reference, which is why storing one in a container had no form at all.
pub const DYN_CLOSURE: i64 = 20;

/// Whether a tag denotes a map of any representation.
pub(crate) fn is_map_tag(tag: i64) -> bool {
    tag == DYN_MAP || (DYN_TMAP_BASE..DYN_TMAP_END).contains(&tag)
}

/// Whether a tag denotes a list of any representation.
pub(crate) fn is_list_tag(tag: i64) -> bool {
    tag == DYN_LIST || (DYN_TLIST_BASE..DYN_TLIST_END).contains(&tag)
}

/// `IsList` / `IsMap` on a boxed value.
///
/// One tag comparison is not the question: a list has five representations
/// (the boxed one and four typed carriers) and a map six, and the interpreter
/// also answers **true** for a `String` — `let [a, b] = "ab"` is a list
/// destructuring there. Native lowering compared the tag against `DYN_LIST`
/// alone, so a list that happened to be in a typed carrier answered `false`,
/// compiled clean, and skipped the arm that should have run.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_is_list(v: LkDyn) -> i64 {
    i64::from(is_list_tag(v.tag) || v.tag == DYN_STR)
}

/// The map half of [`lkrt_dyn_is_list`]. A `String` is not a map.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_is_map(v: LkDyn) -> i64 {
    i64::from(is_map_tag(v.tag))
}

/// Boxes a typed list handle under its carrier's tag. `kind` is `TLIST_*`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_typed_list(handle: *mut c_void, kind: i64) -> LkDyn {
    if !(0..DYN_TLIST_END - DYN_TLIST_BASE).contains(&kind) {
        crate::panic::raise_str("runtime type error");
    }
    LkDyn {
        tag: DYN_TLIST_BASE + kind,
        payload: handle as i64,
    }
}

/// A boxed list's elements, whatever carrier holds them.
///
/// A `DYN_LIST` borrows its `Vec<LkDyn>`; a typed carrier has to box each
/// element, which is a copy — sound because every caller of this reads. The
/// callers that *write* (`push`) go to [`lkrt_dyn_list_push`] instead, which
/// reaches the carrier itself.
pub(crate) fn dyn_list_values<'a>(v: LkDyn) -> alloc::borrow::Cow<'a, [LkDyn]> {
    use alloc::borrow::Cow;
    if v.tag == DYN_LIST {
        return Cow::Borrowed(dyn_list(v));
    }
    if !is_list_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    Cow::Owned(crate::lklist::typed_list_boxed(
        v.tag - DYN_TLIST_BASE,
        v.payload as *mut c_void,
    ))
}

/// Boxes a typed map handle under its carrier's tag. `kind` is `lkmap::KIND_*`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_typed_map(handle: *mut c_void, kind: i64) -> LkDyn {
    if !(0..DYN_TMAP_END - DYN_TMAP_BASE).contains(&kind) {
        crate::panic::raise_str("runtime type error");
    }
    LkDyn {
        tag: DYN_TMAP_BASE + kind,
        payload: handle as i64,
    }
}

/// Boxes a `Set` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_set(handle: *mut c_void) -> LkDyn {
    LkDyn {
        tag: DYN_SET,
        payload: handle as i64,
    }
}

/// Boxes a `Bytes` handle.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_bytes(handle: *mut c_void) -> LkDyn {
    LkDyn {
        tag: DYN_BYTES,
        payload: handle as i64,
    }
}

/// Boxes a window handle. See [`DYN_SLICE`].
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_from_slice(handle: *mut c_void) -> LkDyn {
    LkDyn {
        tag: DYN_SLICE,
        payload: handle as i64,
    }
}

/// The window back out of the box, or a loud failure.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_slice(v: LkDyn) -> *mut c_void {
    if v.tag != DYN_SLICE {
        crate::panic::raise_str("runtime type error");
    }
    v.payload as *mut c_void
}

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

pub(crate) fn dyn_list<'a>(v: LkDyn) -> &'a [LkDyn] {
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

/// `-x` on a boxed value: an Int wraps at `i64::MIN` and a Float gets a real
/// `fneg`, exactly as `Executor::dispatch_neg` does. Anything else is the
/// VM's loud type error.
/// The type name a *caught* type error names its operand by.
///
/// The VM used to format `RuntimeVal::kind()`, which reports the
/// **representation**: a string of <= 7 bytes was `String` and a longer one
/// `Object`, as was every list, map and set. Its own doc said a caller with the
/// heap should use `HeapValue::type_name` — so the VM now does, and this is the
/// mirror of *that*: the language's type name, one per kind.
pub(crate) fn kind_name_of(v: LkDyn) -> String {
    kind_name(v)
}

fn kind_name(v: LkDyn) -> String {
    // A marked struct instance answers the name it was *declared* with. The
    // mirrored function got this right and this one did not, so `typeof(p)` on
    // a struct read `Map` compiled and `P` interpreted, and a type error
    // naming that operand said `Map` too. Third layer of the same rule: the
    // language's name for a struct instance is the struct's name.
    if let Some(name) = struct_type_name(v) {
        return name;
    }
    match v.tag {
        DYN_NIL => "Nil",
        DYN_BOOL => "Bool",
        DYN_I64 => "Int",
        DYN_F64 => "Float",
        DYN_STR => "String",
        tag if is_list_tag(tag) => "List",
        DYN_SET => "Set",
        DYN_BYTES => "Bytes",
        DYN_SLICE => "Slice",
        DYN_CLOSURE => "Function",
        tag if is_map_tag(tag) => "Map",
        _ => "Object",
    }
    .to_string()
}

/// The declared name of a marked struct instance, or `None` for anything else
/// (including a struct whose declaration never reached this runtime).
fn struct_type_name(v: LkDyn) -> Option<String> {
    let type_id = lkrt_dyn_obj_type_id(v);
    if type_id == 0 {
        return None;
    }
    with_struct_types(|types| types.get(&type_id).map(|desc| desc.name.clone()))
}

/// `typeof(x)` on a boxed value — the VM's `RuntimeVal::type_name_in`.
///
/// The lowering answers from the proven MIR type where it can; a `Dyn` or a
/// `MapStrDyn` cannot be decided statically (either may be a struct instance at
/// run time), so it asks here.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_type_name(v: LkDyn) -> *mut c_char {
    arena_c_string(CString::new(kind_name(v)).unwrap_or_default())
}

/// A binary type error in the VM's wording. `verb` is the operator as the VM
/// spells it — the source operator where one exists (`operator_symbol`), which
/// is now every case the AOT can reach. `Sub` is the one that still names an
/// opcode, and it does so in the VM too.
fn binary_type_error(verb: &str, tail: &str, a: LkDyn, b: LkDyn) -> ! {
    crate::panic::raise_str(&format!("{verb} {tail}, got {} and {}", kind_name(a), kind_name(b)))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_neg(v: LkDyn) -> LkDyn {
    match v.tag {
        DYN_I64 => from_i64(v.payload.wrapping_neg()),
        DYN_F64 => from_f64(-v.f64_value()),
        _ => crate::panic::raise_str(&format!("unary '-' expects Int or Float, got {}", kind_name(v))),
    }
}

/// `!x`: a Bool negates, Nil is `true`, anything else is the VM's loud
/// type error.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_not(v: LkDyn) -> i64 {
    match v.tag {
        DYN_NIL => 1,
        DYN_BOOL => i64::from(v.payload == 0),
        _ => crate::panic::raise_str(&format!("Not expected Bool or Nil, got {}", kind_name(v))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_i64(v: LkDyn) -> i64 {
    if v.tag != DYN_I64 {
        crate::panic::raise_str("runtime type error");
    }
    v.payload
}

/// `x as <integer>` where `x` is boxed: the source conversion the VM's
/// `cast_source_to_i64` performs, so a cast lowers natively even when its
/// operand came out of a container. Int passes through, Float truncates toward
/// zero, Bool is 0/1, and anything else raises with the VM's wording — the
/// width reduction itself stays in generated code.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_cast_to_i64(v: LkDyn) -> i64 {
    match v.tag {
        DYN_I64 => v.payload,
        DYN_F64 => v.f64_value() as i64,
        DYN_BOOL => v.payload,
        DYN_STR => crate::panic::raise_str("cannot cast String to an integer"),
        tag if is_list_tag(tag) => crate::panic::raise_str("cannot cast List to an integer"),
        DYN_MAP => crate::panic::raise_str("cannot cast Map to an integer"),
        DYN_SET => crate::panic::raise_str("cannot cast Set to an integer"),
        DYN_BYTES => crate::panic::raise_str("cannot cast Bytes to an integer"),
        DYN_SLICE => crate::panic::raise_str("cannot cast Slice to an integer"),
        _ => crate::panic::raise_str("cannot cast Nil to an integer"),
    }
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

// Thread-local under std, a spin-locked global on bare metal (no TLS there).
#[cfg(feature = "std")]
std::thread_local! {
    static OBJ_TYPE_MARKS: core::cell::RefCell<crate::lkmap::FxMap<usize, i64>> =
        core::cell::RefCell::new(crate::lkmap::FxMap::default());
}

#[cfg(not(feature = "std"))]
static OBJ_TYPE_MARKS_CELL: spin::Mutex<Option<crate::lkmap::FxMap<usize, i64>>> = spin::Mutex::new(None);

/// Runs `f` with the object type-mark table, however it is stored.
#[cfg(feature = "std")]
fn with_obj_type_marks<R>(f: impl FnOnce(&mut crate::lkmap::FxMap<usize, i64>) -> R) -> R {
    OBJ_TYPE_MARKS.with(|marks| f(&mut marks.borrow_mut()))
}

#[cfg(not(feature = "std"))]
fn with_obj_type_marks<R>(f: impl FnOnce(&mut crate::lkmap::FxMap<usize, i64>) -> R) -> R {
    let mut slot = OBJ_TYPE_MARKS_CELL.lock();
    f(slot.get_or_insert_with(crate::lkmap::FxMap::default))
}

/// Marks a freshly built struct-instance map with its lowering-assigned
/// type id (`NewObject` of a declared struct).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_lkmap_obj_mark(handle: *mut c_void, type_id: i64) {
    with_obj_type_marks(|marks| marks.insert(handle as usize, type_id));
}

/// One struct type as `display` needs it: its name, and its field names in
/// **declaration order**.
///
/// The lowering knows both, but it cannot spell the rendering out at the display
/// site: a *field* holding another struct is a bare `str→Dyn` map by then, and
/// whether a field holds one is not decidable there. So the knowledge has to be
/// available at runtime, where the mark is — and then nesting recurses through
/// the same display for free. (An earlier attempt inlined it and printed a
/// nested struct as a hash-ordered map; see `docs/aot/aot-gaps-and-lkrt.md`.)
#[derive(Default)]
struct StructTypeDesc {
    name: String,
    fields: Vec<String>,
}

#[cfg(feature = "std")]
thread_local! {
    static STRUCT_TYPES: core::cell::RefCell<crate::lkmap::FxMap<i64, StructTypeDesc>> =
        core::cell::RefCell::new(crate::lkmap::FxMap::default());
}

#[cfg(not(feature = "std"))]
static STRUCT_TYPES_CELL: spin::Mutex<Option<crate::lkmap::FxMap<i64, StructTypeDesc>>> = spin::Mutex::new(None);

#[cfg(feature = "std")]
fn with_struct_types<R>(f: impl FnOnce(&mut crate::lkmap::FxMap<i64, StructTypeDesc>) -> R) -> R {
    STRUCT_TYPES.with(|types| f(&mut types.borrow_mut()))
}

#[cfg(not(feature = "std"))]
fn with_struct_types<R>(f: impl FnOnce(&mut crate::lkmap::FxMap<i64, StructTypeDesc>) -> R) -> R {
    let mut slot = STRUCT_TYPES_CELL.lock();
    f(slot.get_or_insert_with(crate::lkmap::FxMap::default))
}

/// Opens a type's description: `type_id`'s name is `name`, no fields yet.
///
/// Called from the generated entry prologue, once per declared struct, followed
/// by one [`lkrt_struct_type_field`] per field in declaration order. A sequence
/// of calls rather than a static table because that needs nothing new from
/// codegen — the pieces are the `StrPtr`/`I64` shapes the ABI already has.
///
/// # Safety
/// `name` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_struct_type_begin(type_id: i64, name: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string constant.
    let name = if name.is_null() {
        String::new()
    } else {
        unsafe { core::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    with_struct_types(|types| {
        types.insert(
            type_id,
            StructTypeDesc {
                name,
                fields: Vec::new(),
            },
        )
    });
}

/// Appends one field name to `type_id`'s description. See
/// [`lkrt_struct_type_begin`].
///
/// # Safety
/// `field` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_struct_type_field(type_id: i64, field: *const c_char) {
    // SAFETY: as above.
    let field = if field.is_null() {
        String::new()
    } else {
        unsafe { core::ffi::CStr::from_ptr(field) }
            .to_string_lossy()
            .into_owned()
    };
    with_struct_types(|types| {
        if let Some(desc) = types.get_mut(&type_id) {
            desc.fields.push(field);
        }
    });
}

/// Renders a marked struct instance the way the VM does — `Name{f1:v1,f2:v2}`,
/// fields in declaration order, each value quoted as a nested one.
///
/// `false` (nothing written) when the value is not a marked struct or its type
/// was never described, so the caller falls through to the map rendering — which
/// is also what the VM does for a struct whose declaration is out of reach.
fn display_marked_struct(out: &mut String, v: LkDyn, raise_on_unknown: bool) -> bool {
    if v.tag != DYN_MAP || (v.payload as *mut c_void).is_null() {
        return false;
    }
    let type_id = with_obj_type_marks(|marks| marks.get(&(v.payload as usize)).copied().unwrap_or(0));
    if type_id == 0 {
        return false;
    }
    let Some((name, fields)) =
        with_struct_types(|types| types.get(&type_id).map(|desc| (desc.name.clone(), desc.fields.clone())))
    else {
        return false;
    };
    let entries = dyn_map(v);
    out.push_str(&name);
    out.push('{');
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(field);
        out.push(':');
        match entries.iter().find(|(k, _)| *k == field.as_str()) {
            Some((_, value)) => display_into_impl(out, *value, true, raise_on_unknown),
            None => out.push_str("nil"),
        }
    }
    out.push('}');
    true
}

/// Reads a boxed value's struct type mark; `0` = not a struct instance.
///
/// This used to say "or a type with no trait impls", which stopped being true
/// when `trait_env_prescan` started giving *every* declared struct an id (a
/// struct with no methods still has to print). The distinction matters:
/// equality reads the mark to tell two structurally-identical structs apart,
/// and it can only do that if being unmarked means "not a struct".
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_obj_type_id(v: LkDyn) -> i64 {
    if v.tag != DYN_MAP {
        return 0;
    }
    with_obj_type_marks(|marks| marks.get(&(v.payload as usize)).copied().unwrap_or(0))
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
    // `Executor::dynamic_add`, in its order — and the order is the rule, not a
    // detail: a list operand wins over a string one, so `"p=" + [1, 2]` is the
    // list `["p=", 1, 2]` and not the text `p=[1,2]`.
    //
    // Only the first and last cases were here before, under the belief that the
    // VM "only accepts Str + Str"; everything else raised. `"v=" + x` with a
    // boxed Int aborted the program where the VM prints `v=1`.

    // 1. Numbers.
    if let (Some(x), Some(y)) = (a.as_numeric(), b.as_numeric()) {
        return match (x, y) {
            (Numeric::Int(x), Numeric::Int(y)) => from_i64(x.wrapping_add(y)),
            _ => from_f64(x.as_f64() + y.as_f64()),
        };
    }
    // 2. Two maps merge, the right side winning.
    //
    // The **fill sequence** is the VM's, replayed: the left's entries in the
    // left's own order minus the keys the right also has, then the right's
    // entries in the right's own order (`merge_typed_maps` +
    // `typed_map_without_merge_keys`). A merge builds a new table, and a new
    // table's iteration order is decided by the order it was filled — so
    // "the same members" is not the same answer. This used to merge two
    // *unordered* views into a third, which is three different orders.
    if is_map_tag(a.tag) && is_map_tag(b.tag) {
        let left = crate::lkmap::map_entries_ordered(a);
        let right = crate::lkmap::map_entries_ordered(b);
        let replaced: crate::lkmap::FxSet<_> = right.iter().map(|(key, _)| key.clone()).collect();
        let mut merged: Vec<_> = left.into_iter().filter(|(key, _)| !replaced.contains(key)).collect();
        merged.extend(right);
        return LkDyn {
            tag: DYN_MAP,
            payload: crate::lkmap::str_dyn_from_ordered(merged) as i64,
        };
    }
    // 3. A list on *either* side concatenates; the other operand is one element.
    if is_list_tag(a.tag) || is_list_tag(b.tag) {
        let mut out: Vec<LkDyn> = Vec::new();
        for side in [a, b] {
            if is_list_tag(side.tag) {
                out.extend_from_slice(&dyn_list_values(side));
            } else {
                out.push(side);
            }
        }
        return LkDyn {
            tag: DYN_LIST,
            payload: arena_handle(out) as i64,
        };
    }
    // 4. A string on either side: display-concatenate. Both operands are
    //    scalars by now, which is what makes the bare display exact.
    if a.tag == DYN_STR || b.tag == DYN_STR {
        let mut joined = String::new();
        display_into(&mut joined, a, false);
        display_into(&mut joined, b, false);
        let ptr = arena_c_string(CString::new(joined).unwrap_or_default());
        return LkDyn {
            tag: DYN_STR,
            payload: ptr as i64,
        };
    }
    crate::panic::raise_str("runtime type error")
}

/// A map of any representation as `(key, value)` pairs under the general key,
/// for the merge above. A copy, and sound for the same reason
/// `lkmap::typed_map_keyed` is: the result is a *new* map either way.
pub(crate) fn map_entries(v: LkDyn) -> crate::lkmap::FxMap<crate::lkmap::MapKey, LkDyn> {
    if v.tag == DYN_MAP {
        crate::lkmap::boxed_map_keyed(v.payload as *mut c_void)
    } else {
        crate::lkmap::typed_map_keyed(v.tag - DYN_TMAP_BASE, v.payload as *mut c_void)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_sub(a: LkDyn, b: LkDyn) -> LkDyn {
    if let (Some(x), Some(y)) = (a.as_numeric(), b.as_numeric()) {
        return match (x, y) {
            (Numeric::Int(x), Numeric::Int(y)) => from_i64(x.wrapping_sub(y)),
            (x, y) => from_f64(x.as_f64() - y.as_f64()),
        };
    }
    // `-` removes, which this had never implemented — while its own error text
    // said "expected numbers or list/map lhs", borrowing the VM's rule to
    // describe an ability it did not have. The VM's `dynamic_sub` drops every
    // element of `b` from a list and every key of `b` from a map.
    //
    // Order, as everywhere else: the answer keeps the left's own order, since
    // removal takes entries away and never adds one.
    if is_list_tag(a.tag) && is_list_tag(b.tag) {
        let drop = dyn_list_values(b);
        let kept: Vec<LkDyn> = dyn_list_values(a)
            .iter()
            .filter(|value| !drop.iter().any(|other| contains_eq(**value, *other)))
            .copied()
            .collect();
        return LkDyn {
            tag: DYN_LIST,
            payload: arena_handle(kept) as i64,
        };
    }
    if is_map_tag(a.tag) && is_map_tag(b.tag) {
        let drop: crate::lkmap::FxSet<_> = crate::lkmap::map_entries_ordered(b)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        let kept: Vec<_> = crate::lkmap::map_entries_ordered(a)
            .into_iter()
            .filter(|(key, _)| !drop.contains(key))
            .collect();
        return LkDyn {
            tag: DYN_MAP,
            payload: crate::lkmap::str_dyn_from_ordered(kept) as i64,
        };
    }
    binary_type_error("Sub", "expected numbers or list/map lhs", a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_mul(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_mul(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() * y.as_f64()),
        _ => binary_type_error("*", "expects Int or Float", a, b),
    }
}

/// `/` always produces Float in LK (docs/semantics.md, the numeric adjudication), zero divisor is
/// a loud failure.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_div(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        // `/` yields a `Float` for every numeric pair, and `f64` division by
        // zero is an infinity or a NaN rather than a raise — the same as the
        // VM, which this file exists to mirror.
        (Some(x), Some(y)) => from_f64(x.as_f64() / y.as_f64()),
        _ => binary_type_error("/", "expects Int or Float", a, b),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_mod(a: LkDyn, b: LkDyn) -> LkDyn {
    match (a.as_numeric(), b.as_numeric()) {
        (Some(Numeric::Int(_)), Some(Numeric::Int(0))) => crate::panic::raise_str("ModInt divisor is zero"),
        (Some(Numeric::Int(x)), Some(Numeric::Int(y))) => from_i64(x.wrapping_rem(y)),
        (Some(x), Some(y)) => from_f64(x.as_f64() % y.as_f64()),
        _ => binary_type_error("%", "expects Int or Float", a, b),
    }
}

// ── Equality / ordering ────────────────────────────────────────────────

/// [`lkrt_dyn_as_i64`] / [`lkrt_dyn_as_str`] for a value used as a **map key**.
///
/// A key of a type no map can hold is refused by name — the interpreter's
/// wording, which `vm_mirror::key_from_dyn` also raises for a boxed map. A
/// typed carrier does not go through that function (it stores the key
/// unboxed), so without these two it answered the generic "runtime type error"
/// for `m[|x| x] = 1`.
fn reject_non_key(v: LkDyn) {
    match v.tag {
        DYN_NIL | DYN_BOOL | DYN_I64 | DYN_STR => {}
        DYN_F64 => crate::panic::raise_str("Float cannot be a map key or set member"),
        _ => crate::panic::raise_str(&alloc::format!(
            "{} cannot be a map key or set member: only nil, Bool, Int and String can",
            kind_name_of(v)
        )),
    }
}

/// An `Int`-carrier map's key.
///
/// # Safety
/// As [`lkrt_dyn_as_i64`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_as_key_i64(v: LkDyn) -> i64 {
    reject_non_key(v);
    lkrt_dyn_as_i64(v)
}

/// A `String`-carrier map's key.
///
/// # Safety
/// As [`lkrt_dyn_as_str`]: the returned pointer borrows `v`'s payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_as_key_str(v: LkDyn) -> *const c_char {
    reject_non_key(v);
    lkrt_dyn_as_str(v)
}

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
    // Two maps compare whatever their representations are — a typed carrier
    // against a boxed one is `{"a": 1} == {"a": 1, "b": "x"}` written twice,
    // and the tag difference is a storage detail. Both sides take the general
    // key view, which is a *copy*: sound only because `==` over maps is
    // order-free (see `lkmap::typed_map_keyed`).
    if is_map_tag(a.tag) && is_map_tag(b.tag) {
        // A struct is a marked map and its type is part of its identity; a
        // typed carrier is never a struct, so its mark is 0.
        if lkrt_dyn_obj_type_id(a) != lkrt_dyn_obj_type_id(b) {
            return false;
        }
        let keyed = |v: LkDyn| {
            if v.tag == DYN_MAP {
                crate::lkmap::boxed_map_keyed(v.payload as *mut c_void)
            } else {
                crate::lkmap::typed_map_keyed(v.tag - DYN_TMAP_BASE, v.payload as *mut c_void)
            }
        };
        let (xs, ys) = (keyed(a), keyed(b));
        return xs.len() == ys.len() && xs.iter().all(|(k, &v)| ys.get(k).is_some_and(|&w| dyn_eq_inner(v, w)));
    }
    // A window compares by *content*, against another window or against a
    // list: the VM says `xs.slice(0, 2) == [3, 1]`, because a window is a range
    // of a list and not a distinct kind of value. Element-wise rather than
    // handle-wise, and across the tag difference, for the same reason the two
    // map representations compare across theirs.
    if (a.tag == DYN_SLICE || b.tag == DYN_SLICE)
        && (b.tag == DYN_SLICE || is_list_tag(b.tag))
        && (a.tag == DYN_SLICE || is_list_tag(a.tag))
    {
        let boxed = |v: LkDyn| -> alloc::vec::Vec<LkDyn> {
            if v.tag == DYN_SLICE {
                // SAFETY: a `DYN_SLICE` payload is a live window handle.
                unsafe { crate::lkslice::window_elements(v.payload as *mut c_void) }
                    .iter()
                    .map(|value| lkrt_dyn_from_i64(*value))
                    .collect()
            } else {
                dyn_list_values(v).into_owned()
            }
        };
        let (xs, ys) = (boxed(a), boxed(b));
        return xs.len() == ys.len() && xs.iter().zip(ys).all(|(&x, y)| dyn_eq_inner(x, y));
    }
    // Two lists compare element-wise across representations, for the same
    // reason the two map representations do: `[1]` written as a typed carrier
    // and the same list boxed are one value, and which representation a program
    // happens to hold is not something it can see.
    if is_list_tag(a.tag) && is_list_tag(b.tag) {
        let (xs, ys) = (dyn_list_values(a), dyn_list_values(b));
        return xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(&x, &y)| dyn_eq_inner(x, y));
    }
    if a.tag != b.tag {
        return false;
    }
    match a.tag {
        DYN_NIL => true,
        DYN_BOOL => a.payload == b.payload,
        DYN_STR => unsafe { dyn_str(a) == dyn_str(b) },
        DYN_MAP => {
            // A struct instance is a marked map, and its *type* is part of
            // its identity: the VM says `P{x:1} != Q{x:1}` and
            // `P{x:1} != {"x":1}`, both of which are structurally equal. The
            // mark answers all three cases at once — every declared struct
            // gets an id (`trait_env_prescan`), and a plain map has none, so
            // comparing ids first is exactly the VM's rule.
            //
            // Checked before the null guard so a marked-but-empty struct is
            // not equal to `{}`.
            if lkrt_dyn_obj_type_id(a) != lkrt_dyn_obj_type_id(b) {
                return false;
            }
            if (a.payload as *mut c_void).is_null() || (b.payload as *mut c_void).is_null() {
                return a.payload == b.payload;
            }
            let (xs, ys) = (dyn_map(a), dyn_map(b));
            // Structural, order-free (hash iteration order is not portable,
            // but key-lookup equality is).
            xs.len() == ys.len() && xs.iter().all(|(k, &v)| ys.get(k).is_some_and(|&w| dyn_eq_inner(v, w)))
        }
        // Both compare by *content*, the same rule their unboxed spellings
        // follow (`set.eq` is order-free; `bytes.eq` is byte-wise).
        // SAFETY: a `DYN_SET`/`DYN_BYTES` payload is a live handle of that
        // kind — the tag is only ever set by `from_set`/`from_bytes`.
        DYN_SET => unsafe { crate::lkset::lkrt_lkset_eq(a.payload as *mut c_void, b.payload as *mut c_void) != 0 },
        DYN_BYTES => unsafe {
            crate::lkbytes::lkrt_lkbytes_eq(a.payload as *mut c_void, b.payload as *mut c_void) != 0
        },
        // By reference, which is the VM's rule for a callable: `let g = f`
        // makes one closure two names, and two lambdas written the same way
        // are two closures. Structural equality would call the second pair
        // equal. Native lowering keeps that rule by building a lambda used as
        // a value *once*, at its definition (`inst/call.rs::bind_lambda`).
        DYN_CLOSURE => a.payload == b.payload,
        _ => false,
    }
}

macro_rules! dyn_ord {
    ($name:ident, $op:tt, $vm_name:literal) => {
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
                _ => binary_type_error($vm_name, "expected Int, Float, or String", a, b),
            }
        }
    };
}
dyn_ord!(lkrt_dyn_lt, <, "<");
dyn_ord!(lkrt_dyn_le, <=, "<=");
dyn_ord!(lkrt_dyn_gt, >, ">");
dyn_ord!(lkrt_dyn_ge, >=, ">=");

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
        tag if is_list_tag(tag) => {
            // A string inside a container is quoted, whatever the container's
            // representation is. This used to pass `false` here, mirroring a VM
            // quirk: a *mixed* list rendered its strings bare (`[1,a b,2]`)
            // while a typed string list quoted them (`["a","b c"]`) — the same
            // value shown two ways, decided by an internal representation no
            // program can see. The VM stopped doing that; this follows, and the
            // differential gate is what noticed.
            out.push('[');
            for (i, &e) in dyn_list_values(v).iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                display_into_impl(out, e, true, raise_on_unknown);
            }
            out.push(']');
        }
        DYN_MAP if display_marked_struct(out, v, raise_on_unknown) => {}
        DYN_MAP => {
            // Quoted keys *and* values (`{"k":1,"s":"txt"}`) — a value in a
            // map is inside a container too, and the keys were already quoted.
            // The entry order is the Fx layout order — the mirror discipline
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
                    display_into_impl(out, e, true, raise_on_unknown);
                }
            }
            out.push('}');
        }
        // Rendered through the same function the unboxed spelling calls, so a
        // set in a list and a set on its own cannot drift apart.
        // Rendered straight off the carrier — no copy, so the order is the
        // map's own. This is the arm the rebuild used to route through.
        tag if (DYN_TMAP_BASE..DYN_TMAP_END).contains(&tag) => {
            out.push_str(&crate::lkmap::typed_map_text(
                tag - DYN_TMAP_BASE,
                v.payload as *mut c_void,
            ));
        }
        DYN_SET => out.push_str(&crate::lkset::set_text(v.payload as *mut c_void)),
        DYN_BYTES => out.push_str(&crate::lkbytes::bytes_text(v.payload as *mut c_void)),
        // A window renders as the list it windows, which is what the VM shows.
        DYN_SLICE => out.push_str(&crate::lkslice::slice_text(v.payload as *mut c_void)),
        // SAFETY: the tag is only set by `lkrt_closure_new`.
        DYN_CLOSURE => out.push_str(&unsafe { crate::lkclosure::closure_text(v) }),
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
        // Counted off the carrier — no boxing, which is the whole point of a
        // tag that names one.
        tag if (DYN_TLIST_BASE..DYN_TLIST_END).contains(&tag) => {
            crate::lklist::typed_list_len(tag - DYN_TLIST_BASE, v.payload as *mut c_void)
        }
        DYN_MAP => {
            if (v.payload as *mut c_void).is_null() {
                0
            } else {
                dyn_map(v).len() as i64
            }
        }
        DYN_STR => unsafe { dyn_str(v) }.chars().count() as i64,
        // SAFETY: as in `dyn_eq_inner`, the tag guarantees the handle kind.
        tag if (DYN_TMAP_BASE..DYN_TMAP_END).contains(&tag) => {
            crate::lkmap::typed_map_len(tag - DYN_TMAP_BASE, v.payload as *mut c_void)
        }
        DYN_SET => unsafe { crate::lkset::lkrt_lkset_len(v.payload as *mut c_void) },
        DYN_BYTES => unsafe { crate::lkbytes::lkrt_lkbytes_len(v.payload as *mut c_void) },
        // SAFETY: a `DYN_SLICE` payload is a live window handle — the tag is
        // only ever set by `from_slice`.
        DYN_SLICE => unsafe { crate::lkslice::lkrt_lkslice_i64_len(v.payload as *mut c_void) },
        _ => crate::panic::raise_str("runtime type error"),
    }
}

/// Guarded list unboxing: a `Vec<LkDyn>` handle for a boxed list of either
/// representation (loud failure otherwise — a method on a non-list is a VM
/// error).
///
/// **Read-only.** A `DYN_LIST` hands back its own handle, so a write through it
/// would be visible; a typed carrier has to box its elements, so a write
/// through *that* one would be lost. The two cannot both be served here, and
/// every name that reaches this guard — `map`, `filter`, `reduce`, `take`,
/// `skip`, `concat`, `unique`, `sort`, `reverse` — builds a new list and leaves
/// the receiver alone (`sort` and `reverse` answer new lists in this language;
/// they do not sort in place). `push` is the one mutating consumer and it goes
/// to [`lkrt_dyn_list_push`], which reaches the carrier itself.
///
/// `no_unbox_list_name_mutates_its_receiver` in the lowering is what keeps
/// that true: a mutating name given `unbox_list` would silently start dropping
/// writes here.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_as_list(v: LkDyn) -> *mut c_void {
    if v.tag == DYN_LIST {
        return v.payload as *mut c_void;
    }
    if !is_list_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    arena_handle(crate::lklist::typed_list_boxed(
        v.tag - DYN_TLIST_BASE,
        v.payload as *mut c_void,
    ))
}

/// `xs.push(e)` where `xs` is boxed — appends to the carrier behind the tag, so
/// the box and the original stay one list.
///
/// The counterpart to [`lkrt_dyn_as_list`]'s read-only rule. `ListPush` used to
/// unbox through that guard, which for a typed carrier meant appending to a
/// materialized copy: `c[0].push(9)` answered as if nothing had been pushed.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_list_push(v: LkDyn, value: LkDyn) {
    if v.tag == DYN_LIST {
        // SAFETY: a `DYN_LIST` payload is a live `Vec<LkDyn>`, uniquely
        // reachable through this call for its duration.
        unsafe { (*(v.payload as *mut Vec<LkDyn>)).push(value) };
        return;
    }
    if !is_list_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    crate::lklist::typed_list_push(v.tag - DYN_TLIST_BASE, v.payload as *mut c_void, value);
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

/// Constant-string field read on a Dyn: a map tag of **either** representation
/// looks the key up (missing key → Nil, the VM's nil-on-missing); any non-map
/// tag is the VM's loud failure on member access.
///
/// The typed arm is why this dispatches rather than unboxing. A typed map boxed
/// in place keeps its own carrier, so `dyn.as_map` — which hands back a
/// `str_dyn` handle — cannot serve it, and a member read through that guard
/// raised `runtime type error` on a program the VM answers. Every read of a
/// boxed value has to know both representations; only `len`, display and
/// equality did.
///
/// # Safety
/// `key` must be a NUL-terminated string; a Map payload must be a live
/// `map_h str_dyn` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_field(v: LkDyn, key: *const c_char) -> LkDyn {
    if !is_map_tag(v.tag) || (v.payload as *mut c_void).is_null() {
        crate::panic::raise_str("runtime type error");
    }
    let key = if key.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(key) }.to_str().unwrap_or("")
    };
    if v.tag == DYN_MAP {
        return dyn_map(v).get(key).copied().unwrap_or(LkDyn::NIL);
    }
    map_entries(v)
        .get(&crate::vm_mirror::str_key(key))
        .copied()
        .unwrap_or(LkDyn::NIL)
}

/// [`lkrt_dyn_field`] read by **position**, with the key as the check.
///
/// The boxed twin of `lkrt_lkmap_str_dyn_get_at`, for the shape a member chain
/// produces: `nodes[i].next` reads its element as a boxed value, so the field
/// read goes through the tag check rather than through a typed map handle.
/// Only the boxed `Map<str, Dyn>` representation has a position to read; a
/// typed carrier is never a struct, and falls through to the keyed path.
///
/// # Safety
/// As [`lkrt_dyn_field`], plus `key_len` bytes readable at `key`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_field_at(v: LkDyn, index: i64, key: *const c_char, key_len: i64) -> LkDyn {
    if !is_map_tag(v.tag) || (v.payload as *mut c_void).is_null() {
        crate::panic::raise_str("runtime type error");
    }
    if v.tag == DYN_MAP
        && index >= 0
        && let Some((found, value)) = dyn_map(v).get_index(index as usize)
        && found.len() == key_len as usize
        // SAFETY: `key_len` bytes are readable at `key`, as documented.
        && found.as_bytes() == unsafe { core::slice::from_raw_parts(key as *const u8, key_len as usize) }
    {
        return *value;
    }
    // SAFETY: as documented.
    unsafe { lkrt_dyn_field(v, key) }
}

/// Index into a Dyn: a List tag indexes like `lkrt_lklist_dyn_at`
/// (negative-from-tail, OOB → Nil); any non-container tag is the VM's
/// "index on a non-container" loud failure.
/// `container[key]` where *both* are boxed.
///
/// The static types say nothing about which access this is, so the tag decides
/// — which is what the VM does. A string key reads a field; an integer key
/// indexes a sequence but *looks up* in a map, because an integer-keyed map's
/// keys are keys and not positions (`{3: "a"}[3]` is `"a"`, and there is no
/// element 3).
///
/// # Safety
///
/// `key`'s payload must be a valid interned string when its tag says so, which
/// is the runtime's own invariant for a `DYN_STR`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_get(v: LkDyn, key: LkDyn) -> LkDyn {
    match key.tag {
        DYN_I64 => lkrt_dyn_index(v, key.payload),
        DYN_STR => unsafe { lkrt_dyn_field(v, key.payload as *const c_char) },
        _ => crate::panic::raise_str("runtime type error"),
    }
}

/// `for pair in m` / `m.keys()` / `m.values()` / `m.has(k)` / `m.delete(k)` on
/// a **boxed** map, dispatched on the tag.
///
/// The unboxed spellings reach a carrier-specific symbol because the static
/// type names the carrier. A boxed map has no static carrier — the tag is the
/// only thing that says which — and `dyn.as_map`, which hands back a `str_dyn`
/// handle, cannot serve a typed one. Unboxing through that guard is what made
/// `c[0].keys()` raise `runtime type error` on a program the VM answers.
///
/// Materializing a `str_dyn` copy inside the guard would answer the reads and
/// silently drop `delete`, so the dispatch is per operation rather than per
/// unbox.
///
/// # Safety
/// A map payload must be a live handle of the carrier its tag names.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_map_pairs(v: LkDyn) -> *mut c_void {
    if v.tag == DYN_MAP {
        // SAFETY: a `DYN_MAP` payload is a live `StrDynMap`.
        return unsafe { crate::lkmap::lkrt_lkmap_str_dyn_iter_pairs(v.payload as *mut c_void) };
    }
    if !is_map_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    crate::lkmap::typed_map_pair_list(v.tag - DYN_TMAP_BASE, v.payload as *mut c_void)
}

/// The `n`th component of every `[key, value]` pair — 0 for `.keys()`, 1 for
/// `.values()`. See [`lkrt_dyn_map_pairs`].
///
/// # Safety
/// As [`lkrt_dyn_map_pairs`].
unsafe fn dyn_map_pair_column(v: LkDyn, column: usize) -> *mut c_void {
    let pairs = unsafe { lkrt_dyn_map_pairs(v) };
    let column: Vec<LkDyn> = dyn_slice(pairs)
        .iter()
        .map(|pair| dyn_list(*pair).get(column).copied().unwrap_or(LkDyn::NIL))
        .collect();
    arena_handle(column)
}

/// `for x in v` where `v` is boxed — the VM's `to_iter` normalization, decided
/// by the tag.
///
/// The loop lowering used to call `dyn.as_list` here, which is a *list* guard:
/// every other iterable answered `runtime type error` once boxed, including
/// every map. `to_iter` is not "unwrap a list", it is "what does this value
/// iterate as", and each carrier already has that answer.
///
/// # Safety
/// The payload must be a live handle of the carrier its tag names.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_to_iter(v: LkDyn) -> *mut c_void {
    if is_map_tag(v.tag) {
        return unsafe { lkrt_dyn_map_pairs(v) };
    }
    match v.tag {
        DYN_LIST => v.payload as *mut c_void,
        // A typed carrier snapshots, which is what the VM's `to_iter` does for
        // every map too: the loop reads elements as values, and a value read
        // out of an `i64` carrier has to be boxed to be one.
        tag if (DYN_TLIST_BASE..DYN_TLIST_END).contains(&tag) => arena_handle(crate::lklist::typed_list_boxed(
            tag - DYN_TLIST_BASE,
            v.payload as *mut c_void,
        )),
        // A window iterates as itself; `len` and indexing on the loop handle
        // are window-relative, which is what the loop wants.
        DYN_SLICE => v.payload as *mut c_void,
        DYN_SET => unsafe { crate::lkset::lkrt_lkset_iter(v.payload as *mut c_void) },
        // The i64 list the unboxed spelling also iterates: byte values, in
        // order, boxed one per element so the loop variable is a value.
        DYN_BYTES => {
            let values: Vec<LkDyn> = crate::lkbytes::bytes_slice(v.payload as *mut c_void)
                .iter()
                .map(|byte| lkrt_dyn_from_i64(i64::from(*byte)))
                .collect();
            arena_handle(values)
        }
        DYN_STR => unsafe { crate::lkstr::lkrt_str_chars(v.payload as *const c_char) },
        _ => crate::panic::raise_str("runtime type error"),
    }
}

/// `needle in v` where `v` is boxed — the tag decides what membership means.
///
/// A map answers **key** membership (a stored nil still counts, which is why it
/// is not get-then-test); every other container answers element membership
/// under [`contains_eq`]. Both are what the unboxed spellings already do; this
/// is the one entry point that can pick between them at run time, which is what
/// a boxed haystack needs — `"a" in c[0]` used to drop the whole program to the
/// VM because the lowering had no arm for a `Dyn` haystack at all.
///
/// # Safety
/// The payload must be a live handle of the carrier its tag names.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_contains(v: LkDyn, needle: LkDyn) -> i64 {
    if is_map_tag(v.tag) {
        if needle.tag == DYN_STR {
            return unsafe { lkrt_dyn_map_has(v, needle.payload as *const c_char) };
        }
        return i64::from(map_entries(v).contains_key(&crate::vm_mirror::key_from_dyn(needle)));
    }
    if is_list_tag(v.tag) {
        return i64::from(dyn_list_values(v).iter().any(|&e| contains_eq(e, needle)));
    }
    match v.tag {
        // A string's members are its substrings, which is what the unboxed
        // spelling answers; it was the one carrier `in` did not reach here.
        DYN_STR => unsafe { crate::lkstr::lkrt_str_contains(v.payload as *const c_char, lkrt_dyn_as_str(needle)) },
        DYN_SET => unsafe { crate::lkset::lkrt_lkset_has(v.payload as *mut c_void, needle) },
        DYN_SLICE => {
            // SAFETY: a `DYN_SLICE` payload is a live window handle.
            let window = unsafe { crate::lkslice::window_elements(v.payload as *mut c_void) };
            i64::from(window.iter().any(|&e| contains_eq(lkrt_dyn_from_i64(e), needle)))
        }
        DYN_BYTES => {
            let bytes = crate::lkbytes::bytes_slice(v.payload as *mut c_void);
            i64::from(
                bytes
                    .iter()
                    .any(|&b| contains_eq(lkrt_dyn_from_i64(i64::from(b)), needle)),
            )
        }
        _ => crate::panic::raise_str("runtime type error"),
    }
}

/// `m.keys()` on a boxed map. See [`lkrt_dyn_map_pairs`].
///
/// # Safety
/// As [`lkrt_dyn_map_pairs`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_map_keys(v: LkDyn) -> *mut c_void {
    unsafe { dyn_map_pair_column(v, 0) }
}

/// `m.values()` on a boxed map. See [`lkrt_dyn_map_pairs`].
///
/// # Safety
/// As [`lkrt_dyn_map_pairs`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_map_values(v: LkDyn) -> *mut c_void {
    unsafe { dyn_map_pair_column(v, 1) }
}

/// `m.has(k)` on a boxed map — presence, which is order-free, so it reads the
/// keyed view rather than the ordered snapshot.
///
/// # Safety
/// `key` must be NUL-terminated; the payload as [`lkrt_dyn_map_pairs`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_map_has(v: LkDyn, key: *const c_char) -> i64 {
    if !is_map_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    let key = if key.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(key) }.to_str().unwrap_or("")
    };
    if v.tag == DYN_MAP {
        return i64::from(dyn_map(v).contains_key(key));
    }
    i64::from(map_entries(v).contains_key(&crate::vm_mirror::str_key(key)))
}

/// `m.delete(k)` / `m.remove(k)` on a boxed map — removes **in place**, so the
/// box and the original stay one map, and answers the removed value or nil.
///
/// # Safety
/// `key` must be NUL-terminated; the payload as [`lkrt_dyn_map_pairs`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_dyn_map_delete(v: LkDyn, key: *const c_char) -> LkDyn {
    if v.tag == DYN_MAP {
        // SAFETY: a `DYN_MAP` payload is a live `StrDynMap`; `key` is the
        // caller's NUL-terminated key.
        return unsafe { crate::lkmap::lkrt_lkmap_str_dyn_delete(v.payload as *mut c_void, key) };
    }
    if !is_map_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    crate::lkmap::typed_map_delete(v.tag - DYN_TMAP_BASE, v.payload as *mut c_void, key)
}

/// `c[k] = v` where `c` is boxed — stores into the carrier behind the tag, so
/// the box and the original stay one container.
///
/// One entry point for both containers, because the key rule is one rule: an
/// integer key on a map is a *key*, not a position (the same adjudication
/// [`lkrt_dyn_index`] states for reads). The key travels boxed so this side can
/// apply it; a key of the wrong shape for the carrier raises.
///
/// The store twin of [`lkrt_dyn_list_push`]: `dyn.as_list` and `dyn.as_map` are
/// read-only, and a write through either would land in a materialized copy.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_index_set(v: LkDyn, key: LkDyn, value: LkDyn) {
    if is_map_tag(v.tag) {
        if v.tag == DYN_MAP {
            // SAFETY: a `DYN_MAP` payload is a live `StrDynMap`; the key
            // pointer is the boxed key's own NUL-terminated string.
            unsafe { crate::lkmap::lkrt_lkmap_str_dyn_set(v.payload as *mut c_void, lkrt_dyn_as_str(key), value) };
            return;
        }
        crate::lkmap::typed_map_set(v.tag - DYN_TMAP_BASE, v.payload as *mut c_void, key, value);
        return;
    }
    let index = lkrt_dyn_as_i64(key);
    if v.tag == DYN_LIST {
        // SAFETY: a `DYN_LIST` payload is a live `Vec<LkDyn>`.
        unsafe { lkrt_lklist_dyn_set(v.payload as *mut c_void, index, value) };
        return;
    }
    if !is_list_tag(v.tag) {
        crate::panic::raise_str("runtime type error");
    }
    crate::lklist::typed_list_set(v.tag - DYN_TLIST_BASE, v.payload as *mut c_void, index, value);
}

/// An integer key on a map is a *key*, not a position.
///
/// `{3: 4}[3]` is `4` and there is no element 3 — so a map tag of either
/// representation looks up here rather than indexing. This is the same
/// entry point a constant integer key lowers to directly, which is why the
/// rule lives here and not only in [`lkrt_dyn_get`].
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_dyn_index(v: LkDyn, index: i64) -> LkDyn {
    if is_map_tag(v.tag) {
        return map_entries(v)
            .get(&crate::vm_mirror::RtKey::Int(index))
            .copied()
            .unwrap_or(LkDyn::NIL);
    }
    // A string indexes by character, which is what `s[0]` does on a *typed*
    // `Str` already. It reaches here whenever the same string is boxed —
    // `[a, b]` destructuring one, for instance, since `IsList` calls a string
    // list-like the way the interpreter does.
    if v.tag == DYN_STR {
        // SAFETY: a `DYN_STR` payload is a live NUL-terminated string.
        return unsafe { crate::lkstr::lkrt_str_char_at(v.payload as *const c_char, index) };
    }
    let values = dyn_list_values(v);
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

/// Joins a boxed list with `separator`, each element written bare.
///
/// `display_into(.., quoted = false)` is the same renderer `lkrt_dyn_display`
/// uses, which is the one the VM's `join` uses too: a string element joins
/// unquoted, while the *quoted* form is what an element gets when it is printed
/// inside a list. Sharing the renderer is the point — the alternative is a
/// second opinion on how `2.0` or `nil` looks.
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null;
/// `separator` a valid C string, or null for empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_join(handle: *mut c_void, separator: *const c_char) -> *mut c_char {
    let sep = if separator.is_null() {
        ""
    } else {
        // SAFETY: caller guarantees a valid C string.
        unsafe { core::ffi::CStr::from_ptr(separator) }.to_str().unwrap_or("")
    };
    if handle.is_null() {
        return arena_c_string(CString::default());
    }
    // SAFETY: `handle` addresses a `Vec<LkDyn>` from `lkrt_lklist_dyn_new`.
    let values = unsafe { &*(handle as *mut Vec<LkDyn>) };
    let mut out = String::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(sep);
        }
        display_into(&mut out, *value, false);
    }
    arena_c_string(CString::new(out).unwrap_or_default())
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
        crate::panic::raise_str("runtime error");
    }
    let values = unsafe { &mut *(handle as *mut Vec<LkDyn>) };
    // Out of range is a halt, matching the VM's `list index N out of bounds`.
    // This used to *grow* the list to fit (and silently ignore an index before
    // the start), so `xs[9] = 1` on a three-element list raised interpreted and
    // appended six nils compiled. The wording comes from the one helper the
    // typed lists use, because a caught error is printed output.
    let idx = crate::lklist::store_index_or_raise(index, values.len());
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
pub(crate) fn contains_eq(a: LkDyn, b: LkDyn) -> bool {
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
        // Every heap carrier compares by handle, not just the two that had a
        // tag when this was written — so this is the *default*, and the list
        // is of what is excluded. Enumerating the included tags instead is
        // what left `Set`, `Bytes`, windows and typed maps out for as long as
        // they existed, and then `Function` after them.
        //
        // `_ => false` meant a `Set`, a `Bytes`, a window or a typed map was
        // **never** in any list, however the program got it there:
        //
        // ```lk
        // let b = "ab".bytes();
        // let xs = [b];
        // b in xs            // true interpreted, false compiled
        // ```
        //
        // Those four carriers box *in place* — the tag is the only thing that
        // changed — so their payload is the same handle the VM compares, and
        // the arm above was already the right answer for them. They were simply
        // added to the tag space (see `DYN_SET`, `DYN_TMAP_BASE`, `DYN_SLICE`)
        // without this match being revisited.
        //
        // `DYN_RAW` stays out: it parks a handle that is not a value, and
        // reading one as a value is a loud failure by design.
        DYN_RAW => false,
        _ => a.payload == b.payload,
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

/// Range slice of a boxed list, sharing `lklist::slice_bounds` — one rule, not
/// a fourth copy of "negative counts from the tail and everything clamps".
///
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_slice(handle: *mut c_void, start: i64, end: i64) -> *mut c_void {
    let values = dyn_slice(handle);
    let (start, end) = crate::lklist::slice_bounds(values.len(), start, end);
    arena_handle(values[start..end].to_vec())
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
    // Indexed, re-dereferencing the handle each step: `f`/`p` re-enters generated
    // code, which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past a borrow, so none may be held across the call. Re-deref rather
    // than a `to_vec()` snapshot — this is the native HOF hot path the perf gate
    // measures.
    let len = dyn_slice(handle).len();
    let mut mapped: Vec<LkDyn> = Vec::with_capacity(len);
    for index in 0..len {
        let Some(&value) = dyn_slice(handle).get(index) else {
            break;
        };
        mapped.push(f(value));
    }
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
    // Indexed, re-dereferencing the handle each step: `f`/`p` re-enters generated
    // code, which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past a borrow, so none may be held across the call. Re-deref rather
    // than a `to_vec()` snapshot — this is the native HOF hot path the perf gate
    // measures.
    let len = dyn_slice(handle).len();
    let mut kept: Vec<LkDyn> = Vec::new();
    for index in 0..len {
        let Some(&value) = dyn_slice(handle).get(index) else {
            break;
        };
        if p(value) {
            kept.push(value);
        }
    }
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
    // Indexed, re-dereferencing the handle each step: `f`/`p` re-enters generated
    // code, which can push to *this* list (reallocating its buffer) or raise and
    // longjmp past a borrow, so none may be held across the call. Re-deref rather
    // than a `to_vec()` snapshot — this is the native HOF hot path the perf gate
    // measures.
    let len = dyn_slice(handle).len();
    let mut acc = init;
    for index in 0..len {
        let Some(&value) = dyn_slice(handle).get(index) else {
            break;
        };
        acc = f(acc, value);
    }
    acc
}

/// `xs.map(f)` where `f` is a closure *value* rather than a compiled address.
///
/// The three `*_fn` helpers above take a raw function pointer, which is only
/// available when the lowering knows which lambda the callback register names.
/// A callback read out of a container or passed through a parameter is a
/// `DYN_CLOSURE`, and these three are the same folds called through it.
///
/// # Safety
/// `handle` must be a live dyn-list handle (or null); `callee` a `DYN_CLOSURE`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_map_closure(handle: *mut c_void, callee: LkDyn) -> *mut c_void {
    // Indexed, re-dereferencing the handle each step, for the reason the `*_fn`
    // helpers document: the callback re-enters generated code.
    let len = dyn_slice(handle).len();
    let mut mapped: Vec<LkDyn> = Vec::with_capacity(len);
    for index in 0..len {
        let Some(&value) = dyn_slice(handle).get(index) else {
            break;
        };
        // SAFETY: as documented.
        mapped.push(unsafe { crate::lkclosure::call_with(callee, &mut alloc::vec![value]) });
    }
    arena_handle(mapped)
}

/// `xs.filter(p)` with a closure value.
///
/// The predicate's result is judged the way the interpreter judges it
/// (`core_methods::list_filter`): a `Bool` is itself, `nil` is false, anything
/// else is true. The `*_fn` path cannot do that — it demands a `Bool`-returning
/// callback at compile time — but a closure's return type is not known here.
///
/// # Safety
/// As [`lkrt_lklist_dyn_map_closure`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_filter_closure(handle: *mut c_void, callee: LkDyn) -> *mut c_void {
    let len = dyn_slice(handle).len();
    let mut kept: Vec<LkDyn> = Vec::new();
    for index in 0..len {
        let Some(&value) = dyn_slice(handle).get(index) else {
            break;
        };
        // SAFETY: as documented.
        let verdict = unsafe { crate::lkclosure::call_with(callee, &mut alloc::vec![value]) };
        let keep = match verdict.tag {
            DYN_BOOL => verdict.payload != 0,
            DYN_NIL => false,
            _ => true,
        };
        if keep {
            kept.push(value);
        }
    }
    arena_handle(kept)
}

/// `xs.reduce(init, f)` with a closure value.
///
/// # Safety
/// As [`lkrt_lklist_dyn_map_closure`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_reduce_closure(handle: *mut c_void, init: LkDyn, callee: LkDyn) -> LkDyn {
    let len = dyn_slice(handle).len();
    let mut acc = init;
    for index in 0..len {
        let Some(&value) = dyn_slice(handle).get(index) else {
            break;
        };
        // SAFETY: as documented.
        acc = unsafe { crate::lkclosure::call_with(callee, &mut alloc::vec![acc, value]) };
    }
    acc
}

/// `xs.chunk(size)` — split into `size`-element groups, last group short.
/// `size <= 0` is a VM error (loud failure).
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_chunk(handle: *mut c_void, size: i64) -> *mut c_void {
    if size <= 0 {
        crate::rt_eprintln!("list.chunk() size must be positive");
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

/// `xs.unique()` — order-preserving dedup under `==`. O(n²), like the VM's
/// mixed-list path.
///
/// This used to call a `unique_eq` of its own: numerics by `to_bits`, strings
/// "never equal" past seven bytes, lists and maps by handle. That mirrored the
/// VM *of the time*; once the VM's equality became heap-aware, the two drifted
/// apart with nothing to catch it — `[s, s].unique()` and `[[1], [1]].unique()`
/// answered differently on the two backends, and the differential corpus
/// deliberately did not cover them.
/// # Safety
/// `handle` must be a live handle from [`lkrt_lklist_dyn_new`], or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_lklist_dyn_unique(handle: *mut c_void) -> *mut c_void {
    let mut unique: Vec<LkDyn> = Vec::new();
    for &item in dyn_slice(handle) {
        if !unique.iter().any(|&seen| dyn_eq_inner(seen, item)) {
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
        if is_list_tag(item.tag) {
            flat.extend_from_slice(&dyn_list_values(item));
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
        assert_eq!(lkrt_dyn_from_maybe_str(core::ptr::null(), 0).tag, DYN_NIL);
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
        // `/` yields a Float, even for two Ints — the rule the checker always
        // stated and that both executors now implement.
        let div = lkrt_dyn_div(lkrt_dyn_from_i64(20), lkrt_dyn_from_i64(4));
        assert_eq!(div.tag, DYN_F64);
        assert_eq!(div.f64_value(), 5.0);
        let fractional = lkrt_dyn_div(lkrt_dyn_from_i64(7), lkrt_dyn_from_i64(2));
        assert_eq!(fractional.f64_value(), 3.5);
        // And `f64` division by zero is an infinity rather than a raise.
        let infinite = lkrt_dyn_div(lkrt_dyn_from_i64(1), lkrt_dyn_from_i64(0));
        assert_eq!(infinite.tag, DYN_F64);
        assert!(infinite.f64_value().is_infinite());
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
        // Comma-separated no spaces; `2.0` → "2" (Rust to_string); a string
        // inside a container is `{:?}`-quoted, whatever the container's
        // representation is. This asserted the bare form, mirroring a VM quirk
        // where a *mixed* list rendered strings bare and a typed string list
        // quoted them — one value, two renderings, decided by an internal
        // representation no program can see.
        assert_eq!(text(unsafe { lkrt_lklist_dyn_display(xs) }), "[1,\"b c\",2,true,nil]");
        assert_eq!(text(unsafe { lkrt_dyn_display(s("b c")) }), "b c");
        assert_eq!(text(unsafe { lkrt_dyn_display_quoted(s("b c")) }), "\"b c\"");
    }

    /// `unique()` dedups by `==`, like everything else.
    ///
    /// This test used to pin a `unique_eq` of its own — numerics by `to_bits`,
    /// strings "never equal" past seven bytes, lists by handle — described as
    /// "VM handle semantics". It *was* the VM's rule once; the VM's equality
    /// later became heap-aware and this did not follow, so the two backends
    /// disagreed about `[s, s].unique()` and `[[1], [1]].unique()` with nothing
    /// to catch it. There is one equality now.
    #[test]
    fn unique_dedups_by_the_same_equality_as_everything_else() {
        // Numerics by value: `1 == 1.0` dedups, and so do the two zeros.
        assert!(dyn_eq_inner(lkrt_dyn_from_i64(1), lkrt_dyn_from_f64(1.0)));
        assert!(dyn_eq_inner(lkrt_dyn_from_f64(0.0), lkrt_dyn_from_f64(-0.0)));
        // …and no NaN equals any NaN, so a list of them never dedups.
        assert!(!dyn_eq_inner(lkrt_dyn_from_f64(f64::NAN), lkrt_dyn_from_f64(f64::NAN)));
        // Strings by content, at any length.
        assert!(dyn_eq_inner(s("ab"), s("ab")));
        assert!(dyn_eq_inner(s("longer-than-seven"), s("longer-than-seven")));
        // Lists structurally, not by handle.
        let xs = lkrt_lklist_dyn_new();
        let ys = lkrt_lklist_dyn_new();
        unsafe {
            lkrt_lklist_dyn_push(xs, lkrt_dyn_from_i64(7));
            lkrt_lklist_dyn_push(ys, lkrt_dyn_from_i64(7));
        }
        assert!(dyn_eq_inner(lkrt_dyn_from_list(xs), lkrt_dyn_from_list(xs)));
        assert!(dyn_eq_inner(lkrt_dyn_from_list(xs), lkrt_dyn_from_list(ys)));
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

    /// A marked struct renders `Name{f:v,…}` in declaration order, with nested
    /// values quoted — and a **nested struct** renders as a struct, which is the
    /// whole reason the type description lives here rather than at the display
    /// site (see `docs/aot/aot-gaps-and-lkrt.md`).
    #[test]
    fn a_marked_struct_displays_like_the_vm() {
        // struct P { name: String, v: Int }
        unsafe {
            lkrt_struct_type_begin(101, c"P".as_ptr());
            lkrt_struct_type_field(101, c"name".as_ptr());
            lkrt_struct_type_field(101, c"v".as_ptr());
            // struct Outer { inner: P, tag: String }
            lkrt_struct_type_begin(102, c"Outer".as_ptr());
            lkrt_struct_type_field(102, c"inner".as_ptr());
            lkrt_struct_type_field(102, c"tag".as_ptr());
        }

        let inner = crate::lkmap::lkrt_lkmap_str_dyn_new();
        unsafe {
            crate::lkmap::lkrt_lkmap_str_dyn_set(inner, c"name".as_ptr(), s("a, b"));
            crate::lkmap::lkrt_lkmap_str_dyn_set(inner, c"v".as_ptr(), lkrt_dyn_from_i64(-3));
        }
        lkrt_lkmap_obj_mark(inner, 101);
        let inner_dyn = lkrt_dyn_from_map(inner);
        assert_eq!(
            text(unsafe { lkrt_dyn_display(inner_dyn) }),
            r#"P{name:"a, b",v:-3}"#,
            "declaration order, string field quoted"
        );

        let outer = crate::lkmap::lkrt_lkmap_str_dyn_new();
        unsafe {
            crate::lkmap::lkrt_lkmap_str_dyn_set(outer, c"inner".as_ptr(), inner_dyn);
            crate::lkmap::lkrt_lkmap_str_dyn_set(outer, c"tag".as_ptr(), s("x"));
        }
        lkrt_lkmap_obj_mark(outer, 102);
        assert_eq!(
            text(unsafe { lkrt_dyn_display(lkrt_dyn_from_map(outer)) }),
            r#"Outer{inner:P{name:"a, b",v:-3},tag:"x"}"#,
            "a nested struct is a struct, not a hash-ordered map"
        );

        // An unmarked map is still a map: order is the layout's, and that is
        // deliberately outside the lowering subset.
        let plain = crate::lkmap::lkrt_lkmap_str_dyn_new();
        unsafe { crate::lkmap::lkrt_lkmap_str_dyn_set(plain, c"k".as_ptr(), lkrt_dyn_from_i64(1)) };
        assert_eq!(
            text(unsafe { lkrt_dyn_display(lkrt_dyn_from_map(plain)) }),
            r#"{"k":1}"#
        );
    }

    /// `in` compares a heap value by handle — every heap carrier, not two.
    ///
    /// `contains_eq`'s catch-all answered `false`, so a `Set`, a `Bytes`, a
    /// window or a typed map was never in any list:
    ///
    /// ```lk
    /// let b = "ab".bytes();
    /// let xs = [b];
    /// b in xs            // true interpreted, false compiled
    /// ```
    ///
    /// These four box *in place*, so the payload is the same handle the VM
    /// compares — the existing arm was already right for them. They were added
    /// to the tag space and this match was not revisited, which is the failure
    /// mode a catch-all arm has: a new tag joins the "not equal to anything"
    /// bucket silently.
    #[test]
    fn every_heap_carrier_is_found_by_handle() {
        // SAFETY: both pointers are live NUL-terminated literals.
        let (bytes, other_bytes) = unsafe {
            (
                crate::lkbytes::lkrt_lkbytes_from_str(c"ab".as_ptr()),
                crate::lkbytes::lkrt_lkbytes_from_str(c"cd".as_ptr()),
            )
        };
        let set = crate::lkset::lkrt_lkset_new();
        let slice_src = crate::lklist::lkrt_lklist_i64_new();
        let window = unsafe { crate::lkslice::lkrt_lkslice_i64_new(slice_src, 0, 0) };
        let tmap = crate::lkmap::lkrt_lkmap_str_i64_new();

        for boxed in [
            lkrt_dyn_from_bytes(bytes),
            lkrt_dyn_from_set(set),
            lkrt_dyn_from_slice(window),
            lkrt_dyn_from_typed_map(tmap, crate::lkmap::KIND_STR_I64),
        ] {
            assert!(
                contains_eq(boxed, boxed),
                "tag {} must find itself by handle",
                boxed.tag
            );
        }

        // …and a *different* handle of the same carrier is still not it.
        assert!(!contains_eq(
            lkrt_dyn_from_bytes(bytes),
            lkrt_dyn_from_bytes(other_bytes)
        ));
    }
}
