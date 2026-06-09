use crate::{
    llvm::{
        const_display::native_const_list_display,
        straightline_value::{
            NativeBuiltin, NativeStraightlineValue, NativeStringKeyKind, native_const_runtime_value,
            native_runtime_const_value, native_static_contains, native_static_index, native_static_len,
            native_static_list_join, native_static_slice_from,
        },
    },
    vm::{ConstHeapValueData, ConstRuntimeValueData},
};

pub(super) fn emit_native_list_builtin(
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match builtin {
        NativeBuiltin::ListConcat => emit_native_list_concat(args, ssa_index),
        NativeBuiltin::ListContains => emit_native_list_contains(args),
        NativeBuiltin::ListFirst => emit_native_list_first(args),
        NativeBuiltin::ListGet => emit_native_list_get(args),
        NativeBuiltin::ListIndexOf => emit_native_list_index_of(args),
        NativeBuiltin::ListInsert => emit_native_list_insert(args, ssa_index),
        NativeBuiltin::ListIsEmpty => emit_native_list_is_empty(args),
        NativeBuiltin::ListJoin => emit_native_list_join(args, ssa_index),
        NativeBuiltin::ListLast => emit_native_list_last(args),
        NativeBuiltin::ListLen => emit_native_list_len(args),
        NativeBuiltin::ListPop => emit_native_list_pop(args),
        NativeBuiltin::ListPush => emit_native_list_push(args, ssa_index),
        NativeBuiltin::ListRemoveAt => emit_native_list_remove_at(args, ssa_index),
        NativeBuiltin::ListReverse => emit_native_list_reverse(args, ssa_index),
        NativeBuiltin::ListSet => emit_native_list_set(args, ssa_index),
        NativeBuiltin::ListSlice => emit_native_list_slice(args, ssa_index),
        NativeBuiltin::ListSort => emit_native_list_sort(args, ssa_index),
        _ => None,
    }
}

