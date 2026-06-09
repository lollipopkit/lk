use crate::llvm::straightline_value::NativeStraightlineValue;

use super::NativeScalarKind;

/// Generate a symbolic static value for a register whose kind is known but concrete value is not.
pub(in crate::llvm) fn kind_symbolic_value(kind: NativeScalarKind, reg: u8) -> NativeStraightlineValue {
    match kind {
        NativeScalarKind::I64 => NativeStraightlineValue::I64(format!("%hint_r{reg}")),
        NativeScalarKind::F64 => NativeStraightlineValue::F64(format!("%hint_r{reg}")),
        NativeScalarKind::Bool => NativeStraightlineValue::Bool("0".to_string()),
        NativeScalarKind::Nil => NativeStraightlineValue::Nil,
        NativeScalarKind::StrPtr => NativeStraightlineValue::StringPtr(format!("%hint_str_r{reg}")),
        NativeScalarKind::MaybeI64 => NativeStraightlineValue::I64(format!("%hint_maybe_r{reg}")),
        NativeScalarKind::MaybeStrPtr => NativeStraightlineValue::StringPtr(format!("%hint_maybe_str_r{reg}")),
    }
}
