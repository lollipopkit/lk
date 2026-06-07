use crate::{
    llvm::{
        const_display::native_const_list_display,
        ir_text::llvm_float_literal,
        straightline_value::{NativeStraightlineValue, native_runtime_const_value},
    },
    vm::{ConstHeapValueData, ConstRuntimeValueData},
};

use super::native_static_string_value;

pub(super) fn emit_native_string_module_method(
    method: &str,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match method {
        "lower" => unary_string(args, |value| native_static_string_value(&value.to_lowercase())),
        "upper" => unary_string(args, |value| native_static_string_value(&value.to_uppercase())),
        "trim" => unary_string(args, |value| native_static_string_value(value.trim())),
        "reverse" => unary_string(args, |value| {
            native_static_string_value(&value.chars().rev().collect::<String>())
        }),
        "is_empty" => unary_string(args, |value| bool_value(value.is_empty())),
        "starts_with" => binary_string_bool(args, |value, needle| value.starts_with(needle)),
        "ends_with" => binary_string_bool(args, |value, needle| value.ends_with(needle)),
        "contains" => binary_string_bool(args, |value, needle| value.contains(needle)),
        "replace" => emit_native_string_replace(args),
        "substring" => emit_native_string_substring(args),
        "repeat" => emit_native_string_repeat(args),
        "char" => emit_native_string_char(args),
        "byte" => emit_native_string_byte(args),
        "chars" => emit_native_string_chars(args, ssa_index),
        "find" => emit_native_string_find(args),
        "split" => emit_native_string_split(args, ssa_index),
        "join" => emit_native_string_join(args),
        "format" => emit_native_string_format(args),
        "strip" => emit_native_string_strip(args),
        "strip_prefix" => emit_native_string_strip_prefix(args),
        "strip_suffix" => emit_native_string_strip_suffix(args),
        "count" => emit_native_string_count(args),
        "pad_left" => emit_native_string_pad(args, true),
        "pad_right" => emit_native_string_pad(args, false),
        "to_int" => emit_native_string_to_int(args),
        "to_float" => emit_native_string_to_float(args),
        "title" => unary_string(args, |value| native_static_string_value(&title_case(value))),
        "capitalize" => unary_string(args, |value| native_static_string_value(&capitalize(value))),
        _ => None,
    }
}

fn unary_string(
    args: &[NativeStraightlineValue],
    f: impl FnOnce(&str) -> NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let [value] = args else {
        return None;
    };
    Some(f(&string_arg(value)?))
}

fn binary_string_bool(
    args: &[NativeStraightlineValue],
    f: impl FnOnce(&str, &str) -> bool,
) -> Option<NativeStraightlineValue> {
    let [value, needle] = args else {
        return None;
    };
    Some(bool_value(f(&string_arg(value)?, &string_arg(needle)?)))
}

fn emit_native_string_replace(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [source, pattern, with] = args else {
        return None;
    };
    Some(native_static_string_value(
        &string_arg(source)?.replace(&string_arg(pattern)?, &string_arg(with)?),
    ))
}

fn emit_native_string_substring(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, start, len] = args else {
        return None;
    };
    let value = string_arg(value)?;
    let start = usize_arg(start)?;
    let len = usize_arg(len)?;
    let end = start.saturating_add(len).min(value.len());
    if start > value.len() {
        return None;
    }
    Some(native_static_string_value(value.get(start..end)?))
}

fn emit_native_string_repeat(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, count] = args else {
        return None;
    };
    Some(native_static_string_value(
        &string_arg(value)?.repeat(usize_arg(count)?),
    ))
}

fn emit_native_string_char(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, index] = args else {
        return None;
    };
    Some(
        string_arg(value)?
            .chars()
            .nth(usize_arg(index)?)
            .map(|value| native_static_string_value(&value.to_string()))
            .unwrap_or(NativeStraightlineValue::Nil),
    )
}

