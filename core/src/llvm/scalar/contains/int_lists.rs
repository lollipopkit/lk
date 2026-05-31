use crate::llvm::{
    callee_eval::native_straightline_function_return,
    scalar::block_helpers::{local_static_container_before, local_static_i64_before},
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeStraightlineValue, native_const_runtime_value,
        native_runtime_const_value, native_runtime_string_key_kind, native_static_global, native_static_index,
        native_static_list_from_values, native_straightline_heap_const_value,
    },
};
use crate::vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32, Module32Artifact, Opcode32};

use super::{local_static_index_value_before, local_static_map_rest_before};

pub(in crate::llvm) fn static_int_list_values(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    value: &NativeStraightlineValue,
) -> Option<Vec<i64>> {
    match value {
        NativeStraightlineValue::List { elements, .. } => elements
            .iter()
            .map(|value| match value {
                ConstRuntimeValue32Data::Int(value) => Some(*value),
                _ => None,
            })
            .collect(),
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        } => {
            let instr = *code.get(*id)?;
            match instr.opcode() {
                Opcode32::LoadHeapConst => {
                    let Some(ConstHeapValue32Data::List(values)) = heap_values.get(instr.bx() as usize) else {
                        return None;
                    };
                    if values.is_empty() {
                        return None;
                    }
                    values
                        .iter()
                        .map(|value| match value {
                            ConstRuntimeValue32Data::Int(value) => Some(*value),
                            _ => None,
                        })
                        .collect()
                }
                Opcode32::SliceFrom => {
                    let target = local_static_container_before(code, heap_values, *id, instr.b())
                        .or_else(|| local_static_map_rest_before(code, strings, heap_values, *id, instr.b()))
                        .or_else(|| {
                            local_static_index_value_before(code, int_consts, strings, heap_values, *id, instr.b())
                        })?;
                    let NativeStraightlineValue::I64(start) =
                        local_static_i64_before(code, int_consts, *id, instr.c())?
                    else {
                        return None;
                    };
                    let start = start.parse::<usize>().ok()?;
                    Some(
                        static_int_list_values(code, int_consts, strings, heap_values, &target)?
                            .into_iter()
                            .skip(start)
                            .collect(),
                    )
                }
                _ => None,
            }
        }
        _ => None,
    }
}

pub(in crate::llvm) fn static_int_list_index_value(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: &NativeStraightlineValue,
    key: &NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let values = static_int_list_values(code, int_consts, strings, heap_values, target)?;
    if let NativeStraightlineValue::List { elements, .. } = key {
        let (start, end) = static_int_list_range_bounds(elements, values.len())?;
        let values = values[start..end]
            .iter()
            .map(|value| NativeStraightlineValue::I64(value.to_string()))
            .collect::<Vec<_>>();
        return native_static_list_from_values(&values, String::new());
    }
    let NativeStraightlineValue::I64(index) = key else {
        return None;
    };
    let index = static_int_list_index(index.parse().ok()?, values.len())?;
    let value = *values.get(index)?;
    Some(NativeStraightlineValue::I64(value.to_string()))
}

pub(in crate::llvm) fn static_int_list_filter_map_method(
    artifact: &Module32Artifact,
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    method: &str,
    callable: NativeStraightlineValue,
    static_globals: &mut [Option<NativeStraightlineValue>],
    ir: &mut String,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if method != "filter" && method != "map" {
        return None;
    }
    if let NativeStraightlineValue::List { elements, .. } = &target {
        let (function_index, captures) = static_callable_parts(callable)?;
        let mut out = Vec::new();
        for element in elements {
            let arg = native_const_runtime_value(element, String::new())?;
            let result = native_straightline_function_return(
                artifact,
                function_index as usize,
                &[arg],
                &captures,
                static_globals,
                0,
                ir,
                tmp_index,
            )
            .ok()??;
            match method {
                "filter" if native_filter_truthy(&result)? => out.push(element.clone()),
                "filter" => {}
                "map" => out.push(native_runtime_const_value(&result)?),
                _ => return None,
            }
        }
        return Some(NativeStraightlineValue::List {
            value: String::new(),
            symbol: String::new(),
            elements: out,
        });
    }
    let values = static_int_list_values(code, int_consts, strings, heap_values, &target)?;
    let (function_index, captures) = static_callable_parts(callable)?;
    let mut out = Vec::new();
    for value in values {
        let result = native_straightline_function_return(
            artifact,
            function_index as usize,
            &[NativeStraightlineValue::I64(value.to_string())],
            &captures,
            static_globals,
            0,
            ir,
            tmp_index,
        )
        .ok()??;
        match method {
            "filter" if native_filter_truthy(&result)? => out.push(ConstRuntimeValue32Data::Int(value)),
            "filter" => {}
            "map" => out.push(ConstRuntimeValue32Data::Int(native_static_i64(&result)?)),
            _ => return None,
        }
    }
    Some(NativeStraightlineValue::List {
        value: String::new(),
        symbol: String::new(),
        elements: out,
    })
}

