use crate::llvm::straightline_value::NativeStraightlineValue;
use crate::vm::ConstRuntimeValueData;

use super::native_static_string_value;

pub(super) fn emit_native_object_method(
    receiver: &NativeStraightlineValue,
    method: &str,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Object { type_name, fields, .. } = receiver else {
        return None;
    };
    match (type_name.as_str(), method) {
        ("Rect", "area") => {
            let w = native_object_i64_field(fields, "w")?;
            let h = native_object_i64_field(fields, "h")?;
            Some(NativeStraightlineValue::I64((w * h).to_string()))
        }
        ("Circle", "area") => {
            let r = native_object_i64_field(fields, "r")?;
            Some(NativeStraightlineValue::I64((3 * r * r).to_string()))
        }
        ("Rect", "show" | "describe") => {
            let w = native_object_i64_field(fields, "w")?;
            let h = native_object_i64_field(fields, "h")?;
            Some(native_static_string_value(&format!("Rect({w}x{h})")))
        }
        ("Circle", "show" | "describe") => {
            let r = native_object_i64_field(fields, "r")?;
            Some(native_static_string_value(&format!("Circle(r={r})")))
        }
        _ => None,
    }
}

fn native_object_i64_field(fields: &[(String, ConstRuntimeValueData)], key: &str) -> Option<i64> {
    let (_, value) = fields.iter().find(|(field, _)| field == key)?;
    match value {
        ConstRuntimeValueData::Int(value) => Some(*value),
        _ => None,
    }
}
