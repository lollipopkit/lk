use crate::llvm::straightline_value::NativeStraightlineValue;

use super::NativeScalarKind;

pub(super) fn set_native_kind(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    reg: u8,
    kind: NativeScalarKind,
) -> bool {
    let Some(slot) = kinds.get_mut(reg as usize) else {
        return false;
    };
    *slot = Some(kind);
    if let Some(value) = static_values.get_mut(reg as usize) {
        *value = None;
    }
    true
}

pub(super) fn set_static_value(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    reg: u8,
    value_kind: Option<NativeScalarKind>,
    value: NativeStraightlineValue,
) -> bool {
    let Some(slot) = static_values.get_mut(reg as usize) else {
        return false;
    };
    *slot = Some(value);
    if let Some(kind) = kinds.get_mut(reg as usize) {
        *kind = value_kind;
    }
    true
}

pub(super) fn native_global_kind(kinds: &[Option<NativeScalarKind>], slot: u16) -> Option<NativeScalarKind> {
    kinds.get(slot as usize).copied().flatten()
}

pub(super) fn set_native_global_kind(
    kinds: &mut [Option<NativeScalarKind>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    slot: u16,
    kind: NativeScalarKind,
) -> bool {
    let index = slot as usize;
    let Some(slot) = kinds.get_mut(index) else {
        return false;
    };
    *slot = Some(kind);
    if let Some(value) = static_globals.get_mut(index) {
        *value = None;
    }
    true
}

pub(super) fn set_static_global(
    static_globals: &mut [Option<NativeStraightlineValue>],
    kinds: &mut [Option<NativeScalarKind>],
    slot: u16,
    value: NativeStraightlineValue,
) -> bool {
    let Some(static_slot) = static_globals.get_mut(slot as usize) else {
        return false;
    };
    *static_slot = Some(value);
    if let Some(kind) = kinds.get_mut(slot as usize) {
        *kind = None;
    }
    true
}