pub(in crate::llvm::output) fn emit_native_static_list_method(
    receiver: NativeStraightlineValue,
    method: &str,
    args: &[ConstRuntimeValueData],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if method == "len" && args.is_empty() {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        return Some(NativeStraightlineValue::I64(elements.len().to_string()));
    }
    if method == "is_empty" && args.is_empty() {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        return Some(NativeStraightlineValue::Bool(
            i64::from(elements.is_empty()).to_string(),
        ));
    }
    if method == "contains" && args.len() == 1 {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let needle = args.first()?;
        return Some(NativeStraightlineValue::Bool(
            i64::from(
                elements
                    .iter()
                    .any(|element| const_runtime_values_equal(element, needle)),
            )
            .to_string(),
        ));
    }
    if method == "reverse" && args.is_empty() {
        let NativeStraightlineValue::List { mut elements, .. } = receiver else {
            return None;
        };
        elements.reverse();
        return static_list_method_value(elements, ssa_index);
    }
    if method == "sort" && args.is_empty() {
        let NativeStraightlineValue::List { mut elements, .. } = receiver else {
            return None;
        };
        elements.sort_by(compare_const_runtime_values);
        return static_list_method_value(elements, ssa_index);
    }
    if (method == "first" || method == "last") && args.is_empty() {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let value = if method == "first" {
            elements.first()
        } else {
            elements.last()
        };
        return value
            .and_then(native_const_list_method_arg)
            .or(Some(NativeStraightlineValue::Nil));
    }
    if method == "get" && args.len() == 1 {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let ConstRuntimeValueData::Int(index) = args.first()? else {
            return None;
        };
        let index = usize::try_from(*index).ok()?;
        return elements
            .get(index)
            .and_then(native_const_list_method_arg)
            .or(Some(NativeStraightlineValue::Nil));
    }
    if method == "take" && args.len() == 1 {
        let NativeStraightlineValue::List { mut elements, .. } = receiver else {
            return None;
        };
        let ConstRuntimeValueData::Int(count) = args.first()? else {
            return None;
        };
        elements.truncate(usize::try_from((*count).max(0)).ok()?);
        return static_list_method_value(elements, ssa_index);
    }
    if (method == "concat" || method == "chain") && args.len() == 1 {
        let NativeStraightlineValue::List { mut elements, .. } = receiver else {
            return None;
        };
        let ConstRuntimeValueData::Heap(value) = args.first()? else {
            return None;
        };
        let ConstHeapValueData::List(rhs) = value.as_ref() else {
            return None;
        };
        elements.extend(rhs.iter().cloned());
        return static_list_method_value(elements, ssa_index);
    }
    if method == "enumerate" && args.is_empty() {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let elements = elements
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(vec![
                    ConstRuntimeValueData::Int(index as i64),
                    value,
                ])))
            })
            .collect::<Vec<_>>();
        return static_list_method_value(elements, ssa_index);
    }
    if method == "unique" && args.is_empty() {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let mut unique = Vec::new();
        for element in elements {
            if !unique.iter().any(|value| const_runtime_values_equal(value, &element)) {
                unique.push(element);
            }
        }
        return static_list_method_value(unique, ssa_index);
    }
    if method == "zip" && args.len() == 1 {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let ConstRuntimeValueData::Heap(value) = args.first()? else {
            return None;
        };
        let ConstHeapValueData::List(rhs) = value.as_ref() else {
            return None;
        };
        let elements = elements
            .into_iter()
            .zip(rhs.iter().cloned())
            .map(|(lhs, rhs)| ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(vec![lhs, rhs]))))
            .collect::<Vec<_>>();
        return static_list_method_value(elements, ssa_index);
    }
    if method == "flatten" && args.is_empty() {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let mut flat = Vec::new();
        for element in elements {
            match element {
                ConstRuntimeValueData::Heap(value) => match *value {
                    ConstHeapValueData::List(values) => flat.extend(values),
                    value => flat.push(ConstRuntimeValueData::Heap(Box::new(value))),
                },
                value => flat.push(value),
            }
        }
        return static_list_method_value(flat, ssa_index);
    }
    if method == "chunk" && args.len() == 1 {
        let NativeStraightlineValue::List { elements, .. } = receiver else {
            return None;
        };
        let ConstRuntimeValueData::Int(size) = args.first()? else {
            return None;
        };
        let size = usize::try_from(*size).ok().filter(|size| *size > 0)?;
        let elements = elements
            .chunks(size)
            .map(|chunk| ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(chunk.to_vec()))))
            .collect::<Vec<_>>();
        return static_list_method_value(elements, ssa_index);
    }
    if method != "skip" || args.len() != 1 {
        return None;
    }
    let count = match args.first()? {
        ConstRuntimeValueData::Int(value) if *value >= 0 => *value,
        _ => return None,
    };
    let symbol = next_static_list_symbol(ssa_index);
    native_static_slice_from(receiver, NativeStraightlineValue::I64(count.to_string()), symbol)
}

fn static_list_method_value(
    elements: Vec<ConstRuntimeValueData>,
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let symbol = next_static_list_symbol(ssa_index);
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

fn next_static_list_symbol(ssa_index: &mut usize) -> String {
    let symbol = format!("@lk_static_list_method_{}", *ssa_index);
    *ssa_index += 1;
    symbol
}

fn emit_native_list_concat(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, rhs] = args else {
        return None;
    };
    let rhs = native_runtime_const_value(rhs)?;
    emit_native_static_list_method(target.clone(), "concat", &[rhs], ssa_index)
}

fn emit_native_list_contains(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target, needle] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { elements } = target {
        let contains = elements
            .iter()
            .any(|element| arg_list_runtime_values_equal(element, needle));
        return Some(NativeStraightlineValue::Bool(i64::from(contains).to_string()));
    }
    native_static_contains(needle.clone(), target.clone())
}

fn emit_native_list_first(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    native_static_index(
        target.clone(),
        NativeStraightlineValue::I64("0".to_string()),
        String::new(),
    )
}

fn emit_native_list_get(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target, index] = args else {
        return None;
    };
    native_static_index(target.clone(), index.clone(), String::new())
}

