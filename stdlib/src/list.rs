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
        let list = list_arg(&values[0], runtime.heap(), "push() first argument")?;
        let typed = list_push_preserving_backing(list, values[1].clone(), runtime.heap_mut());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(typed))))
    }

    fn concat32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "concat()")?;
        let values = args.as_slice();
        let left = list_arg(&values[0], runtime.heap(), "concat() first argument")?;
        let right = list_arg(&values[1], runtime.heap(), "concat() second argument")?;
        let typed = list_concat_preserving_backing(left, right, runtime.heap_mut());
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
        Ok(list_get(&list, index as usize, runtime.heap_mut()).unwrap_or(RuntimeVal::Nil))
    }

    fn first32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let list = one_list(args, runtime, "first()")?;
        Ok(list_get(&list, 0, runtime.heap_mut()).unwrap_or(RuntimeVal::Nil))
    }

    fn last32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let list = one_list(args, runtime, "last()")?;
        let Some(index) = list.len().checked_sub(1) else {
            return Ok(RuntimeVal::Nil);
        };
        Ok(list_get(&list, index, runtime.heap_mut()).unwrap_or(RuntimeVal::Nil))
    }

    fn set32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 3, "set()")?;
        let values = args.as_slice();
        let list = list_arg(&values[0], runtime.heap(), "set() first argument")?;
        let index = int_arg(&values[1], "set() index")?;
        if index < 0 {
            bail!("set() index must be non-negative");
        }
        let (updated_list, old) =
            list_set_preserving_backing(list, index as usize, values[2].clone(), runtime.heap_mut())?;
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

fn one_list(args: NativeArgs32<'_>, runtime: &NativeRuntime32<'_>, name: &str) -> Result<TypedList> {
    expect_arity(args, 1, name)?;
    list_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn list_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<TypedList> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} argument must be a list");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(list.clone()),
        other => Err(anyhow!("{context} argument must be a list, got {}", other.type_name())),
    }
}

fn list_get(list: &TypedList, index: usize, heap: &mut HeapStore) -> Option<RuntimeVal> {
    match list {
        TypedList::Mixed(values) => values.get(index).cloned(),
        TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int),
        TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float),
        TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool),
        TypedList::String(values) => values
            .get(index)
            .map(|value| runtime_string_value(value.as_ref(), heap)),
    }
}

fn list_push_preserving_backing(list: TypedList, value: RuntimeVal, heap: &mut HeapStore) -> TypedList {
    match list {
        TypedList::Mixed(mut values) => {
            values.push(value);
            TypedList::Mixed(values)
        }
        TypedList::Int(mut values) => match value {
            RuntimeVal::Int(value) => {
                values.push(value);
                TypedList::Int(values)
            }
            value => {
                let mut mixed = values.into_iter().map(RuntimeVal::Int).collect::<Vec<_>>();
                mixed.push(value);
                TypedList::Mixed(mixed)
            }
        },
        TypedList::Float(mut values) => match value {
            RuntimeVal::Float(value) => {
                values.push(value);
                TypedList::Float(values)
            }
            value => {
                let mut mixed = values.into_iter().map(RuntimeVal::Float).collect::<Vec<_>>();
                mixed.push(value);
                TypedList::Mixed(mixed)
            }
        },
        TypedList::Bool(mut values) => match value {
            RuntimeVal::Bool(value) => {
                values.push(value);
                TypedList::Bool(values)
            }
            value => {
                let mut mixed = values.into_iter().map(RuntimeVal::Bool).collect::<Vec<_>>();
                mixed.push(value);
                TypedList::Mixed(mixed)
            }
        },
        TypedList::String(mut values) => match runtime_string_value_arg(&value, heap) {
            Some(value) => {
                values.push(value);
                TypedList::String(values)
            }
            None => {
                let mut mixed = materialize_string_values(values, heap);
                mixed.push(value);
                TypedList::Mixed(mixed)
            }
        },
    }
}

