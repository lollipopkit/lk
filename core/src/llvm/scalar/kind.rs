//! Scalar type kind and facts types for LLVM native scalar block compilation.
//!
//! Extracted from facts.rs to keep individual files under the 1500-line
//! limit. Contains the type kind enum, the per-block facts container, and
//! their inherent methods.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::llvm) enum NativeScalarKind {
    I64,
    F64,
    Bool,
    Nil,
    StrPtr,
    MaybeI64,
}

pub(in crate::llvm) struct NativeScalarFacts {
    pub(in crate::llvm) registers_before: Vec<Vec<Option<NativeScalarKind>>>,
    pub(in crate::llvm) globals_before: Vec<Vec<Option<NativeScalarKind>>>,
}

impl NativeScalarFacts {
    pub(in crate::llvm) fn register_kind_before(&self, pc: usize, reg: u8) -> Option<NativeScalarKind> {
        self.registers_before
            .get(pc)
            .and_then(|kinds| kinds.get(reg as usize))
            .copied()
            .flatten()
    }

    pub(in crate::llvm) fn global_kind_before(&self, pc: usize, slot: u16) -> Option<NativeScalarKind> {
        self.globals_before
            .get(pc)
            .and_then(|kinds| kinds.get(slot as usize))
            .copied()
            .flatten()
    }

    pub(in crate::llvm) fn global_kinds_before(&self, pc: usize) -> Option<&[Option<NativeScalarKind>]> {
        self.globals_before.get(pc).map(Vec::as_slice)
    }
}

impl NativeScalarKind {
    pub(in crate::llvm) const fn llvm_type(self) -> &'static str {
        match self {
            Self::F64 => "double",
            Self::StrPtr => "ptr",
            Self::I64 | Self::Bool | Self::Nil | Self::MaybeI64 => "i64",
        }
    }

    pub(in crate::llvm) const fn is_numeric(self) -> bool {
        matches!(self, Self::I64 | Self::F64)
    }
}
