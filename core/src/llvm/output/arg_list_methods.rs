use crate::{
    llvm::straightline_value::{NativeStraightlineValue, native_static_value_eq},
    vm::ConstRuntimeValue32Data,
};

pub(super) fn emit_native_arg_list_method(
    receiver: &[NativeStraightlineValue],
    method: &str,
    args: &NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    match method {
        "first" if native_arg_list_method_arg_count(args)? == 0 => {
            receiver.first().cloned().or(Some(NativeStraightlineValue::Nil))
        }
        "last" if native_arg_list_method_arg_count(args)? == 0 => {
            receiver.last().cloned().or(Some(NativeStraightlineValue::Nil))
        }
        "get" => native_arg_list_i64_arg(args)
            .and_then(|index| receiver.get(index).cloned())
            .or(Some(NativeStraightlineValue::Nil)),
        "contains" => {
            let needle = native_arg_list_single_arg(args)?;
            Some(NativeStraightlineValue::Bool(
                i64::from(receiver.iter().any(|value| native_static_value_eq(value, needle))).to_string(),
            ))
        }
        "index_of" => {
            let needle = native_arg_list_single_arg(args)?;
            let index = receiver
                .iter()
                .position(|value| native_static_value_eq(value, needle))
                .map_or(-1, |index| index as i64);
            Some(NativeStraightlineValue::I64(index.to_string()))
        }
        "is_empty" if native_arg_list_method_arg_count(args)? == 0 => Some(NativeStraightlineValue::Bool(
            i64::from(receiver.is_empty()).to_string(),
        )),
        "pop" if native_arg_list_method_arg_count(args)? == 0 => {
            receiver.last().cloned().or(Some(NativeStraightlineValue::Nil))
        }
        "reverse" if native_arg_list_method_arg_count(args)? == 0 => {
            let mut elements = receiver.to_vec();
            elements.reverse();
            Some(NativeStraightlineValue::ArgList { elements })
        }
        "push" => {
            let value = native_arg_list_single_arg(args)?;
            let mut elements = Vec::with_capacity(receiver.len() + 1);
            elements.extend(receiver.iter().cloned());
            elements.push(value.clone());
            Some(NativeStraightlineValue::ArgList { elements })
        }
        "slice" => {
            let (start, end) = match args {
                NativeStraightlineValue::ArgList { elements: args } => {
                    let start = native_arg_list_usize_value(args.first()?)?;
                    let end = native_arg_list_optional_usize(args.get(1))?;
                    (start, end)
                }
                NativeStraightlineValue::List { elements: args, .. } => {
                    let start = native_const_list_usize_value(args.first()?)?;
                    let end = native_const_list_optional_usize(args.get(1))?;
                    (start, end)
                }
                _ => return None,
            };
            let end = end.unwrap_or(receiver.len());
            let end = end.min(receiver.len());
            let elements = if start >= end {
                Vec::new()
            } else {
                receiver.get(start..end)?.to_vec()
            };
            Some(NativeStraightlineValue::ArgList { elements })
        }
        "insert" => {
            let (index, value) = native_arg_list_index_value_args(args)?;
            if index > receiver.len() {
                return None;
            }
            let mut elements = receiver.to_vec();
            elements.insert(index, value.clone());
            Some(NativeStraightlineValue::ArgList { elements })
        }
        "remove_at" => {
            let index = native_arg_list_i64_arg(args)?;
            let mut elements = receiver.to_vec();
            if index >= elements.len() {
                return None;
            }
            let old = elements.remove(index);
            Some(NativeStraightlineValue::ArgList {
                elements: vec![NativeStraightlineValue::ArgList { elements }, old],
            })
        }
        "set" => {
            let (index, value) = native_arg_list_index_value_args(args)?;
            let mut elements = receiver.to_vec();
            let old = std::mem::replace(elements.get_mut(index)?, value.clone());
            Some(NativeStraightlineValue::ArgList {
                elements: vec![NativeStraightlineValue::ArgList { elements }, old],
            })
        }
        "unique" if native_arg_list_method_arg_count(args)? == 0 => {
            let mut elements = Vec::new();
            for value in receiver {
                if !elements.iter().any(|seen| native_static_value_eq(seen, value)) {
                    elements.push(value.clone());
                }
            }
            Some(NativeStraightlineValue::ArgList { elements })
        }
        "take" => {
            let count = native_arg_list_i64_arg(args)?;
            Some(NativeStraightlineValue::ArgList {
                elements: receiver.iter().take(count).cloned().collect(),
            })
        }
        "skip" => {
            let count = native_arg_list_i64_arg(args)?;
            Some(NativeStraightlineValue::ArgList {
                elements: receiver.iter().skip(count).cloned().collect(),
            })
        }
        "concat" | "chain" => {
            let rhs = native_arg_list_single_arg(args)?;
            let NativeStraightlineValue::ArgList { elements: rhs } = rhs else {
                return None;
            };
            let mut elements = Vec::with_capacity(receiver.len() + rhs.len());
            elements.extend(receiver.iter().cloned());
            elements.extend(rhs.iter().cloned());
            Some(NativeStraightlineValue::ArgList { elements })
        }
        _ => None,
    }
}

fn native_arg_list_usize_value(value: &NativeStraightlineValue) -> Option<usize> {
    match value {
        NativeStraightlineValue::I64(index) => index.parse().ok(),
        _ => None,
    }
}

fn native_arg_list_optional_usize(value: Option<&NativeStraightlineValue>) -> Option<Option<usize>> {
    match value {
        Some(value) => native_arg_list_usize_value(value).map(Some),
        None => Some(None),
    }
}

fn native_const_list_usize_value(value: &ConstRuntimeValue32Data) -> Option<usize> {
    match value {
        ConstRuntimeValue32Data::Int(index) => usize::try_from(*index).ok(),
        _ => None,
    }
}

fn native_const_list_optional_usize(value: Option<&ConstRuntimeValue32Data>) -> Option<Option<usize>> {
    match value {
        Some(value) => native_const_list_usize_value(value).map(Some),
        None => Some(None),
    }
}

fn native_arg_list_method_arg_count(value: &NativeStraightlineValue) -> Option<usize> {
    match value {
        NativeStraightlineValue::ArgList { elements } => Some(elements.len()),
        NativeStraightlineValue::List { elements, .. } => Some(elements.len()),
        NativeStraightlineValue::DynamicList { .. } => Some(0),
        _ => None,
    }
}

fn native_arg_list_i64_arg(value: &NativeStraightlineValue) -> Option<usize> {
    match value {
        NativeStraightlineValue::ArgList { elements } => match elements.as_slice() {
            [NativeStraightlineValue::I64(index)] => index.parse().ok(),
            _ => None,
        },
        NativeStraightlineValue::List { elements, .. } => match elements.as_slice() {
            [ConstRuntimeValue32Data::Int(index)] => usize::try_from(*index).ok(),
            _ => None,
        },
        _ => None,
    }
}

fn native_arg_list_index_value_args(value: &NativeStraightlineValue) -> Option<(usize, &NativeStraightlineValue)> {
    match value {
        NativeStraightlineValue::ArgList { elements } => match elements.as_slice() {
            [NativeStraightlineValue::I64(index), value] => Some((index.parse().ok()?, value)),
            _ => None,
        },
        _ => None,
    }
}

fn native_arg_list_single_arg(value: &NativeStraightlineValue) -> Option<&NativeStraightlineValue> {
    match value {
        NativeStraightlineValue::ArgList { elements } => match elements.as_slice() {
            [value] => Some(value),
            _ => None,
        },
        _ => None,
    }
}
