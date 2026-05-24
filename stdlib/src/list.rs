use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs32, NativeRuntime32, RuntimeExport32},
};

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct ListModule;

impl Default for ListModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ListModule {
    pub fn new() -> Self {
        Self
    }

    fn len32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let list = one_list(args, runtime, "len()")?;
        Ok(RuntimeVal::Int(list.len() as i64))
    }

    fn push32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "push()")?;
        let values = args.as_slice();
        let plan = list_push_preserving_backing(
            list_arg(&values[0], runtime.heap(), "push() first argument")?,
            values[1].clone(),
            runtime.heap(),
        );
        let typed = plan.into_typed(runtime.heap_mut());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(typed))))
    }

    fn concat32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "concat()")?;
        let values = args.as_slice();
        let plan = list_concat_preserving_backing(
            list_arg(&values[0], runtime.heap(), "concat() first argument")?,
            list_arg(&values[1], runtime.heap(), "concat() second argument")?,
        );
        let typed = plan.into_typed(runtime.heap_mut());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(typed))))
    }

    fn join32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "join()")?;
        let values = args.as_slice();
        let strings = string_list_arg(&values[0], runtime.heap(), "join() first argument")?;
        let delimiter = runtime_string_arg(&values[1], runtime.heap(), "join() second argument")?;
        Ok(runtime_string_value(
            &strings.join(delimiter.as_ref()),
            runtime.heap_mut(),
        ))
    }

    fn get32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "get()")?;
        let values = args.as_slice();
        let list = list_arg(&values[0], runtime.heap(), "get() first argument")?;
        let index = int_arg(&values[1], "get() index")?;
        if index < 0 {
            return Ok(RuntimeVal::Nil);
        }
        Ok(list_get_item(list, index as usize)
            .map(|item| item.into_runtime(runtime.heap_mut()))
            .unwrap_or(RuntimeVal::Nil))
    }

    fn first32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let list = one_list(args, runtime, "first()")?;
        Ok(list_get_item(list, 0)
            .map(|item| item.into_runtime(runtime.heap_mut()))
            .unwrap_or(RuntimeVal::Nil))
    }

    fn last32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let list = one_list(args, runtime, "last()")?;
        let Some(index) = list.len().checked_sub(1) else {
            return Ok(RuntimeVal::Nil);
        };
        Ok(list_get_item(list, index)
            .map(|item| item.into_runtime(runtime.heap_mut()))
            .unwrap_or(RuntimeVal::Nil))
    }

    fn set32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 3, "set()")?;
        let values = args.as_slice();
        let index = int_arg(&values[1], "set() index")?;
        if index < 0 {
            bail!("set() index must be non-negative");
        }
        let plan = list_set_preserving_backing(
            list_arg(&values[0], runtime.heap(), "set() first argument")?,
            index as usize,
            values[2].clone(),
            runtime.heap(),
        )?;
        let (updated_list, old) = plan.into_typed(runtime.heap_mut());
        let updated = RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(updated_list)));
        Ok(RuntimeVal::Obj(
            runtime
                .heap_mut()
                .alloc(HeapValue::List(TypedList::Mixed(vec![updated, old]))),
        ))
    }
}

impl Module for ListModule {
    fn name(&self) -> &str {
        "list"
    }

    fn description(&self) -> &str {
        "List utilities"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("len", Self::len32, 1),
                RuntimeNativeExport32::plain("push", Self::push32, 2),
                RuntimeNativeExport32::plain("concat", Self::concat32, 2),
                RuntimeNativeExport32::plain("join", Self::join32, 2),
                RuntimeNativeExport32::plain("get", Self::get32, 2),
                RuntimeNativeExport32::plain("first", Self::first32, 1),
                RuntimeNativeExport32::plain("last", Self::last32, 1),
                RuntimeNativeExport32::plain("set", Self::set32, 3),
            ],
            &[],
        ))
    }
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} takes exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
    }
}