fn emit_native_string_byte(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, index] = args else {
        return None;
    };
    Some(
        string_arg(value)?
            .as_bytes()
            .get(usize_arg(index)?)
            .map(|value| NativeStraightlineValue::I64((*value as i64).to_string()))
            .unwrap_or(NativeStraightlineValue::Nil),
    )
}

fn emit_native_string_chars(
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [value] = args else {
        return None;
    };
    let elements = string_arg(value)?
        .chars()
        .map(|ch| ConstRuntimeValueData::ShortStr(ch.to_string()))
        .collect::<Vec<_>>();
    let symbol = format!("@lk_string_chars_{}", *ssa_index);
    *ssa_index += 1;
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

fn emit_native_string_find(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, pattern] = args else {
        if let [value, pattern, start] = args {
            return find_from(value, pattern, usize_arg(start)?);
        }
        return None;
    };
    find_from(value, pattern, 0)
}

fn find_from(
    value: &NativeStraightlineValue,
    pattern: &NativeStraightlineValue,
    start: usize,
) -> Option<NativeStraightlineValue> {
    let value = string_arg(value)?;
    let pattern = string_arg(pattern)?;
    if start > value.len() {
        return Some(NativeStraightlineValue::Nil);
    }
    Some(
        value[start..]
            .find(&pattern)
            .map(|index| NativeStraightlineValue::I64((start + index).to_string()))
            .unwrap_or(NativeStraightlineValue::Nil),
    )
}

fn emit_native_string_split(
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [value, delimiter] = args else {
        return None;
    };
    let value = string_arg(value)?;
    let delimiter = string_arg(delimiter)?;
    let parts = if delimiter.is_empty() {
        value.chars().map(|ch| ch.to_string()).collect::<Vec<_>>()
    } else {
        value.split(&delimiter).map(ToString::to_string).collect::<Vec<_>>()
    };
    let elements = parts
        .into_iter()
        .map(ConstRuntimeValueData::ShortStr)
        .collect::<Vec<_>>();
    let symbol = format!("@lk_string_split_{}", *ssa_index);
    *ssa_index += 1;
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

fn emit_native_string_join(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [values, delimiter] = args else {
        return None;
    };
    let NativeStraightlineValue::List { elements, .. } = values else {
        return None;
    };
    let delimiter = string_arg(delimiter)?;
    let parts = elements.iter().map(const_string_arg).collect::<Option<Vec<_>>>()?;
    Some(native_static_string_value(&parts.join(&delimiter)))
}

fn emit_native_string_format(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [format, rest @ ..] = args else {
        return None;
    };
    let format = string_arg(format)?;
    let mut out = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();
    let mut arg_index = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if let Some(value) = rest.get(arg_index) {
                out.push_str(&native_display_arg(value)?);
                arg_index += 1;
            } else {
                out.push_str("{}");
            }
        } else {
            out.push(ch);
        }
    }
    if arg_index < rest.len() {
        if !out.is_empty() {
            out.push(' ');
        }
        for (index, value) in rest[arg_index..].iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            out.push_str(&native_display_arg(value)?);
        }
    }
    Some(native_static_string_value(&out))
}

fn emit_native_string_strip(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, pattern] = args else {
        return None;
    };
    let value = string_arg(value)?;
    let pattern = string_arg(pattern)?;
    Some(
        value
            .strip_prefix(&pattern)
            .or_else(|| value.strip_suffix(&pattern))
            .map(native_static_string_value)
            .unwrap_or(NativeStraightlineValue::Nil),
    )
}

fn emit_native_string_strip_prefix(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, prefix] = args else {
        return None;
    };
    Some(
        string_arg(value)?
            .strip_prefix(&string_arg(prefix)?)
            .map(native_static_string_value)
            .unwrap_or(NativeStraightlineValue::Nil),
    )
}

fn emit_native_string_strip_suffix(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, suffix] = args else {
        return None;
    };
    Some(
        string_arg(value)?
            .strip_suffix(&string_arg(suffix)?)
            .map(native_static_string_value)
            .unwrap_or(NativeStraightlineValue::Nil),
    )
}