fn static_callable_parts(callable: NativeStraightlineValue) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match callable {
        NativeStraightlineValue::Function(index) => Some((index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((function_index, captures)),
        _ => None,
    }
}

pub(in crate::llvm) fn static_int_list_reduce_method(
    artifact: &Module32Artifact,
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    args: &[NativeStraightlineValue],
    static_globals: &mut [Option<NativeStraightlineValue>],
    ir: &mut String,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [initial, callable] = args else {
        return None;
    };
    if let NativeStraightlineValue::List { elements, .. } = &target {
        let (function_index, captures) = static_callable_parts(callable.clone())?;
        let mut acc = initial.clone();
        for element in elements {
            let item = native_const_runtime_value(element, String::new())?;
            acc = native_straightline_function_return(
                artifact,
                function_index as usize,
                &[acc, item],
                &captures,
                static_globals,
                0,
                ir,
                tmp_index,
            )
            .ok()??;
        }
        return Some(acc);
    }
    let values = static_int_list_values(code, int_consts, strings, heap_values, &target)?;
    let mut acc = native_static_i64(initial)?;
    let (function_index, captures) = match callable.clone() {
        NativeStraightlineValue::Function(index) => (index, Vec::new()),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => (function_index, captures),
        _ => return None,
    };
    for value in values {
        let result = native_straightline_function_return(
            artifact,
            function_index as usize,
            &[
                NativeStraightlineValue::I64(acc.to_string()),
                NativeStraightlineValue::I64(value.to_string()),
            ],
            &captures,
            static_globals,
            0,
            ir,
            tmp_index,
        )
        .ok()??;
        acc = native_static_i64(&result)?;
    }
    Some(NativeStraightlineValue::I64(acc.to_string()))
}

pub(in crate::llvm) fn static_int_list_chunk_method(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    size_value: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let values = static_int_list_values(code, int_consts, strings, heap_values, &target)?;
    let size = native_static_i64(&size_value).or_else(|| {
        static_int_list_values(code, int_consts, strings, heap_values, &size_value)
            .and_then(|values| values.first().copied())
    })?;
    if size <= 0 {
        return None;
    }
    let mut elements = Vec::new();
    for chunk in values.chunks(size as usize) {
        elements.push(ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(
            chunk.iter().map(|value| ConstRuntimeValue32Data::Int(*value)).collect(),
        ))));
    }
    Some(NativeStraightlineValue::List {
        value: String::new(),
        symbol: String::new(),
        elements,
    })
}

pub(in crate::llvm) fn static_int_list_zip_method(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    args: &[ConstRuntimeValue32Data],
) -> Option<NativeStraightlineValue> {
    let lhs = static_int_list_values(code, int_consts, strings, heap_values, &target)?;
    let rhs_values;
    let rhs = if let Some(ConstRuntimeValue32Data::Heap(rhs)) = args.first() {
        let ConstHeapValue32Data::List(rhs) = rhs.as_ref() else {
            return None;
        };
        rhs
    } else {
        rhs_values = args.to_vec();
        &rhs_values
    };
    let elements = lhs
        .into_iter()
        .zip(rhs.iter().cloned())
        .map(|(lhs, rhs)| {
            ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(vec![
                ConstRuntimeValue32Data::Int(lhs),
                rhs,
            ])))
        })
        .collect();
    Some(NativeStraightlineValue::List {
        value: String::new(),
        symbol: String::new(),
        elements,
    })
}

