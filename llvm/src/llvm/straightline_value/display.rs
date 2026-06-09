use crate::{
    llvm::{
        const_display::{native_const_list_display, native_const_map_display, native_const_object_display},
        stdlib_catalog::{stdlib_builtin_display, stdlib_module_display},
    },
    vm::RuntimeMapKeyData,
};

use super::{NativeBuiltin, NativeModule, NativeStraightlineValue, NativeTextPart};

pub(super) fn native_builtin_display(builtin: NativeBuiltin) -> String {
    if let Some(display) = stdlib_builtin_display(builtin) {
        return display;
    }
    let (name, arity) = match builtin {
        NativeBuiltin::BitAnd => ("__lk_bit_and", Some(2)),
        NativeBuiltin::BitNot => ("__lk_bit_not", Some(1)),
        NativeBuiltin::BitOr => ("__lk_bit_or", Some(2)),
        NativeBuiltin::CoreCallMethod => ("__lk_call_method", None),
        NativeBuiltin::CoreMakeStruct => ("__lk_make_struct", None),
        NativeBuiltin::CoreMergeFields => ("__lk_merge_fields", None),
        NativeBuiltin::CoreRegisterTrait => ("__lk_register_trait", None),
        NativeBuiltin::CoreRegisterTraitImpl => ("__lk_register_trait_impl", None),
        NativeBuiltin::CoreSet => ("Set", Some(1)),
        NativeBuiltin::CoreTypeof => ("typeof", Some(1)),
        NativeBuiltin::FibIterative => ("iterative", Some(1)),
        NativeBuiltin::GreetingsMessage => ("message", Some(1)),
        NativeBuiltin::IterModuleMethod(method) => (method, Some(1)),
        NativeBuiltin::StringModuleMethod(method) => (method, None),
        NativeBuiltin::ListConcat => ("concat", Some(2)),
        NativeBuiltin::ListContains => ("contains", Some(2)),
        NativeBuiltin::ListFirst => ("first", Some(1)),
        NativeBuiltin::ListGet => ("get", Some(2)),
        NativeBuiltin::ListIndexOf => ("index_of", Some(2)),
        NativeBuiltin::ListInsert => ("insert", Some(3)),
        NativeBuiltin::ListIsEmpty => ("is_empty", Some(1)),
        NativeBuiltin::ListJoin => ("join", Some(2)),
        NativeBuiltin::ListLast => ("last", Some(1)),
        NativeBuiltin::ListLen => ("len", Some(1)),
        NativeBuiltin::ListPop => ("pop", Some(1)),
        NativeBuiltin::ListPush => ("push", Some(2)),
        NativeBuiltin::ListRemoveAt => ("remove_at", Some(2)),
        NativeBuiltin::ListReverse => ("reverse", Some(1)),
        NativeBuiltin::ListSet => ("set", Some(3)),
        NativeBuiltin::ListSlice => ("slice", None),
        NativeBuiltin::ListSort => ("sort", Some(1)),
        NativeBuiltin::MathlibDouble => ("double", Some(1)),
        NativeBuiltin::MathModuleMethod(method) => (method, None),
        NativeBuiltin::MapModuleMethod(method) => (method, None),
        NativeBuiltin::MapDelete => ("delete", Some(2)),
        NativeBuiltin::MapSet => ("set", Some(3)),
        NativeBuiltin::MapMutate => ("mutate", None),
        _ => ("<native>", None),
    };
    match arity {
        Some(arity) => format!("<native fn {name}({arity} args)>"),
        None => format!("<native fn {name}(...)>"),
    }
}

pub(super) fn native_module_display(module: NativeModule) -> Option<String> {
    stdlib_module_display(module.name()).or_else(|| native_core_container_module_display(module.name()))
}

