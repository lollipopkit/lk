//! Conformance of lkrt's exported symbols against the shared ABI schema
//! (single source of truth, `docs/llvm/aot-redesign.md` §3.3).
//!
//! Both this module and `lk_aot_abi::ABI_FUNCTIONS` expand from the same
//! `for_each_abi_fn!` data macro, so every schema entry is checked here by
//! construction:
//! - the symbol must exist in this crate, be `extern "C"`, and have the exact
//!   arity — enforced at *compile time* by a fn-pointer coercion;
//! - each parameter/return must be in the same ABI register class (i64 / f64 /
//!   pointer / void) as the schema claims — enforced by the test below.
//!
//! `StrPtr` vs `Ptr` are deliberately the same class: both render as an opaque
//! LLVM `ptr` and are calling-convention-identical; the distinction in the
//! schema is documentation, not ABI.

use lk_aot_abi::{ABI_FUNCTIONS, AbiType, for_each_abi_fn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    I64,
    F64,
    Ptr,
    Nil,
    DynVal,
}

trait ClassOf {
    const CLASS: Class;
}
impl ClassOf for i64 {
    const CLASS: Class = Class::I64;
}
impl ClassOf for f64 {
    const CLASS: Class = Class::F64;
}
impl ClassOf for () {
    const CLASS: Class = Class::Nil;
}
impl ClassOf for crate::LkDyn {
    const CLASS: Class = Class::DynVal;
}
impl<T> ClassOf for *const T {
    const CLASS: Class = Class::Ptr;
}
impl<T> ClassOf for *mut T {
    const CLASS: Class = Class::Ptr;
}
// Compiled-callback pointers (list HOF): pointer-class like any other ptr.
impl ClassOf for extern "C" fn(i64) -> i64 {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(i64) -> bool {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(i64, i64) -> i64 {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(crate::LkDyn) -> crate::LkDyn {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(crate::LkDyn) -> bool {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(crate::LkDyn, crate::LkDyn) -> crate::LkDyn {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(*const core::ffi::c_char) -> *const core::ffi::c_char {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(*const core::ffi::c_char) -> bool {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(*mut core::ffi::c_void) -> crate::LkDyn {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn() -> crate::LkDyn {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(crate::LkDyn, crate::LkDyn, crate::LkDyn) -> crate::LkDyn {
    const CLASS: Class = Class::Ptr;
}
impl ClassOf for extern "C" fn(crate::LkDyn, crate::LkDyn, crate::LkDyn, crate::LkDyn) -> crate::LkDyn {
    const CLASS: Class = Class::Ptr;
}

trait FnClasses {
    fn classes(&self) -> (Vec<Class>, Class);
}

macro_rules! impl_fn_classes {
    ($($a:ident),*) => {
        impl<$($a: ClassOf,)* R: ClassOf> FnClasses for unsafe extern "C" fn($($a),*) -> R {
            fn classes(&self) -> (Vec<Class>, Class) {
                (vec![$($a::CLASS),*], R::CLASS)
            }
        }
    };
}
impl_fn_classes!();
impl_fn_classes!(A1);
impl_fn_classes!(A1, A2);
impl_fn_classes!(A1, A2, A3);
impl_fn_classes!(A1, A2, A3, A4);
impl_fn_classes!(A1, A2, A3, A4, A5);
impl_fn_classes!(A1, A2, A3, A4, A5, A6);
impl_fn_classes!(A1, A2, A3, A4, A5, A6, A7);
impl_fn_classes!(A1, A2, A3, A4, A5, A6, A7, A8);
impl_fn_classes!(A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_fn_classes!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_fn_classes!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
impl_fn_classes!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12);

fn abi_class(ty: AbiType) -> Class {
    match ty {
        AbiType::I64 => Class::I64,
        AbiType::F64 => Class::F64,
        AbiType::Ptr | AbiType::StrPtr => Class::Ptr,
        AbiType::Nil => Class::Nil,
        AbiType::DynVal => Class::DynVal,
    }
}

macro_rules! underscore {
    ($t:ident) => {
        _
    };
}

macro_rules! collect_impl_signatures {
    ($( ($module:literal, $name:literal, $symbol:ident, $effect:ident, [$($param:ident),* $(,)?], $ret:ident) );* $(;)?) => {
        fn impl_signatures() -> Vec<(&'static str, Vec<Class>, Class)> {
            let mut sigs = Vec::new();
            $(
                {
                    // The coercion is the compile-time check: the symbol must
                    // exist, be `extern "C"`, and have exactly this arity.
                    let f: unsafe extern "C" fn($(underscore!($param)),*) -> _ = crate::$symbol;
                    let (params, ret) = f.classes();
                    sigs.push((stringify!($symbol), params, ret));
                }
            )*
            sigs
        }
    };
}

for_each_abi_fn!(collect_impl_signatures);

#[test]
fn every_schema_entry_matches_its_implementation() {
    let sigs = impl_signatures();
    assert_eq!(
        sigs.len(),
        ABI_FUNCTIONS.len(),
        "schema and conformance list expand from the same macro; lengths must agree"
    );
    for (abi, (symbol, params, ret)) in ABI_FUNCTIONS.iter().zip(sigs) {
        assert_eq!(abi.symbol, symbol, "table order drifted");
        let want: Vec<Class> = abi.params.iter().copied().map(abi_class).collect();
        assert_eq!(params, want, "`{symbol}` parameter classes drifted from the ABI schema");
        assert_eq!(
            ret,
            abi_class(abi.result),
            "`{symbol}` return class drifted from the ABI schema"
        );
    }
}
