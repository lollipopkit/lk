use crate::{
    llvm::const_display::native_const_list_display,
    vm::ConstRuntimeValueData,
};

use super::{
    NativeStraightlineValue, native_const_runtime_value, native_runtime_const_value, native_static_value_eq,
};

pub(in crate::llvm) fn native_static_set_from_arg(
    arg: &NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let mut elements = match arg {
        NativeStraightlineValue::List { elements, .. } => elements.clone(),
        NativeStraightlineValue::ArgList { elements } => elements
            .iter()
            .map(native_runtime_const_value)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    dedup_set_elements(&mut elements)?;
    Some(NativeStraightlineValue::Set {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

pub(in crate::llvm) fn native_static_set_method(
    receiver: &NativeStraightlineValue,
    method: &str,
    args: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Set { elements, .. } = receiver else {
        return None;
    };
    match (method, args) {
        ("has" | "contains", [needle]) => {
            let contains = set_contains(elements, needle)?;
            Some(NativeStraightlineValue::Bool(i64::from(contains).to_string()))
        }
        ("len", []) => Some(NativeStraightlineValue::I64(elements.len().to_string())),
        ("add", [value]) => {
            let mut elements = elements.clone();
            let value = native_runtime_const_value(value)?;
            if !elements.iter().any(|element| const_runtime_value_eq(element, &value)) {
                elements.push(value);
            }
            Some(NativeStraightlineValue::Set {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        ("delete", [value]) => {
            let mut elements = elements.clone();
            elements.retain(|element| !const_runtime_value_matches(element, value).unwrap_or(false));
            Some(NativeStraightlineValue::Set {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        _ => None,
    }
}

pub(in crate::llvm) fn native_static_set_contains(
    elements: &[ConstRuntimeValueData],
    needle: &NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    Some(NativeStraightlineValue::Bool(
        i64::from(set_contains(elements, needle)?).to_string(),
    ))
}

fn dedup_set_elements(elements: &mut Vec<ConstRuntimeValueData>) -> Option<()> {
    let mut out = Vec::with_capacity(elements.len());
    for element in elements.drain(..) {
        if !out
            .iter()
            .any(|existing| const_runtime_value_eq(existing, &element))
        {
            out.push(element);
        }
    }
    *elements = out;
    Some(())
}

fn set_contains(elements: &[ConstRuntimeValueData], needle: &NativeStraightlineValue) -> Option<bool> {
    elements
        .iter()
        .map(|element| const_runtime_value_matches(element, needle))
        .try_fold(false, |found, matched| Some(found || matched?))
}

fn const_runtime_value_matches(
    element: &ConstRuntimeValueData,
    needle: &NativeStraightlineValue,
) -> Option<bool> {
    let element = native_const_runtime_value(element, String::new())?;
    Some(native_static_value_eq(&element, needle))
}

fn const_runtime_value_eq(lhs: &ConstRuntimeValueData, rhs: &ConstRuntimeValueData) -> bool {
    native_const_runtime_value(lhs, String::new())
        .zip(native_const_runtime_value(rhs, String::new()))
        .is_some_and(|(lhs, rhs)| native_static_value_eq(&lhs, &rhs))
}