fn emit_native_list_index_of(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target, needle] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { elements } = target {
        let index = elements
            .iter()
            .position(|element| arg_list_runtime_values_equal(element, needle))
            .map_or(-1, |index| index as i64);
        return Some(NativeStraightlineValue::I64(index.to_string()));
    }
    let NativeStraightlineValue::List { elements, .. } = target else {
        return None;
    };
    let needle = native_runtime_const_value(needle)?;
    let index = elements
        .iter()
        .position(|element| const_runtime_values_equal(element, &needle))
        .map_or(-1, |index| index as i64);
    Some(NativeStraightlineValue::I64(index.to_string()))
}

fn emit_native_list_insert(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, index, value] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { mut elements } = target.clone() {
        let index = native_static_usize(index)?;
        if index > elements.len() {
            return None;
        }
        elements.insert(index, value.clone());
        return Some(NativeStraightlineValue::ArgList { elements });
    }
    let NativeStraightlineValue::List { mut elements, .. } = target.clone() else {
        return None;
    };
    let index = native_static_usize(index)?;
    if index > elements.len() {
        return None;
    }
    elements.insert(index, native_runtime_const_value(value)?);
    static_list_method_value(elements, ssa_index)
}

fn emit_native_list_is_empty(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    let NativeStraightlineValue::I64(len) = native_static_len(target.clone())? else {
        return None;
    };
    Some(NativeStraightlineValue::Bool(i64::from(len == "0").to_string()))
}

fn emit_native_list_join(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, separator] = args else {
        return None;
    };
    *ssa_index += 1;
    native_static_list_join(target.clone(), separator.clone(), String::new())
}

fn emit_native_list_last(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    let NativeStraightlineValue::I64(len) = native_static_len(target.clone())? else {
        return None;
    };
    let len = len.parse::<usize>().ok()?;
    let Some(index) = len.checked_sub(1) else {
        return Some(NativeStraightlineValue::Nil);
    };
    native_static_index(
        target.clone(),
        NativeStraightlineValue::I64(index.to_string()),
        String::new(),
    )
}

fn emit_native_list_len(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    native_static_len(target.clone())
}

fn emit_native_list_pop(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { elements } = target {
        return elements.last().cloned().or(Some(NativeStraightlineValue::Nil));
    }
    let NativeStraightlineValue::List { elements, .. } = target else {
        return None;
    };
    elements
        .last()
        .and_then(native_const_list_method_arg)
        .or(Some(NativeStraightlineValue::Nil))
}

fn emit_native_list_push(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, value] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { mut elements } = target.clone() {
        elements.push(value.clone());
        return Some(NativeStraightlineValue::ArgList { elements });
    }
    let NativeStraightlineValue::List { mut elements, .. } = target.clone() else {
        return None;
    };
    elements.push(native_runtime_const_value(value)?);
    static_list_method_value(elements, ssa_index)
}

fn emit_native_list_remove_at(
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [target, index] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { mut elements } = target.clone() {
        let index = native_static_usize(index)?;
        if index >= elements.len() {
            return None;
        }
        let old = elements.remove(index);
        return Some(static_arg_list_mutation_pair(elements, old));
    }
    let NativeStraightlineValue::List { mut elements, .. } = target.clone() else {
        return None;
    };
    let index = native_static_usize(index)?;
    if index >= elements.len() {
        return None;
    }
    let old = elements.remove(index);
    static_list_mutation_pair(elements, old, ssa_index)
}

fn emit_native_list_reverse(
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { mut elements } = target.clone() {
        elements.reverse();
        return Some(NativeStraightlineValue::ArgList { elements });
    }
    let NativeStraightlineValue::List { mut elements, .. } = target.clone() else {
        return None;
    };
    elements.reverse();
    static_list_method_value(elements, ssa_index)
}

fn emit_native_list_set(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, index, value] = args else {
        return None;
    };
    if let NativeStraightlineValue::ArgList { mut elements } = target.clone() {
        let index = native_static_usize(index)?;
        let slot = elements.get_mut(index)?;
        let old = std::mem::replace(slot, value.clone());
        return Some(static_arg_list_mutation_pair(elements, old));
    }
    let NativeStraightlineValue::List { mut elements, .. } = target.clone() else {
        return None;
    };
    let index = native_static_usize(index)?;
    let slot = elements.get_mut(index)?;
    let old = std::mem::replace(slot, native_runtime_const_value(value)?);
    static_list_mutation_pair(elements, old, ssa_index)
}

