use crate::{
    llvm::straightline_value::{NativeListElementKind, NativeStraightlineValue},
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Function32Data, Instr32, Opcode32},
};

pub(in crate::llvm) fn dynamic_list_return_value(
    function: &Function32Data,
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

fn callee_has_list_return_shape(function: &Function32Data) -> bool {
    function
        .code
        .iter()
        .copied()
        .filter_map(|raw| Instr32::try_from_raw(raw).ok())
        .any(|instr| instr.opcode() == Opcode32::ListPush)
}

fn value_element_kind(value: &NativeStraightlineValue) -> Option<NativeListElementKind> {
    match value {
        NativeStraightlineValue::DynamicList { element, .. } => Some(*element),
        NativeStraightlineValue::List { elements, .. } => const_list_element_kind(elements),
        NativeStraightlineValue::I64(_) => Some(NativeListElementKind::I64),
        NativeStraightlineValue::F64(_) => Some(NativeListElementKind::F64),
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::DynamicTextChar => Some(NativeListElementKind::StrPtr),
        _ => None,
    }
}

fn const_list_element_kind(elements: &[ConstRuntimeValue32Data]) -> Option<NativeListElementKind> {
    if elements
        .iter()
        .all(|value| matches!(value, ConstRuntimeValue32Data::Int(_)))
    {
        return Some(NativeListElementKind::I64);
    }
    if elements
        .iter()
        .all(|value| matches!(value, ConstRuntimeValue32Data::Float(_)))
    {
        return Some(NativeListElementKind::F64);
    }
    if elements.iter().all(|value| match value {
        ConstRuntimeValue32Data::ShortStr(_) => true,
        ConstRuntimeValue32Data::Heap(value) => matches!(value.as_ref(), ConstHeapValue32Data::LongString(_)),
        _ => false,
    }) {
        return Some(NativeListElementKind::StrPtr);
    }
    None
}
