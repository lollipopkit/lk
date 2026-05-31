use crate::vm::ConstRuntimeValue32Data;

pub(super) fn native_object_display_text(
    type_name: &str,
    fields: &[(String, ConstRuntimeValue32Data)],
) -> Option<String> {
    match type_name {
        "Rect" => {
            let w = native_object_i64_field(fields, "w")?;
            let h = native_object_i64_field(fields, "h")?;
            Some(format!("Rect({w}x{h})"))
        }
        "Circle" => {
            let r = native_object_i64_field(fields, "r")?;
            Some(format!("Circle(r={r})"))
        }
        _ => None,
    }
}

fn native_object_i64_field(fields: &[(String, ConstRuntimeValue32Data)], key: &str) -> Option<i64> {
    let (_, value) = fields.iter().find(|(field, _)| field == key)?;
    match value {
        ConstRuntimeValue32Data::Int(value) => Some(*value),
        _ => None,
    }
}