fn native_core_container_module_display(name: &str) -> Option<String> {
    let entries: &[(&str, &str)] = match name {
        "map" => &[
            ("delete", "<native fn delete(2 args)>"),
            ("get", "<native fn get(...)>"),
            ("has", "<native fn has(...)>"),
            ("keys", "<native fn keys(...)>"),
            ("len", "<native fn len(...)>"),
            ("mutate", "<native fn mutate(...)>"),
            ("set", "<native fn set(3 args)>"),
            ("values", "<native fn values(...)>"),
        ],
        "list" => &[
            ("concat", "<native fn concat(2 args)>"),
            ("contains", "<native fn contains(2 args)>"),
            ("first", "<native fn first(1 args)>"),
            ("get", "<native fn get(2 args)>"),
            ("index_of", "<native fn index_of(2 args)>"),
            ("insert", "<native fn insert(3 args)>"),
            ("is_empty", "<native fn is_empty(1 args)>"),
            ("join", "<native fn join(2 args)>"),
            ("last", "<native fn last(1 args)>"),
            ("len", "<native fn len(1 args)>"),
            ("pop", "<native fn pop(1 args)>"),
            ("push", "<native fn push(2 args)>"),
            ("remove_at", "<native fn remove_at(2 args)>"),
            ("reverse", "<native fn reverse(1 args)>"),
            ("set", "<native fn set(3 args)>"),
            ("slice", "<native fn slice(...)>"),
            ("sort", "<native fn sort(1 args)>"),
        ],
        _ => return None,
    };
    Some(format!(
        "{{{}}}",
        entries
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

pub(super) fn native_arg_list_display(values: &[NativeStraightlineValue]) -> Option<String> {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&native_value_display(value)?);
    }
    out.push(']');
    Some(out)
}

pub(super) fn native_display_map_display(entries: &[(RuntimeMapKeyData, NativeStraightlineValue)]) -> Option<String> {
    let mut out = String::from("{");
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&native_map_key_display(key)?);
        out.push_str(": ");
        out.push_str(&native_value_display(value)?);
    }
    out.push('}');
    Some(out)
}

fn native_value_display(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::Nil => Some("nil".to_string()),
        NativeStraightlineValue::Bool(value) if value == "0" => Some("false".to_string()),
        NativeStraightlineValue::Bool(value) if value == "1" => Some("true".to_string()),
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => Some(value.clone()),
        NativeStraightlineValue::F64(value) if !value.starts_with('%') => native_f64_display(value),
        NativeStraightlineValue::String { value, .. } => Some(value.clone()),
        NativeStraightlineValue::Text(parts) => native_text_display(parts),
        NativeStraightlineValue::List { elements, .. } => native_const_list_display(elements),
        NativeStraightlineValue::Map { entries, .. } => native_const_map_display(entries),
        NativeStraightlineValue::Set { elements, .. } => native_const_list_display(elements),
        NativeStraightlineValue::DisplayMap { entries, .. } => native_display_map_display(entries),
        NativeStraightlineValue::Object { type_name, fields, .. } => native_const_object_display(type_name, fields),
        NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Builtin(_) => super::native_static_callable_display(value),
        NativeStraightlineValue::Module(_) => super::native_static_module_display(value),
        NativeStraightlineValue::ArgList { elements } => native_arg_list_display(elements),
        NativeStraightlineValue::DynamicArgListElement { .. } => None,
        _ => None,
    }
}

fn native_f64_display(value: &str) -> Option<String> {
    if value.starts_with("0x") {
        return Some(value.to_string());
    }
    Some(value.parse::<f64>().ok()?.to_string())
}

fn native_map_key_display(key: &RuntimeMapKeyData) -> Option<String> {
    match key {
        RuntimeMapKeyData::Nil => Some("nil".to_string()),
        RuntimeMapKeyData::Bool(value) => Some(value.to_string()),
        RuntimeMapKeyData::Int(value) => Some(value.to_string()),
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => Some(value.clone()),
        RuntimeMapKeyData::Obj(value) => Some(format!("<obj:{}>", value)),
    }
}

fn native_text_display(parts: &[NativeTextPart]) -> Option<String> {
    let mut out = String::new();
    for part in parts {
        match part {
            NativeTextPart::I64(value) | NativeTextPart::F64(value) => {
                if value.starts_with('%') {
                    return None;
                }
                out.push_str(value);
            }
            NativeTextPart::Bool(value) => {
                if value.starts_with('%') {
                    return None;
                }
                let value = match value.as_str() {
                    "1" => "true",
                    "0" => "false",
                    value => value,
                };
                out.push_str(value);
            }
            NativeTextPart::Nil => out.push_str("nil"),
            NativeTextPart::String { value, .. } => out.push_str(value),
            NativeTextPart::StrPtr(_) => return None,
        }
    }
    Some(out)
}