fn emit_native_list_slice(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target, start] = args else {
        if let [target, start, end] = args {
            return emit_native_list_slice_range(target, start, Some(end), ssa_index);
        }
        return None;
    };
    emit_native_list_slice_range(target, start, None, ssa_index)
}

fn emit_native_list_slice_range(
    target: &NativeStraightlineValue,
    start: &NativeStraightlineValue,
    end: Option<&NativeStraightlineValue>,
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let NativeStraightlineValue::ArgList { elements } = target {
        let start = native_static_usize(start)?;
        let end = match end {
            Some(end) => native_static_usize(end)?,
            None => elements.len(),
        };
        let end = end.min(elements.len());
        let elements = if start >= end {
            Vec::new()
        } else {
            elements.get(start..end)?.to_vec()
        };
        return Some(NativeStraightlineValue::ArgList { elements });
    }
    let NativeStraightlineValue::List { elements, .. } = target else {
        return None;
    };
    let start = native_static_usize(start)?;
    let end = match end {
        Some(end) => native_static_usize(end)?,
        None => elements.len(),
    };
    let end = end.min(elements.len());
    let elements = if start >= end {
        Vec::new()
    } else {
        elements.get(start..end)?.to_vec()
    };
    static_list_method_value(elements, ssa_index)
}

fn emit_native_list_sort(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    let NativeStraightlineValue::List { mut elements, .. } = target.clone() else {
        return None;
    };
    elements.sort_by(compare_const_runtime_values);
    static_list_method_value(elements, ssa_index)
}

fn static_list_mutation_pair(
    updated: Vec<ConstRuntimeValueData>,
    old: ConstRuntimeValueData,
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let updated = ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(updated)));
    static_list_method_value(vec![updated, old], ssa_index)
}

fn static_arg_list_mutation_pair(
    updated: Vec<NativeStraightlineValue>,
    old: NativeStraightlineValue,
) -> NativeStraightlineValue {
    NativeStraightlineValue::ArgList {
        elements: vec![NativeStraightlineValue::ArgList { elements: updated }, old],
    }
}

fn arg_list_runtime_values_equal(lhs: &NativeStraightlineValue, rhs: &NativeStraightlineValue) -> bool {
    match (lhs, rhs) {
        (NativeStraightlineValue::Nil, NativeStraightlineValue::Nil) => true,
        (NativeStraightlineValue::Bool(lhs), NativeStraightlineValue::Bool(rhs)) => lhs == rhs,
        (NativeStraightlineValue::I64(lhs), NativeStraightlineValue::I64(rhs)) => lhs == rhs,
        (NativeStraightlineValue::F64(lhs), NativeStraightlineValue::F64(rhs)) => lhs == rhs,
        (
            NativeStraightlineValue::String {
                value: lhs,
                key_kind: NativeStringKeyKind::Short,
                ..
            },
            NativeStraightlineValue::String {
                value: rhs,
                key_kind: NativeStringKeyKind::Short,
                ..
            },
        ) => lhs == rhs,
        _ => false,
    }
}

fn native_static_usize(value: &NativeStraightlineValue) -> Option<usize> {
    let NativeStraightlineValue::I64(value) = value else {
        return None;
    };
    value.parse::<usize>().ok()
}

fn native_const_list_method_arg(value: &ConstRuntimeValueData) -> Option<NativeStraightlineValue> {
    native_const_runtime_value(value, String::new())
}

