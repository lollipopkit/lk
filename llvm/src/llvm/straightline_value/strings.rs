use crate::{
    val::ShortStr,
    vm::{ConstHeapValueData, ConstRuntimeValueData},
};

use super::NativeStringKeyKind;

pub(in crate::llvm) fn native_const_runtime_string(value: ConstRuntimeValueData) -> Option<String> {
    match value {
        ConstRuntimeValueData::ShortStr(value) => Some(value),
        ConstRuntimeValueData::Heap(value) => match *value {
            ConstHeapValueData::LongString(value) => Some(value),
            _ => None,
        },
        _ => None,
    }
}

pub(in crate::llvm) fn native_runtime_string_key_kind(value: &str) -> NativeStringKeyKind {
    if ShortStr::new(value).is_some() {
        NativeStringKeyKind::Short
    } else {
        NativeStringKeyKind::Heap
    }
}

pub(super) fn native_const_string_value(value: &str) -> ConstRuntimeValueData {
    if ShortStr::new(value).is_some() {
        ConstRuntimeValueData::ShortStr(value.to_string())
    } else {
        ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::LongString(value.to_string())))
    }
}