fn one_list<'a>(args: NativeArgs32<'a>, runtime: &'a NativeRuntime32<'a>, name: &str) -> Result<&'a TypedList> {
    expect_arity(args, 1, name)?;
    list_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn list_arg<'a>(value: &RuntimeVal, heap: &'a HeapStore, context: &str) -> Result<&'a TypedList> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} argument must be a list");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(list),
        other => Err(anyhow!("{context} argument must be a list, got {}", other.type_name())),
    }
}

enum ListItem32 {
    Value(RuntimeVal),
    String(Arc<str>),
}

impl ListItem32 {
    fn into_runtime(self, heap: &mut HeapStore) -> RuntimeVal {
        match self {
            Self::Value(value) => value,
            Self::String(value) => runtime_string_value(value.as_ref(), heap),
        }
    }
}

fn list_get_item(list: &TypedList, index: usize) -> Option<ListItem32> {
    match list {
        TypedList::Mixed(values) => values.get(index).cloned().map(ListItem32::Value),
        TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int).map(ListItem32::Value),
        TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float).map(ListItem32::Value),
        TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool).map(ListItem32::Value),
        TypedList::String(values) => values.get(index).cloned().map(ListItem32::String),
    }
}

enum ListPushPlan {
    Ready(TypedList),
    MaterializeString { values: Vec<Arc<str>>, value: RuntimeVal },
}

impl ListPushPlan {
    fn into_typed(self, heap: &mut HeapStore) -> TypedList {
        match self {
            Self::Ready(list) => list,
            Self::MaterializeString { values, value } => {
                let mut mixed = Vec::with_capacity(values.len() + 1);
                append_string_values(values, &mut mixed, heap);
                mixed.push(value);
                TypedList::Mixed(mixed)
            }
        }
    }
}

fn list_push_preserving_backing(list: &TypedList, value: RuntimeVal, heap: &HeapStore) -> ListPushPlan {
    match list {
        TypedList::Mixed(values) => {
            let mut out = Vec::with_capacity(values.len() + 1);
            out.extend_from_slice(values);
            out.push(value);
            ListPushPlan::Ready(TypedList::Mixed(out))
        }
        TypedList::Int(values) => match value {
            RuntimeVal::Int(value) => ListPushPlan::Ready(TypedList::Int(copy_with_extra_item(values, value))),
            value => ListPushPlan::Ready(TypedList::Mixed(copy_numeric_with_extra_mixed(
                values,
                value,
                RuntimeVal::Int,
            ))),
        },
        TypedList::Float(values) => match value {
            RuntimeVal::Float(value) => ListPushPlan::Ready(TypedList::Float(copy_with_extra_item(values, value))),
            value => ListPushPlan::Ready(TypedList::Mixed(copy_numeric_with_extra_mixed(
                values,
                value,
                RuntimeVal::Float,
            ))),
        },
        TypedList::Bool(values) => match value {
            RuntimeVal::Bool(value) => ListPushPlan::Ready(TypedList::Bool(copy_with_extra_item(values, value))),
            value => ListPushPlan::Ready(TypedList::Mixed(copy_numeric_with_extra_mixed(
                values,
                value,
                RuntimeVal::Bool,
            ))),
        },
        TypedList::String(values) => match runtime_string_value_arg(&value, heap) {
            Some(value) => ListPushPlan::Ready(TypedList::String(copy_with_extra_item(values, value))),
            None => ListPushPlan::MaterializeString {
                values: copy_slice(values),
                value,
            },
        },
    }
}

enum RuntimeListSnapshot {
    Mixed(Vec<RuntimeVal>),
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
}

impl RuntimeListSnapshot {
    fn from_typed(list: &TypedList) -> Self {
        match list {
            TypedList::Mixed(values) => Self::Mixed(copy_slice(values)),
            TypedList::Int(values) => Self::Int(copy_slice(values)),
            TypedList::Float(values) => Self::Float(copy_slice(values)),
            TypedList::Bool(values) => Self::Bool(copy_slice(values)),
            TypedList::String(values) => Self::String(copy_slice(values)),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::String(values) => values.len(),
        }
    }

