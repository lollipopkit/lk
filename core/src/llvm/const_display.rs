use crate::vm::{ConstHeapValueData, ConstRuntimeValueData, RuntimeMapKeyData};

use super::ir_text::llvm_escape_bytes;

pub(super) fn native_string_const_value(value: &str) -> Option<String> {
    if value.as_bytes().contains(&0) {
        None
    } else {
        Some(value.to_string())
    }
}

pub(super) fn native_const_list_display(values: &[ConstRuntimeValueData]) -> Option<String> {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&native_const_runtime_display(value)?);
    }
    out.push(']');
    Some(out)
}

pub(super) fn native_const_map_display(values: &[(RuntimeMapKeyData, ConstRuntimeValueData)]) -> Option<String> {
    let mut out = String::from("{");
    for (index, (key, value)) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&native_const_map_key_display(key)?);
        out.push_str(": ");
        out.push_str(&native_const_runtime_display(value)?);
    }
    out.push('}');
    Some(out)
}

pub(super) fn native_const_object_display(
    type_name: &str,
    fields: &[(String, ConstRuntimeValueData)],
) -> Option<String> {
    let mut fields = fields.iter().collect::<Vec<_>>();
    fields.sort_by(|(lhs, _), (rhs, _)| lhs.cmp(rhs));
    let mut out = String::with_capacity(type_name.len() + 4);
    out.push('<');
    out.push_str(type_name);
    out.push_str(" {");
    for (index, (key, value)) in fields.into_iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&native_const_runtime_display(value)?);
    }
    out.push_str("}>");
    Some(out)
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

fn native_const_runtime_display(value: &ConstRuntimeValueData) -> Option<String> {
    match value {
        ConstRuntimeValueData::Nil => Some("nil".to_string()),
        ConstRuntimeValueData::Bool(value) => Some(value.to_string()),
        ConstRuntimeValueData::Int(value) => Some(value.to_string()),
        ConstRuntimeValueData::Float(value) => Some(value.to_string()),
        ConstRuntimeValueData::ShortStr(value) => native_string_const_value(value),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => native_string_const_value(value),
            ConstHeapValueData::List(values) => native_const_list_display(values),
            ConstHeapValueData::Map(values) => native_const_map_display(values),
            ConstHeapValueData::UpvalCell(_) => None,
        },
    }
}

pub(super) fn llvm_string_constant(symbol: &str, value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len() + 1);
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    format!(
        "{symbol} = private unnamed_addr constant [{} x i8] c\"{}\", align 1\n",
        bytes.len(),
        llvm_escape_bytes(&bytes)
    )
}