fn list_concat_preserving_backing(left: TypedList, right: TypedList, heap: &mut HeapStore) -> TypedList {
    match (left, right) {
        (TypedList::Int(mut left), TypedList::Int(right)) => {
            left.extend(right);
            TypedList::Int(left)
        }
        (TypedList::Float(mut left), TypedList::Float(right)) => {
            left.extend(right);
            TypedList::Float(left)
        }
        (TypedList::Bool(mut left), TypedList::Bool(right)) => {
            left.extend(right);
            TypedList::Bool(left)
        }
        (TypedList::String(mut left), TypedList::String(right)) => {
            left.extend(right);
            TypedList::String(left)
        }
        (left, right) => {
            let mut mixed = list_to_runtime_values(left, heap);
            mixed.extend(list_to_runtime_values(right, heap));
            TypedList::Mixed(mixed)
        }
    }
}

fn list_set_preserving_backing(
    list: TypedList,
    index: usize,
    value: RuntimeVal,
    heap: &mut HeapStore,
) -> Result<(TypedList, RuntimeVal)> {
    match list {
        TypedList::Mixed(mut values) => {
            let Some(slot) = values.get_mut(index) else {
                bail!("list index {} out of bounds", index);
            };
            let old = std::mem::replace(slot, value);
            Ok((TypedList::Mixed(values), old))
        }
        TypedList::Int(mut values) => match value {
            RuntimeVal::Int(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                let old = RuntimeVal::Int(std::mem::replace(slot, value));
                Ok((TypedList::Int(values), old))
            }
            value => set_materialized_list(values.into_iter().map(RuntimeVal::Int).collect(), index, value),
        },
        TypedList::Float(mut values) => match value {
            RuntimeVal::Float(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                let old = RuntimeVal::Float(std::mem::replace(slot, value));
                Ok((TypedList::Float(values), old))
            }
            value => set_materialized_list(values.into_iter().map(RuntimeVal::Float).collect(), index, value),
        },
        TypedList::Bool(mut values) => match value {
            RuntimeVal::Bool(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                let old = RuntimeVal::Bool(std::mem::replace(slot, value));
                Ok((TypedList::Bool(values), old))
            }
            value => set_materialized_list(values.into_iter().map(RuntimeVal::Bool).collect(), index, value),
        },
        TypedList::String(mut values) => match runtime_string_value_arg(&value, heap) {
            Some(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                let old = runtime_string_value(slot.as_ref(), heap);
                *slot = value;
                Ok((TypedList::String(values), old))
            }
            None => set_materialized_list(materialize_string_values(values, heap), index, value),
        },
    }
}

fn set_materialized_list(
    mut values: Vec<RuntimeVal>,
    index: usize,
    value: RuntimeVal,
) -> Result<(TypedList, RuntimeVal)> {
    let Some(slot) = values.get_mut(index) else {
        bail!("list index {} out of bounds", index);
    };
    let old = std::mem::replace(slot, value);
    Ok((TypedList::Mixed(values), old))
}

fn list_to_runtime_values(list: TypedList, heap: &mut HeapStore) -> Vec<RuntimeVal> {
    match list {
        TypedList::Mixed(values) => values,
        TypedList::Int(values) => values.into_iter().map(RuntimeVal::Int).collect(),
        TypedList::Float(values) => values.into_iter().map(RuntimeVal::Float).collect(),
        TypedList::Bool(values) => values.into_iter().map(RuntimeVal::Bool).collect(),
        TypedList::String(values) => materialize_string_values(values, heap),
    }
}

fn materialize_string_values(values: Vec<Arc<str>>, heap: &mut HeapStore) -> Vec<RuntimeVal> {
    values
        .into_iter()
        .map(|value| runtime_string_value(value.as_ref(), heap))
        .collect()
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
        TypedList::String(values) => Ok(values.iter().map(ToString::to_string).collect()),
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| {
                runtime_string_arg(value, heap, context)
                    .map(|value| value.to_string())
                    .map_err(|_| anyhow!("join() list must contain only strings"))
            })
            .collect(),
        _ => Err(anyhow!("join() list must contain only strings")),
    }
}
