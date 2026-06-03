use crate::{
    val::ShortStr,
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data},
};

use super::NativeStringKeyKind;

pub(in crate::llvm) fn native_const_runtime_string(value: ConstRuntimeValue32Data) -> Option<String> {
    match value {
        ConstRuntimeValue32Data::ShortStr(value) => Some(value),
        ConstRuntimeValue32Data::Heap(value) => match *value {
            ConstHeapValue32Data::LongString(value) => Some(value),
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

pub(super) fn native_const_string_value(value: &str) -> ConstRuntimeValue32Data {
    if ShortStr::new(value).is_some() {
        ConstRuntimeValue32Data::ShortStr(value.to_string())
    } else {
        ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::LongString(value.to_string())))
    }
}