    fn append_to_mixed_output(self, out: &mut Vec<RuntimeVal>, heap: &mut HeapStore) {
        match self {
            Self::Mixed(values) => out.extend(values),
            Self::Int(values) => out.extend(values.into_iter().map(RuntimeVal::Int)),
            Self::Float(values) => out.extend(values.into_iter().map(RuntimeVal::Float)),
            Self::Bool(values) => out.extend(values.into_iter().map(RuntimeVal::Bool)),
            Self::String(values) => out.extend(
                values
                    .into_iter()
                    .map(|value| runtime_string_value(value.as_ref(), heap)),
            ),
        }
    }
}

enum ListConcatPlan {
    Ready(TypedList),
    Mixed {
        left: RuntimeListSnapshot,
        right: RuntimeListSnapshot,
    },
}

impl ListConcatPlan {
    fn into_typed(self, heap: &mut HeapStore) -> TypedList {
        match self {
            Self::Ready(list) => list,
            Self::Mixed { left, right } => {
                let mut mixed = Vec::with_capacity(left.len() + right.len());
                left.append_to_mixed_output(&mut mixed, heap);
                right.append_to_mixed_output(&mut mixed, heap);
                TypedList::Mixed(mixed)
            }
        }
    }
}

fn list_concat_preserving_backing(left: &TypedList, right: &TypedList) -> ListConcatPlan {
    match (left, right) {
        (TypedList::Int(left), TypedList::Int(right)) => {
            ListConcatPlan::Ready(TypedList::Int(copy_concat(left, right)))
        }
        (TypedList::Float(left), TypedList::Float(right)) => {
            ListConcatPlan::Ready(TypedList::Float(copy_concat(left, right)))
        }
        (TypedList::Bool(left), TypedList::Bool(right)) => {
            ListConcatPlan::Ready(TypedList::Bool(copy_concat(left, right)))
        }
        (TypedList::String(left), TypedList::String(right)) => {
            ListConcatPlan::Ready(TypedList::String(copy_concat(left, right)))
        }
        (left, right) => ListConcatPlan::Mixed {
            left: RuntimeListSnapshot::from_typed(left),
            right: RuntimeListSnapshot::from_typed(right),
        },
    }
}

fn copy_with_extra_item<T: Clone>(values: &[T], value: T) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len() + 1);
    out.extend_from_slice(values);
    out.push(value);
    out
}

fn copy_numeric_with_extra_mixed<T: Copy>(
    values: &[T],
    value: RuntimeVal,
    wrap: impl Fn(T) -> RuntimeVal,
) -> Vec<RuntimeVal> {
    let mut out = Vec::with_capacity(values.len() + 1);
    for value in values {
        out.push(wrap(*value));
    }
    out.push(value);
    out
}

fn copy_concat<T: Clone>(left: &[T], right: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    out.extend_from_slice(left);
    out.extend_from_slice(right);
    out
}

fn copy_replace<T: Clone>(values: &[T], index: usize, value: T) -> Result<(Vec<T>, T)> {
    let Some(old) = values.get(index).cloned() else {
        bail!("list index {} out of bounds", index);
    };
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(&values[..index]);
    out.push(value);
    out.extend_from_slice(&values[index + 1..]);
    Ok((out, old))
}

enum ListSetPlan {
    Ready {
        list: TypedList,
        old: RuntimeVal,
    },
    StringReady {
        list: TypedList,
        old: Arc<str>,
    },
    MaterializeString {
        values: Vec<Arc<str>>,
        index: usize,
        value: RuntimeVal,
    },
}

impl ListSetPlan {
    fn into_typed(self, heap: &mut HeapStore) -> (TypedList, RuntimeVal) {
        match self {
            Self::Ready { list, old } => (list, old),
            Self::StringReady { list, old } => (list, runtime_string_value(old.as_ref(), heap)),
            Self::MaterializeString { values, index, value } => {
                let mut mixed = Vec::with_capacity(values.len());
                append_string_values(values, &mut mixed, heap);
                set_polluted_list(mixed, index, value, 0).expect("index was checked before materializing string list")
            }
        }
    }
}

