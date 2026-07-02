//! LLVM-side view of the native ABI schema.
//!
//! The schema itself (types, table, version) lives in the dependency-free
//! `lk-aot-abi` crate so codegen and `lkrt` share a single source of truth. This
//! module keeps the crate-local names the rest of the LLVM backend already uses
//! and owns the LLVM-specific rendering (the `declare` text), which is the only
//! part of the ABI that knows about LLVM syntax.

#![allow(dead_code)]

pub(in crate::llvm) use lk_aot_abi::{
    ABI_FUNCTIONS as NATIVE_INTRINSICS, ABI_VERSION, AbiFn as NativeIntrinsic, AbiType as NativeIntrinsicType,
};

/// Looks up a native intrinsic by its `(module, name)` identity.
pub(in crate::llvm) fn native_intrinsic(module: &str, name: &str) -> Option<&'static NativeIntrinsic> {
    lk_aot_abi::find(module, name)
}

/// Renders `declare` lines for every `lkrt_`-exported ABI function, so the
/// generated module can call them. Symbols that are not `lkrt_` (e.g. libc) are
/// declared elsewhere.
pub(in crate::llvm) fn native_intrinsic_declarations() -> String {
    let mut declarations = String::new();
    for intrinsic in NATIVE_INTRINSICS {
        if !intrinsic.symbol.starts_with("lkrt_") {
            continue;
        }
        declarations.push_str("declare ");
        declarations.push_str(llvm_type(intrinsic.result));
        declarations.push_str(" @");
        declarations.push_str(intrinsic.symbol);
        declarations.push('(');
        for (index, param) in intrinsic.params.iter().enumerate() {
            if index > 0 {
                declarations.push_str(", ");
            }
            declarations.push_str(llvm_type(*param));
        }
        declarations.push_str(")\n");
    }
    declarations.push('\n');
    declarations
}

fn llvm_type(value: NativeIntrinsicType) -> &'static str {
    match value {
        NativeIntrinsicType::I64 => "i64",
        NativeIntrinsicType::F64 => "double",
        NativeIntrinsicType::Ptr | NativeIntrinsicType::StrPtr => "ptr",
        NativeIntrinsicType::Nil => "void",
    }
}
