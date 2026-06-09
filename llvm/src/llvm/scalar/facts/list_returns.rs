use crate::{
    llvm::scalar::list_shape::function_returns_pushed_list,
    llvm::straightline_value::{NativeListElementKind, NativeStraightlineValue},
    vm::{ConstHeapValueData, ConstRuntimeValueData, FunctionData},
};

pub(in crate::llvm) fn dynamic_list_return_value(
    function: &FunctionData,
    args: &[Option<NativeStraightlineValue>],
    id: usize,
) -> Option<NativeStraightlineValue> {
    if !callee_has_list_return_shape(function) {
        return None;
    }
    let element = args
        .iter()
        .filter_map(|arg| arg.as_ref().and_then(value_element_kind))
        .next()
        .unwrap_or(NativeListElementKind::StrPtr);
    Some(NativeStraightlineValue::DynamicList { id, element })
}

fn callee_has_list_return_shape(function: &FunctionData) -> bool {
    function_returns_pushed_list(function)
}

fn value_element_kind(value: &NativeStraightlineValue) -> Option<NativeListElementKind> {
    match value {
        NativeStraightlineValue::DynamicList { element, .. } => Some(*element),
        NativeStraightlineValue::List { elements, .. } => const_list_element_kind(elements),
        NativeStraightlineValue::I64(_) => Some(NativeListElementKind::I64),
        NativeStraightlineValue::Bool(_) => Some(NativeListElementKind::Bool),
        NativeStraightlineValue::F64(_) => Some(NativeListElementKind::F64),
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::DynamicTextChar => Some(NativeListElementKind::StrPtr),
        _ => None,
    }
}

fn const_list_element_kind(elements: &[ConstRuntimeValueData]) -> Option<NativeListElementKind> {
    if elements
        .iter()
        .all(|value| matches!(value, ConstRuntimeValueData::Int(_)))
    {
        return Some(NativeListElementKind::I64);
    }
    if elements
        .iter()
        .all(|value| matches!(value, ConstRuntimeValueData::Float(_)))
    {
        return Some(NativeListElementKind::F64);
    }
    if elements
        .iter()
        .all(|value| matches!(value, ConstRuntimeValueData::Bool(_)))
    {
        return Some(NativeListElementKind::Bool);
    }
    if elements.iter().all(|value| match value {
        ConstRuntimeValueData::ShortStr(_) => true,
        ConstRuntimeValueData::Heap(value) => matches!(value.as_ref(), ConstHeapValueData::LongString(_)),
        _ => false,
    }) {
        return Some(NativeListElementKind::StrPtr);
    }
    None
}