pub(in crate::llvm) fn static_int_list_single_arg_method(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    method: &str,
    args: &[ConstRuntimeValue32Data],
) -> Option<NativeStraightlineValue> {
    let lhs = static_int_list_values(code, int_consts, strings, heap_values, &target)?;
    match method {
        "take" | "skip" => {
            let ConstRuntimeValue32Data::Int(count) = args.first()? else {
                return None;
            };
            let count = usize::try_from((*count).max(0)).ok()?;
            let iter: Box<dyn Iterator<Item = i64>> = if method == "take" {
                Box::new(lhs.into_iter().take(count))
            } else {
                Box::new(lhs.into_iter().skip(count))
            };
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements: iter.map(ConstRuntimeValue32Data::Int).collect(),
            })
        }
        "concat" | "chain" => {
            let ConstRuntimeValue32Data::Heap(rhs) = args.first()? else {
                return None;
            };
            let ConstHeapValue32Data::List(rhs) = rhs.as_ref() else {
                return None;
            };
            let mut elements = lhs.into_iter().map(ConstRuntimeValue32Data::Int).collect::<Vec<_>>();
            elements.extend(rhs.iter().cloned());
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements,
            })
        }
        _ => None,
    }
}

pub(in crate::llvm) fn static_list_empty_arg_method(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    method: &str,
) -> Option<NativeStraightlineValue> {
    match method {
        "unique" => {
            if let NativeStraightlineValue::List { elements, .. } = target {
                let mut seen_keys = Vec::new();
                let mut seen = Vec::new();
                for value in elements {
                    let key = const_runtime_unique_key(&value)?;
                    if !seen_keys.contains(&key) {
                        seen_keys.push(key);
                        seen.push(value);
                    }
                }
                return Some(NativeStraightlineValue::List {
                    value: String::new(),
                    symbol: String::new(),
                    elements: seen,
                });
            }
            let mut seen = Vec::new();
            for value in static_int_list_values(code, int_consts, strings, heap_values, &target)? {
                if !seen.contains(&value) {
                    seen.push(value);
                }
            }
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements: seen.into_iter().map(ConstRuntimeValue32Data::Int).collect(),
            })
        }
        "flatten" => {
            let NativeStraightlineValue::List { elements, .. } = target else {
                return None;
            };
            let mut out = Vec::new();
            for element in elements {
                let ConstRuntimeValue32Data::Heap(value) = element else {
                    return None;
                };
                let ConstHeapValue32Data::List(values) = value.as_ref() else {
                    return None;
                };
                out.extend(values.iter().cloned());
            }
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements: out,
            })
        }
        "enumerate" => {
            let NativeStraightlineValue::List { elements, .. } = target else {
                return None;
            };
            let elements = elements
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(vec![
                        ConstRuntimeValue32Data::Int(index as i64),
                        value,
                    ])))
                })
                .collect();
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements,
            })
        }
        _ => None,
    }
}

