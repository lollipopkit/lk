use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry},
    val::{HeapStore, HeapValue, RuntimeVal, TypedList, Val},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32},
};

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct ListModule {
    functions: HashMap<String, Val>,
}

impl Default for ListModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ListModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        register_native(&mut functions, "len", Self::len32, 1);
        register_native(&mut functions, "push", Self::push32, 2);
        register_native(&mut functions, "concat", Self::concat32, 2);
        register_native(&mut functions, "join", Self::join32, 2);
        register_native(&mut functions, "get", Self::get32, 2);
        register_native(&mut functions, "first", Self::first32, 1);
        register_native(&mut functions, "last", Self::last32, 1);
        register_native(&mut functions, "set", Self::set32, 3);

        Self { functions }
    }

    fn len32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let list = one_list(args, runtime, "len()")?;
        Ok(RuntimeVal::Int(list.len() as i64))
    }

    fn push32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "push()")?;
        let values = args.as_slice();
        let list = list_arg(&values[0], &runtime.state.heap, "push() first argument")?;
        let mut items = list.materialize_mixed(runtime.heap_mut());
        items.push(values[1].clone());
        let typed = TypedList::from_runtime_values(items, &runtime.state.heap);
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(typed))))
    }

    fn concat32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "concat()")?;
        let values = args.as_slice();
        let left = list_arg(&values[0], &runtime.state.heap, "concat() first argument")?;
        let right = list_arg(&values[1], &runtime.state.heap, "concat() second argument")?;
        let mut items = left.materialize_mixed(runtime.heap_mut());
        items.extend(right.materialize_mixed(runtime.heap_mut()));
        let typed = TypedList::from_runtime_values(items, &runtime.state.heap);
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(typed))))
    }

    fn join32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "join()")?;
        let values = args.as_slice();
        let strings = string_list_arg(&values[0], &runtime.state.heap, "join() first argument")?;
        let delimiter = runtime_string_arg(&values[1], &runtime.state.heap, "join() second argument")?;
        Ok(runtime_string_value(
            &strings.join(delimiter.as_ref()),
            runtime.heap_mut(),
        ))
    }

    fn get32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "get()")?;
        let values = args.as_slice();
        let list = list_arg(&values[0], &runtime.state.heap, "get() first argument")?;
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
        let list = list_arg(&values[0], &runtime.state.heap, "set() first argument")?;
        let index = int_arg(&values[1], "set() index")?;
        if index < 0 {
            bail!("set() index must be non-negative");
        }
        let mut items = list.materialize_mixed(runtime.heap_mut());
        let Some(slot) = items.get_mut(index as usize) else {
            bail!("list index {} out of bounds", index);
        };
        let old = std::mem::replace(slot, values[2].clone());
        let updated_list = TypedList::from_runtime_values(items, &runtime.state.heap);
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

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn register_native(
    functions: &mut HashMap<String, Val>,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    functions.insert(
        name.to_string(),
        Val::runtime_native32(NativeFunction32::Plain(function), arity),
    );
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
    list_arg(&args.as_slice()[0], &runtime.state.heap, name)
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
