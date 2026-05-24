use crate::vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, RuntimeMapKeyData};

use super::ir_text::llvm_escape_bytes;

pub(super) fn native_string_const_value(value: &str) -> Option<String> {
    if value.as_bytes().contains(&0) {
        None
    } else {
        Some(value.to_string())
    }
}

pub(super) fn native_const_list_display(values: &[ConstRuntimeValue32Data]) -> Option<String> {
    let mut parts = Vec::with_capacity(values.len());
    for value in values {
        parts.push(native_const_runtime_display(value)?);
    }
    Some(format!("[{}]", parts.join(", ")))
}

pub(super) fn native_const_map_display(values: &[(RuntimeMapKeyData, ConstRuntimeValue32Data)]) -> Option<String> {
    let mut parts = Vec::with_capacity(values.len());
    for (key, value) in values {
        parts.push(format!(
            "{}: {}",
            native_const_map_key_display(key)?,
            native_const_runtime_display(value)?
        ));
    }
    Some(format!("{{{}}}", parts.join(", ")))
}

pub(super) fn native_const_object_display(
    type_name: &str,
    fields: &[(String, ConstRuntimeValue32Data)],
) -> Option<String> {
    let mut fields = fields.to_vec();
    fields.sort_by(|(lhs, _), (rhs, _)| lhs.cmp(rhs));
    let mut parts = Vec::with_capacity(fields.len());
    for (key, value) in fields {
        parts.push(format!("{key}: {}", native_const_runtime_display(&value)?));
    }
    Some(format!("<{type_name} {{{}}}>", parts.join(", ")))
}

fn native_const_map_key_display(key: &RuntimeMapKeyData) -> Option<String> {
    match key {
        RuntimeMapKeyData::Nil => Some("nil".to_string()),
        RuntimeMapKeyData::Bool(value) => Some(value.to_string()),
        RuntimeMapKeyData::Int(value) => Some(value.to_string()),
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => native_string_const_value(value),
        RuntimeMapKeyData::Obj(value) => Some(format!("<obj:{}>", value)),
    }
}

fn native_const_runtime_display(value: &ConstRuntimeValue32Data) -> Option<String> {
    match value {
        ConstRuntimeValue32Data::Nil => Some("nil".to_string()),
        ConstRuntimeValue32Data::Bool(value) => Some(value.to_string()),
        ConstRuntimeValue32Data::Int(value) => Some(value.to_string()),
        ConstRuntimeValue32Data::Float(value) => Some(value.to_string()),
        ConstRuntimeValue32Data::ShortStr(value) => native_string_const_value(value),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => native_string_const_value(value),
            ConstHeapValue32Data::List(values) => native_const_list_display(values),
            ConstHeapValue32Data::Map(values) => native_const_map_display(values),
            ConstHeapValue32Data::UpvalCell(_) => None,
        },
    }
}

pub(super) fn llvm_string_constant(symbol: &str, value: &str) -> String {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    format!(
        "{symbol} = private unnamed_addr constant [{} x i8] c\"{}\", align 1\n",
        bytes.len(),
        llvm_escape_bytes(&bytes)
    )
}