fn emit_native_string_count(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value, pattern] = args else {
        return None;
    };
    let value = string_arg(value)?;
    let pattern = string_arg(pattern)?;
    let count = if pattern.is_empty() {
        value.len() + 1
    } else {
        value.matches(&pattern).count()
    };
    Some(NativeStraightlineValue::I64(count.to_string()))
}

fn emit_native_string_pad(args: &[NativeStraightlineValue], left: bool) -> Option<NativeStraightlineValue> {
    let (value, width, fill) = match args {
        [value, width] => (string_arg(value)?, usize_arg(width)?, " ".to_string()),
        [value, width, fill] => (string_arg(value)?, usize_arg(width)?, string_arg(fill)?),
        _ => return None,
    };
    if fill.is_empty() {
        return None;
    }
    if width <= value.len() {
        return Some(native_static_string_value(&value));
    }
    let needed = width - value.len();
    let pad = fill.repeat(needed / fill.len() + 1);
    let value = if left {
        format!("{}{}", &pad[pad.len() - needed..], value)
    } else {
        format!("{}{}", value, &pad[..needed])
    };
    Some(native_static_string_value(&value))
}

fn emit_native_string_to_int(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value] = args else {
        return None;
    };
    let value = match value {
        NativeStraightlineValue::I64(value) => value.parse::<i64>().ok()?,
        NativeStraightlineValue::F64(value) => value.parse::<f64>().ok()? as i64,
        NativeStraightlineValue::Bool(value) => i64::from(value != "0"),
        _ => return None,
    };
    Some(NativeStraightlineValue::I64(value.to_string()))
}

fn emit_native_string_to_float(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value] = args else {
        return None;
    };
    let value = match value {
        NativeStraightlineValue::F64(value) => value.parse::<f64>().ok()?,
        NativeStraightlineValue::I64(value) => value.parse::<i64>().ok()? as f64,
        NativeStraightlineValue::Bool(value) => {
            if value == "0" {
                0.0
            } else {
                1.0
            }
        }
        _ => return None,
    };
    Some(NativeStraightlineValue::F64(llvm_float_literal(value)))
}

fn string_arg(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::String { value, .. } => Some(value.clone()),
        _ => None,
    }
}

fn const_string_arg(value: &ConstRuntimeValueData) -> Option<String> {
    match value {
        ConstRuntimeValueData::ShortStr(value) => Some(value.clone()),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn usize_arg(value: &NativeStraightlineValue) -> Option<usize> {
    let NativeStraightlineValue::I64(value) = value else {
        return None;
    };
    value.parse().ok()
}

fn bool_value(value: bool) -> NativeStraightlineValue {
    NativeStraightlineValue::Bool(i64::from(value).to_string())
}

fn title_case(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut capitalize_next = true;
    for ch in value.chars() {
        if ch.is_whitespace() {
            capitalize_next = true;
            result.push(ch);
        } else if capitalize_next {
            for ch in ch.to_uppercase() {
                result.push(ch);
            }
            capitalize_next = false;
        } else {
            for ch in ch.to_lowercase() {
                result.push(ch);
            }
        }
    }
    result
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    let mut result = String::with_capacity(value.len());
    if let Some(first) = chars.next() {
        for ch in first.to_uppercase() {
            result.push(ch);
        }
    }
    for ch in chars {
        for ch in ch.to_lowercase() {
            result.push(ch);
        }
    }
    result
}

fn native_display_arg(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::Nil => Some("nil".to_string()),
        NativeStraightlineValue::Bool(value) => Some(if value == "0" { "false" } else { "true" }.to_string()),
        NativeStraightlineValue::I64(value) | NativeStraightlineValue::F64(value) => Some(value.clone()),
        NativeStraightlineValue::String { value, .. } => Some(value.clone()),
        NativeStraightlineValue::List { elements, .. } => native_const_list_display(elements),
        value => {
            let value = native_runtime_const_value(value)?;
            let display = native_const_list_display(std::slice::from_ref(&value))?;
            Some(display.trim_start_matches('[').trim_end_matches(']').to_string())
        }
    }
}