fn const_runtime_unique_key(value: &ConstRuntimeValue32Data) -> Option<String> {
    match value {
        ConstRuntimeValue32Data::Nil => Some("nil:".to_string()),
        ConstRuntimeValue32Data::Bool(value) => Some(format!("bool:{value}")),
        ConstRuntimeValue32Data::Int(value) => Some(format!("int:{value}")),
        ConstRuntimeValue32Data::Float(value) => Some(format!("float:{value:?}")),
        ConstRuntimeValue32Data::ShortStr(value) => Some(format!("str:{value}")),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => Some(format!("str:{value}")),
            _ => None,
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn static_iter_builtin_call(
    artifact: &Module32Artifact,
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    static_globals: &mut [Option<NativeStraightlineValue>],
    ir: &mut String,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match builtin {
        NativeBuiltin::IterMap | NativeBuiltin::IterFilter => {
            let [target, callable] = args else {
                return None;
            };
            let method = if builtin == NativeBuiltin::IterMap {
                "map"
            } else {
                "filter"
            };
            static_int_list_filter_map_method(
                artifact,
                code,
                int_consts,
                strings,
                heap_values,
                target.clone(),
                method,
                callable.clone(),
                static_globals,
                ir,
                tmp_index,
            )
        }
        NativeBuiltin::IterReduce => {
            let [target, initial, callable] = args else {
                return None;
            };
            static_int_list_reduce_method(
                artifact,
                code,
                int_consts,
                strings,
                heap_values,
                target.clone(),
                &[initial.clone(), callable.clone()],
                static_globals,
                ir,
                tmp_index,
            )
        }
        NativeBuiltin::IterChunk => {
            let [target, size] = args else {
                return None;
            };
            static_int_list_chunk_method(code, int_consts, strings, heap_values, target.clone(), size.clone())
        }
        NativeBuiltin::IterTake | NativeBuiltin::IterSkip => {
            let [target, count] = args else {
                return None;
            };
            let lhs = static_int_list_values(code, int_consts, strings, heap_values, target)?;
            let count = usize::try_from(native_static_i64(count)?.max(0)).ok()?;
            let iter: Box<dyn Iterator<Item = i64>> = if builtin == NativeBuiltin::IterTake {
                Box::new(lhs.into_iter().take(count))
            } else {
                Box::new(lhs.into_iter().skip(count))
            };
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements: iter.map(ConstRuntimeValue32Data::Int).collect(),
            })
        }
        NativeBuiltin::IterChain => {
            let [target, rhs] = args else {
                return None;
            };
            let mut elements = static_int_list_values(code, int_consts, strings, heap_values, target)?
                .into_iter()
                .map(ConstRuntimeValue32Data::Int)
                .collect::<Vec<_>>();
            if let Some(rhs) = static_int_list_values(code, int_consts, strings, heap_values, rhs) {
                elements.extend(rhs.into_iter().map(ConstRuntimeValue32Data::Int));
            } else if let NativeStraightlineValue::List { elements: rhs, .. } = rhs {
                elements.extend(rhs.iter().cloned());
            } else {
                return None;
            }
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements,
            })
        }
        NativeBuiltin::IterFlatten => {
            let [target] = args else {
                return None;
            };
            let NativeStraightlineValue::List { elements, .. } = target else {
                return None;
            };
            let mut out = Vec::new();
            for element in elements {
                if let ConstRuntimeValue32Data::Heap(value) = element
                    && let ConstHeapValue32Data::List(values) = value.as_ref()
                {
                    out.extend(values.iter().cloned());
                } else {
                    out.push(element.clone());
                }
            }
            Some(NativeStraightlineValue::List {
                value: String::new(),
                symbol: String::new(),
                elements: out,
            })
        }
        NativeBuiltin::IterUnique => {
            let [target] = args else {
                return None;
            };
            static_list_empty_arg_method(code, int_consts, strings, heap_values, target.clone(), "unique")
        }
        NativeBuiltin::IterEnumerate => {
            let [target] = args else {
                return None;
            };
            if let Some(values) = static_int_list_values(code, int_consts, strings, heap_values, target) {
                let elements = values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(vec![
                            ConstRuntimeValue32Data::Int(index as i64),
                            ConstRuntimeValue32Data::Int(value),
                        ])))
                    })
                    .collect();
                Some(NativeStraightlineValue::List {
                    value: String::new(),
                    symbol: String::new(),
                    elements,
                })
            } else {
                static_list_empty_arg_method(code, int_consts, strings, heap_values, target.clone(), "enumerate")
            }
        }
        NativeBuiltin::IterZip => {
            let [target, NativeStraightlineValue::List { elements, .. }] = args else {
                return None;
            };
            static_int_list_zip_method(code, int_consts, strings, heap_values, target.clone(), elements)
        }
        _ => None,
    }
}

pub(in crate::llvm) fn local_static_iter_zip_before(
    global_names: &[String],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    for prev_pc in (0..pc).rev() {
        let instr = *code.get(prev_pc)?;
        if instr.a() != reg {
            continue;
        }
        return match instr.opcode() {
            Opcode32::Move => local_static_iter_zip_before(
                global_names,
                code,
                int_consts,
                strings,
                heap_values,
                prev_pc,
                instr.b(),
                static_regs,
            ),
            Opcode32::Call if instr.a() == instr.b() && instr.c() == 2 => {
                let NativeStraightlineValue::Builtin(NativeBuiltin::IterZip) = local_static_value_before(
                    global_names,
                    code,
                    strings,
                    heap_values,
                    prev_pc,
                    instr.b(),
                    static_regs,
                )?
                else {
                    return None;
                };
                let lhs = local_static_value_before(
                    global_names,
                    code,
                    strings,
                    heap_values,
                    prev_pc,
                    instr.b().checked_add(1)?,
                    static_regs,
                )?;
                let NativeStraightlineValue::List { elements, .. } = local_static_value_before(
                    global_names,
                    code,
                    strings,
                    heap_values,
                    prev_pc,
                    instr.b().checked_add(2)?,
                    static_regs,
                )?
                else {
                    return None;
                };
                static_int_list_zip_method(code, int_consts, strings, heap_values, lhs, &elements)
            }
            _ => None,
        };
    }
    None
}

