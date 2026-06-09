//! Registry for host primitives that LLVM AOT may call through `lkrt`.
//!
//! This is metadata only. Full stdlib method bodies should live in LK stdlib
//! source or in compile-time constant evaluation, not as scattered LLVM matches.

#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::llvm) enum NativeIntrinsicEffect {
    Pure,
    ReadsHost,
    WritesHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::llvm) enum NativeIntrinsicType {
    I64,
    F64,
    StrPtr,
    Nil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::llvm) struct NativeIntrinsic {
    pub module: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub params: &'static [NativeIntrinsicType],
    pub result: NativeIntrinsicType,
    pub effect: NativeIntrinsicEffect,
}

pub(in crate::llvm) const NATIVE_INTRINSICS: &[NativeIntrinsic] = &[
    NativeIntrinsic {
        module: "os",
        name: "clock",
        symbol: "lkrt_os_clock",
        params: &[],
        result: NativeIntrinsicType::F64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "os",
        name: "epoch",
        symbol: "lkrt_os_epoch",
        params: &[],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "time",
        name: "sleep",
        symbol: "lkrt_time_sleep_ms",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::Nil,
        effect: NativeIntrinsicEffect::WritesHost,
    },
];

pub(in crate::llvm) fn native_intrinsic(module: &str, name: &str) -> Option<&'static NativeIntrinsic> {
    NATIVE_INTRINSICS
        .iter()
        .find(|intrinsic| intrinsic.module == module && intrinsic.name == name)
}