fn compare_const_runtime_values(lhs: &ConstRuntimeValueData, rhs: &ConstRuntimeValueData) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (lhs, rhs) {
        (ConstRuntimeValueData::Nil, ConstRuntimeValueData::Nil) => Ordering::Equal,
        (ConstRuntimeValueData::Nil, _) => Ordering::Less,
        (_, ConstRuntimeValueData::Nil) => Ordering::Greater,
        (ConstRuntimeValueData::Bool(lhs), ConstRuntimeValueData::Bool(rhs)) => lhs.cmp(rhs),
        (ConstRuntimeValueData::Int(lhs), ConstRuntimeValueData::Int(rhs)) => lhs.cmp(rhs),
        (ConstRuntimeValueData::Float(lhs), ConstRuntimeValueData::Float(rhs)) => {
            lhs.partial_cmp(rhs).unwrap_or(Ordering::Equal)
        }
        (ConstRuntimeValueData::Int(lhs), ConstRuntimeValueData::Float(rhs)) => {
            (*lhs as f64).partial_cmp(rhs).unwrap_or(Ordering::Equal)
        }
        (ConstRuntimeValueData::Float(lhs), ConstRuntimeValueData::Int(rhs)) => {
            lhs.partial_cmp(&(*rhs as f64)).unwrap_or(Ordering::Equal)
        }
        _ => {
            let lhs_string = const_runtime_string(lhs);
            let rhs_string = const_runtime_string(rhs);
            match (lhs_string, rhs_string) {
                (Some(lhs), Some(rhs)) => lhs.cmp(&rhs),
                _ => const_runtime_kind_order(lhs).cmp(&const_runtime_kind_order(rhs)),
            }
        }
    }
}

fn const_runtime_string(value: &ConstRuntimeValueData) -> Option<String> {
    match value {
        ConstRuntimeValueData::ShortStr(value) => Some(value.as_str().to_string()),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn const_runtime_kind_order(value: &ConstRuntimeValueData) -> u8 {
    match value {
        ConstRuntimeValueData::Nil => 0,
        ConstRuntimeValueData::Bool(_) => 1,
        ConstRuntimeValueData::Int(_) => 2,
        ConstRuntimeValueData::Float(_) => 3,
        ConstRuntimeValueData::ShortStr(_) => 4,
        ConstRuntimeValueData::Heap(_) => 5,
    }
}

fn const_runtime_values_equal(lhs: &ConstRuntimeValueData, rhs: &ConstRuntimeValueData) -> bool {
    match (lhs, rhs) {
        (ConstRuntimeValueData::Nil, ConstRuntimeValueData::Nil) => true,
        (ConstRuntimeValueData::Bool(lhs), ConstRuntimeValueData::Bool(rhs)) => lhs == rhs,
        (ConstRuntimeValueData::Int(lhs), ConstRuntimeValueData::Int(rhs)) => lhs == rhs,
        (ConstRuntimeValueData::Float(lhs), ConstRuntimeValueData::Float(rhs)) => lhs == rhs,
        (ConstRuntimeValueData::ShortStr(lhs), ConstRuntimeValueData::ShortStr(rhs)) => lhs == rhs,
        (ConstRuntimeValueData::ShortStr(lhs), ConstRuntimeValueData::Heap(rhs))
        | (ConstRuntimeValueData::Heap(rhs), ConstRuntimeValueData::ShortStr(lhs)) => {
            matches!(rhs.as_ref(), ConstHeapValueData::LongString(rhs) if lhs == rhs)
        }
        (ConstRuntimeValueData::Heap(lhs), ConstRuntimeValueData::Heap(rhs)) => {
            const_heap_values_equal(lhs.as_ref(), rhs.as_ref())
        }
        _ => false,
    }
}

fn const_heap_values_equal(lhs: &ConstHeapValueData, rhs: &ConstHeapValueData) -> bool {
    match (lhs, rhs) {
        (ConstHeapValueData::LongString(lhs), ConstHeapValueData::LongString(rhs)) => lhs == rhs,
        (ConstHeapValueData::List(lhs), ConstHeapValueData::List(rhs)) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs)
                    .all(|(lhs, rhs)| const_runtime_values_equal(lhs, rhs))
        }
        (ConstHeapValueData::Map(lhs), ConstHeapValueData::Map(rhs)) => {
            lhs.len() == rhs.len()
                && lhs.iter().zip(rhs).all(|((lhs_key, lhs_value), (rhs_key, rhs_value))| {
                    lhs_key == rhs_key && const_runtime_values_equal(lhs_value, rhs_value)
                })
        }
        (ConstHeapValueData::UpvalCell(lhs), ConstHeapValueData::UpvalCell(rhs)) => {
            const_runtime_values_equal(lhs, rhs)
        }
        _ => false,
    }
}