fn list_set_preserving_backing(
    list: &TypedList,
    index: usize,
    value: RuntimeVal,
    heap: &HeapStore,
) -> Result<ListSetPlan> {
    match list {
        TypedList::Mixed(values) => {
            let (values, old) = copy_replace(values, index, value)?;
            Ok(ListSetPlan::Ready {
                list: TypedList::Mixed(values),
                old,
            })
        }
        TypedList::Int(values) => match value {
            RuntimeVal::Int(value) => {
                let (values, old) = copy_replace(values, index, value)?;
                Ok(ListSetPlan::Ready {
                    list: TypedList::Int(values),
                    old: RuntimeVal::Int(old),
                })
            }
            value => set_polluted_list(values.iter().copied().map(RuntimeVal::Int), index, value, values.len())
                .map(|(list, old)| ListSetPlan::Ready { list, old }),
        },
        TypedList::Float(values) => match value {
            RuntimeVal::Float(value) => {
                let (values, old) = copy_replace(values, index, value)?;
                Ok(ListSetPlan::Ready {
                    list: TypedList::Float(values),
                    old: RuntimeVal::Float(old),
                })
            }
            value => set_polluted_list(
                values.iter().copied().map(RuntimeVal::Float),
                index,
                value,
                values.len(),
            )
            .map(|(list, old)| ListSetPlan::Ready { list, old }),
        },
        TypedList::Bool(values) => match value {
            RuntimeVal::Bool(value) => {
                let (values, old) = copy_replace(values, index, value)?;
                Ok(ListSetPlan::Ready {
                    list: TypedList::Bool(values),
                    old: RuntimeVal::Bool(old),
                })
            }
            value => set_polluted_list(values.iter().copied().map(RuntimeVal::Bool), index, value, values.len())
                .map(|(list, old)| ListSetPlan::Ready { list, old }),
        },
        TypedList::String(values) => match runtime_string_value_arg(&value, heap) {
            Some(value) => {
                let (values, old) = copy_replace(values, index, value)?;
                Ok(ListSetPlan::StringReady {
                    list: TypedList::String(values),
                    old,
                })
            }
            None => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                Ok(ListSetPlan::MaterializeString {
                    values: copy_slice(values),
                    index,
                    value,
                })
            }
        },
    }
}

fn set_polluted_list(
    items: impl IntoIterator<Item = RuntimeVal>,
    index: usize,
    value: RuntimeVal,
    len_hint: usize,
) -> Result<(TypedList, RuntimeVal)> {
    let mut values = Vec::with_capacity(len_hint);
    values.extend(items);
    let Some(slot) = values.get_mut(index) else {
        bail!("list index {} out of bounds", index);
    };
    let old = std::mem::replace(slot, value);
    Ok((TypedList::Mixed(values), old))
}

fn copy_slice<T: Clone>(values: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(values);
    out
}

fn append_string_values(values: Vec<Arc<str>>, out: &mut Vec<RuntimeVal>, heap: &mut HeapStore) {
    out.extend(
        values
            .into_iter()
            .map(|value| runtime_string_value(value.as_ref(), heap)),
    );
}

fn runtime_string_value_arg(value: &RuntimeVal, heap: &HeapStore) -> Option<Arc<str>> {
    match value {
        RuntimeVal::ShortStr(value) => Some(Arc::<str>::from(value.as_str())),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be an integer")),
    }
}

fn string_list_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Vec<String>> {
    let list = list_arg(value, heap, context)?;
    match list {
        TypedList::String(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(value.to_string());
            }
            Ok(out)
        }
        TypedList::Mixed(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(
                    runtime_string_arg(value, heap, context)
                        .map(|value| value.to_string())
                        .map_err(|_| anyhow!("join() list must contain only strings"))?,
                );
            }
            Ok(out)
        }
        _ => Err(anyhow!("join() list must contain only strings")),
    }
}
