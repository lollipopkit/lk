use crate::{
    llvm::straightline_value::{NativeListElementKind, NativeStraightlineValue, native_static_list_push},
    vm::{ConstRuntimeValueData, Instr},
};

use super::{
    NativeScalarKind, analysis::native_dynamic_text_len_supported, arg_lists::collected_arg_list_push,
    slots::set_static_value,
};

pub(super) fn propagate_list_push(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    instr: Instr,
) -> Option<()> {
    let target = static_values.get(instr.a() as usize).and_then(Clone::clone)?;
    match target {
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        } => {
            if let Some((first, second)) = dynamic_pair_list_kinds(static_value(static_values, instr.b()).as_ref()) {
                set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    Some(NativeScalarKind::I64),
                    NativeStraightlineValue::DynamicPairList { id, first, second },
                )
                .then_some(())
            } else if let Some(value) = collected_arg_list_push(static_value(static_values, instr.b())) {
                set_static_value(kinds, static_values, instr.a(), Some(NativeScalarKind::I64), value).then_some(())
            } else if native_kind(kinds, instr.b()) == Some(NativeScalarKind::I64) {
                set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    Some(NativeScalarKind::I64),
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::I64,
                    },
                )
                .then_some(())
            } else if native_kind(kinds, instr.b()) == Some(NativeScalarKind::F64) {
                set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    Some(NativeScalarKind::I64),
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::F64,
                    },
                )
                .then_some(())
            } else if native_kind(kinds, instr.b()) == Some(NativeScalarKind::Bool) {
                set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    Some(NativeScalarKind::I64),
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::Bool,
                    },
                )
                .then_some(())
            } else {
                let value = static_value(static_values, instr.b())?;
                let element = if native_kind(kinds, instr.b()) == Some(NativeScalarKind::StrPtr)
                    && matches!(
                        value,
                        NativeStraightlineValue::Text(_) | NativeStraightlineValue::StringPtr(_)
                    ) {
                    NativeListElementKind::StrPtr
                } else {
                    NativeListElementKind::Text
                };
                (native_dynamic_text_len_supported(&value)
                    && set_static_value(
                        kinds,
                        static_values,
                        instr.a(),
                        None,
                        NativeStraightlineValue::DynamicList { id, element },
                    ))
                .then_some(())
            }
        }
        NativeStraightlineValue::DynamicPairList { id, first, second } => {
            let Some((next_first, next_second)) =
                dynamic_pair_list_kinds(static_value(static_values, instr.b()).as_ref())
            else {
                return None;
            };
            (first == next_first
                && second == next_second
                && set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    Some(NativeScalarKind::I64),
                    NativeStraightlineValue::DynamicPairList { id, first, second },
                ))
            .then_some(())
        }
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::F64,
        } => (native_kind(kinds, instr.b()) == Some(NativeScalarKind::F64)
            && set_static_value(
                kinds,
                static_values,
                instr.a(),
                Some(NativeScalarKind::I64),
                NativeStraightlineValue::DynamicList {
                    id,
                    element: NativeListElementKind::F64,
                },
            ))
        .then_some(()),
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Bool,
        } => (native_kind(kinds, instr.b()) == Some(NativeScalarKind::Bool)
            && set_static_value(
                kinds,
                static_values,
                instr.a(),
                Some(NativeScalarKind::I64),
                NativeStraightlineValue::DynamicList {
                    id,
                    element: NativeListElementKind::Bool,
                },
            ))
        .then_some(()),
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::Text,
        } => {
            let value = static_value(static_values, instr.b())?;
            (native_dynamic_text_len_supported(&value)
                && set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    None,
                    NativeStraightlineValue::DynamicList {
                        id,
                        element: NativeListElementKind::Text,
                    },
                ))
            .then_some(())
        }
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::StrPtr,
        } => (native_kind(kinds, instr.b()) == Some(NativeScalarKind::StrPtr)
            && set_static_value(
                kinds,
                static_values,
                instr.a(),
                None,
                NativeStraightlineValue::DynamicList {
                    id,
                    element: NativeListElementKind::StrPtr,
                },
            ))
        .then_some(()),
        target => {
            if let NativeStraightlineValue::ArgList { mut elements } = target {
                let value = static_value(static_values, instr.b())?;
                elements.push(value);
                return set_static_value(
                    kinds,
                    static_values,
                    instr.a(),
                    None,
                    NativeStraightlineValue::ArgList { elements },
                )
                .then_some(());
            }
            if matches!(target, NativeStraightlineValue::List { ref elements, .. } if elements.is_empty())
                && let Some(value) = collected_arg_list_push(static_value(static_values, instr.b()))
            {
                return set_static_value(kinds, static_values, instr.a(), Some(NativeScalarKind::I64), value)
                    .then_some(());
            }
            let value = static_value(static_values, instr.b())?;
            let value = native_static_list_push(target, value)?;
            set_static_value(kinds, static_values, instr.a(), None, value).then_some(())
        }
    }
}