fn local_static_value_before(
    global_names: &[String],
    code: &[Instr32],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
    static_regs: &[Option<NativeStraightlineValue>],
) -> Option<NativeStraightlineValue> {
    if let Some(value) = static_regs.get(reg as usize).cloned().flatten() {
        return Some(value);
    }
    for prev_pc in (0..pc).rev() {
        let instr = *code.get(prev_pc)?;
        if instr.a() != reg {
            continue;
        }
        return match instr.opcode() {
            Opcode32::Move => local_static_value_before(
                global_names,
                code,
                strings,
                heap_values,
                prev_pc,
                instr.b(),
                static_regs,
            ),
            Opcode32::GetGlobal => native_static_global(global_names.get(instr.bx() as usize)?),
            Opcode32::LoadString => {
                let value = strings.get(instr.bx() as usize)?.clone();
                Some(NativeStraightlineValue::String {
                    symbol: String::new(),
                    len: value.chars().count(),
                    key_kind: native_runtime_string_key_kind(&value),
                    value,
                })
            }
            Opcode32::LoadHeapConst => {
                native_straightline_heap_const_value(0, instr.bx(), heap_values.get(instr.bx() as usize)?)
            }
            Opcode32::GetIndex => {
                let target = local_static_value_before(
                    global_names,
                    code,
                    strings,
                    heap_values,
                    prev_pc,
                    instr.b(),
                    static_regs,
                )?;
                let key = local_static_value_before(
                    global_names,
                    code,
                    strings,
                    heap_values,
                    prev_pc,
                    instr.c(),
                    static_regs,
                )?;
                native_static_index(target, key, String::new())
            }
            _ => None,
        };
    }
    None
}

fn native_filter_truthy(value: &NativeStraightlineValue) -> Option<bool> {
    match value {
        NativeStraightlineValue::Bool(value) | NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            Some(value != "0")
        }
        NativeStraightlineValue::Nil => Some(false),
        _ => None,
    }
}

fn native_static_i64(value: &NativeStraightlineValue) -> Option<i64> {
    match value {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => value.parse().ok(),
        _ => None,
    }
}

fn static_int_list_index(index: i64, len: usize) -> Option<usize> {
    if index < 0 {
        usize::try_from((len as i64).checked_add(index)?).ok()
    } else {
        usize::try_from(index).ok()
    }
}

fn static_int_list_range_bounds(elements: &[ConstRuntimeValue32Data], len: usize) -> Option<(usize, usize)> {
    if elements.is_empty() || elements.len() > 3 {
        return None;
    }
    let ConstRuntimeValue32Data::Int(start) = elements.first()? else {
        return None;
    };
    let last = elements.last().and_then(|value| match value {
        ConstRuntimeValue32Data::Int(value) => value.checked_add(1),
        _ => None,
    });
    let start = static_int_list_index((*start).max(-(len as i64)), len)?;
    let end = static_int_list_index(last.unwrap_or(len as i64).max(-(len as i64)), len)?.min(len);
    Some((start.min(end), end))
}

pub(in crate::llvm) fn static_dynamic_int_list_slice(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    target: NativeStraightlineValue,
    start: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let (
        NativeStraightlineValue::DynamicList {
            id,
            element: NativeListElementKind::I64,
        },
        NativeStraightlineValue::I64(start),
    ) = (target, start)
    else {
        return None;
    };
    let instr = *code.get(id)?;
    let Opcode32::LoadHeapConst = instr.opcode() else {
        return None;
    };
    let Some(ConstHeapValue32Data::List(values)) = heap_values.get(instr.bx() as usize) else {
        return None;
    };
    let values = values
        .iter()
        .skip(start.parse::<usize>().ok()?)
        .map(|value| match value {
            ConstRuntimeValue32Data::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    native_static_list_from_values(&values, symbol)
}

pub(in crate::llvm) fn static_dynamic_int_list_contains(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    needle: NativeStraightlineValue,
    haystack: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::DynamicList {
        id,
        element: NativeListElementKind::I64,
    } = haystack
    else {
        return None;
    };
    let needle = match needle {
        NativeStraightlineValue::I64(needle) => Some(needle.parse::<i64>().ok()?),
        _ => None,
    };
    let instr = *code.get(id)?;
    let Opcode32::LoadHeapConst = instr.opcode() else {
        return None;
    };
    let Some(ConstHeapValue32Data::List(values)) = heap_values.get(instr.bx() as usize) else {
        return None;
    };
    let contains = needle.is_some_and(|needle| {
        values
            .iter()
            .any(|value| matches!(value, ConstRuntimeValue32Data::Int(value) if *value == needle))
    });
    Some(NativeStraightlineValue::Bool(i64::from(contains).to_string()))
}