fn native_kind(kinds: &[Option<NativeScalarKind>], reg: u8) -> Option<NativeScalarKind> {
    kinds.get(reg as usize).copied().flatten()
}

fn static_value(values: &[Option<NativeStraightlineValue>], reg: u8) -> Option<NativeStraightlineValue> {
    values.get(reg as usize).and_then(Clone::clone)
}

fn dynamic_pair_list_kinds(
    value: Option<&NativeStraightlineValue>,
) -> Option<(NativeListElementKind, NativeListElementKind)> {
    let (first, second) = match value? {
        NativeStraightlineValue::ArgList { elements } => {
            let [first, second] = elements.as_slice() else {
                return None;
            };
            (dynamic_pair_field_kind(first)?, dynamic_pair_field_kind(second)?)
        }
        NativeStraightlineValue::List { elements, .. } => {
            let [first, second] = elements.as_slice() else {
                return None;
            };
            (
                dynamic_pair_const_field_kind(first)?,
                dynamic_pair_const_field_kind(second)?,
            )
        }
        _ => return None,
    };
    dynamic_pair_fields_have_distinct_storage(first, second).then_some((first, second))
}

fn dynamic_pair_field_kind(value: &NativeStraightlineValue) -> Option<NativeListElementKind> {
    match value {
        NativeStraightlineValue::I64(_) => Some(NativeListElementKind::I64),
        NativeStraightlineValue::Bool(_) => Some(NativeListElementKind::Bool),
        NativeStraightlineValue::F64(_) => Some(NativeListElementKind::F64),
        NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_) => {
            Some(NativeListElementKind::StrPtr)
        }
        _ => None,
    }
}

fn dynamic_pair_const_field_kind(value: &ConstRuntimeValueData) -> Option<NativeListElementKind> {
    match value {
        ConstRuntimeValueData::Int(_) => Some(NativeListElementKind::I64),
        ConstRuntimeValueData::Bool(_) => Some(NativeListElementKind::Bool),
        ConstRuntimeValueData::Float(_) => Some(NativeListElementKind::F64),
        _ => None,
    }
}

fn dynamic_pair_fields_have_distinct_storage(first: NativeListElementKind, second: NativeListElementKind) -> bool {
    match (first, second) {
        (
            NativeListElementKind::StrPtr,
            NativeListElementKind::I64 | NativeListElementKind::Bool | NativeListElementKind::F64,
        )
        | (NativeListElementKind::I64 | NativeListElementKind::Bool, NativeListElementKind::F64)
        | (NativeListElementKind::F64, NativeListElementKind::I64 | NativeListElementKind::Bool)
        | (NativeListElementKind::F64, NativeListElementKind::StrPtr)
        | (NativeListElementKind::I64 | NativeListElementKind::Bool, NativeListElementKind::StrPtr) => true,
        _ => false,
    }
}
