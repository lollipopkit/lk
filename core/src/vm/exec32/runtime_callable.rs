use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{
        CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, TypedList, TypedMap, Val,
        runtime_val_to_val, val_to_runtime_val,
    },
    vm::{Module32, NativeArgs32, RuntimeCallable32, RuntimeModuleState32},
};

use super::{Exec32Result, Executor32, named_call::order_named_args32};

pub fn call_runtime_callable32(
    function: &RuntimeCallable32,
    args: &[Val],
    ctx: &mut crate::vm::VmContext,
) -> Result<Val> {
    let result = call_runtime_callable32_raw(function, args, ctx)?;
    let value = result.returns.first().unwrap_or(&RuntimeVal::Nil);
    runtime_val_to_val(value, &result.state.heap)
}

pub fn call_runtime_callable32_named(
    function: &RuntimeCallable32,
    pos: &[Val],
    named: &[(String, Val)],
    ctx: &mut crate::vm::VmContext,
) -> Result<Val> {
    let result = call_runtime_callable32_named_raw(function, pos, named, ctx)?;
    let value = result.returns.first().unwrap_or(&RuntimeVal::Nil);
    runtime_val_to_val(value, &result.state.heap)
}

pub fn call_runtime_callable32_named_raw(
    function: &RuntimeCallable32,
    pos: &[Val],
    named: &[(String, Val)],
    ctx: &mut crate::vm::VmContext,
) -> Result<Exec32Result> {
    let state = take_runtime_callable32_state(function)?;
    let function_meta = function
        .module
        .functions
        .get(function.function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function.function_index))?;
    let register_count = function_meta.register_count;
    let result = match Executor32::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        function.function_index,
        function.captures.as_ref().clone(),
        state,
        ctx,
        |executor| {
            let mut positional = Vec::with_capacity(pos.len());
            for arg in pos {
                positional.push(val_to_runtime_val(arg, executor.heap_mut())?);
            }
            let mut named_args = Vec::with_capacity(named.len());
            for (name, value) in named {
                named_args.push((name.clone(), val_to_runtime_val(value, executor.heap_mut())?));
            }
            let args = order_named_args32(function_meta, positional, named_args)?;
            let arg_count = checked_arg_count(args.len())?;
            for (index, value) in args.into_iter().enumerate() {
                executor.seed_param_arg(index, value)?;
            }
            Ok(arg_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            commit_runtime_callable32_state(function, &failure.state)?;
            return Err(failure.error);
        }
    };
    commit_runtime_callable32_state(function, &result.state)?;
    Ok(result)
}

pub fn call_runtime_callable32_raw(
    function: &RuntimeCallable32,
    args: &[Val],
    ctx: &mut crate::vm::VmContext,
) -> Result<Exec32Result> {
    let state = take_runtime_callable32_state(function)?;
    let arg_count = checked_arg_count(args.len())?;
    let register_count = function
        .module
        .functions
        .get(function.function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function.function_index))?
        .register_count;
    let result = match Executor32::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        function.function_index,
        function.captures.as_ref().clone(),
        state,
        ctx,
        |executor| {
            for (index, arg) in args.iter().enumerate() {
                let value = val_to_runtime_val(arg, executor.heap_mut())?;
                executor.seed_param_arg(index, value)?;
            }
            Ok(arg_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            commit_runtime_callable32_state(function, &failure.state)?;
            return Err(failure.error);
        }
    };
    commit_runtime_callable32_state(function, &result.state)?;
    Ok(result)
}

pub fn call_runtime_callable32_runtime_named(
    function: &RuntimeCallable32,
    pos: NativeArgs32<'_>,
    named: &[(String, RuntimeVal)],
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let state = take_runtime_callable32_state(function)?;
    let function_meta = function
        .module
        .functions
        .get(function.function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function.function_index))?;
    let register_count = function_meta.register_count;
    let mut local_ctx;
    let ctx = match ctx {
        Some(ctx) => ctx,
        None => {
            local_ctx = crate::vm::VmContext::new_without_core_vm_builtins();
            &mut local_ctx
        }
    };
    let mut result = match Executor32::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        function.function_index,
        function.captures.as_ref().clone(),
        state,
        ctx,
        |executor| {
            let mut positional = Vec::with_capacity(pos.len());
            for arg in pos.into_iter() {
                positional.push(copy_runtime_value(arg, caller_heap, executor.heap_mut())?);
            }
            let mut named_args = Vec::with_capacity(named.len());
            for (name, value) in named {
                named_args.push((
                    name.clone(),
                    copy_runtime_value(value, caller_heap, executor.heap_mut())?,
                ));
            }
            let args = order_named_args32(function_meta, positional, named_args)?;
            let arg_count = checked_arg_count(args.len())?;
            for (index, value) in args.into_iter().enumerate() {
                executor.seed_param_arg(index, value)?;
            }
            Ok(arg_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            commit_runtime_callable32_state(function, &failure.state)?;
            return Err(failure.error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &mut result.state.heap, caller_heap)?;
    commit_runtime_callable32_state(function, &result.state)?;
    Ok(value)
}

pub fn call_runtime_callable32_runtime(
    function: &RuntimeCallable32,
    args: NativeArgs32<'_>,
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let state = take_runtime_callable32_state(function)?;
    let arg_count = checked_arg_count(args.len())?;
    let register_count = function
        .module
        .functions
        .get(function.function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function.function_index))?
        .register_count;
    let mut local_ctx;
    let ctx = match ctx {
        Some(ctx) => ctx,
        None => {
            local_ctx = crate::vm::VmContext::new_without_core_vm_builtins();
            &mut local_ctx
        }
    };
    let mut result = match Executor32::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        function.function_index,
        function.captures.as_ref().clone(),
        state,
        ctx,
        |executor| {
            for (index, arg) in args.into_iter().enumerate() {
                let value = copy_runtime_value(arg, caller_heap, executor.heap_mut())?;
                executor.seed_param_arg(index, value)?;
            }
            Ok(arg_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            commit_runtime_callable32_state(function, &failure.state)?;
            return Err(failure.error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &mut result.state.heap, caller_heap)?;
    commit_runtime_callable32_state(function, &result.state)?;
    Ok(value)
}

fn commit_runtime_callable32_state(function: &RuntimeCallable32, next_state: &RuntimeModuleState32) -> Result<()> {
    let mut state = function
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeCallable32 state lock poisoned"))?;
    *state = next_state.clone();
    Ok(())
}

fn take_runtime_callable32_state(function: &RuntimeCallable32) -> Result<RuntimeModuleState32> {
    let mut state = function
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeCallable32 state lock poisoned"))?;
    Ok(std::mem::take(&mut *state))
}

fn checked_arg_count(len: usize) -> Result<u16> {
    u16::try_from(len).map_err(|_| anyhow!("function arg count {} exceeds u16", len))
}

pub fn runtime_value_to_callable32(
    value: &RuntimeVal,
    heap: &HeapStore,
    globals: &[RuntimeVal],
    module: Arc<Module32>,
) -> Option<RuntimeCallable32> {
    if let RuntimeVal::Obj(handle) = value
        && let Some(value) = heap.get(*handle)
        && let HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) = value
    {
        return Some(RuntimeCallable32::new(
            module,
            *function_index,
            captures.clone(),
            heap.clone(),
            globals.to_vec(),
        ));
    }
    None
}

pub fn copy_runtime_value(
    value: &RuntimeVal,
    source_heap: &mut HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<RuntimeVal> {
    match value {
        RuntimeVal::Nil => Ok(RuntimeVal::Nil),
        RuntimeVal::Bool(value) => Ok(RuntimeVal::Bool(*value)),
        RuntimeVal::Int(value) => Ok(RuntimeVal::Int(*value)),
        RuntimeVal::Float(value) => Ok(RuntimeVal::Float(*value)),
        RuntimeVal::ShortStr(value) => Ok(RuntimeVal::ShortStr(*value)),
        RuntimeVal::Obj(handle) => {
            let value = source_heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                .clone();
            copy_heap_value(value, source_heap, dest_heap).map(|value| RuntimeVal::Obj(dest_heap.alloc(value)))
        }
    }
}

fn copy_heap_value(value: HeapValue, source_heap: &mut HeapStore, dest_heap: &mut HeapStore) -> Result<HeapValue> {
    Ok(match value {
        HeapValue::String(value) => HeapValue::String(value),
        HeapValue::List(values) => HeapValue::List(copy_typed_list(values, source_heap, dest_heap)?),
        HeapValue::Map(values) => HeapValue::Map(copy_typed_map(values, source_heap, dest_heap)?),
        HeapValue::Object(object) => {
            let fields = object
                .fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), copy_runtime_value(value, source_heap, dest_heap)?)))
                .collect::<Result<_>>()?;
            HeapValue::Object(RuntimeObject {
                type_name: object.type_name,
                fields,
            })
        }
        HeapValue::Callable(CallableValue::ParsedClosure(value)) => {
            HeapValue::Callable(CallableValue::ParsedClosure(value))
        }
        HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) => {
            HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function })
        }
        HeapValue::Callable(CallableValue::Native { function_index, arity }) => {
            HeapValue::Callable(CallableValue::Native { function_index, arity })
        }
        HeapValue::Callable(CallableValue::Runtime32(function)) => {
            HeapValue::Callable(CallableValue::Runtime32(function))
        }
        HeapValue::Callable(CallableValue::Aot(value)) => HeapValue::Callable(CallableValue::Aot(value)),
        HeapValue::Callable(CallableValue::AotHandle { handle, arity }) => {
            HeapValue::Callable(CallableValue::AotHandle { handle, arity })
        }
        HeapValue::Callable(CallableValue::Closure { .. }) => {
            bail!("cannot copy raw closure without module context")
        }
        HeapValue::Task(value) => HeapValue::Task(value),
        HeapValue::Channel(value) => HeapValue::Channel(value),
        HeapValue::Stream(value) => HeapValue::Stream(value),
        HeapValue::StreamCursor(value) => HeapValue::StreamCursor(value),
    })
}

fn copy_typed_list(values: TypedList, source_heap: &mut HeapStore, dest_heap: &mut HeapStore) -> Result<TypedList> {
    Ok(match values {
        TypedList::Mixed(values) => TypedList::Mixed(
            values
                .iter()
                .map(|value| copy_runtime_value(value, source_heap, dest_heap))
                .collect::<Result<_>>()?,
        ),
        TypedList::Int(values) => TypedList::Int(values),
        TypedList::Float(values) => TypedList::Float(values),
        TypedList::Bool(values) => TypedList::Bool(values),
        TypedList::String(values) => TypedList::String(values),
    })
}

fn copy_typed_map(values: TypedMap, source_heap: &mut HeapStore, dest_heap: &mut HeapStore) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => TypedMap::Mixed(copy_runtime_entries(values, source_heap, dest_heap)?),
        TypedMap::StringMixed(values) => TypedMap::StringMixed(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), copy_runtime_value(value, source_heap, dest_heap)?)))
                .collect::<Result<_>>()?,
        ),
        TypedMap::StringInt(values) => TypedMap::StringInt(values),
        TypedMap::StringFloat(values) => TypedMap::StringFloat(values),
        TypedMap::StringBool(values) => TypedMap::StringBool(values),
    })
}

fn copy_runtime_entries(
    values: std::collections::BTreeMap<RuntimeMapKey, RuntimeVal>,
    source_heap: &mut HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<std::collections::BTreeMap<RuntimeMapKey, RuntimeVal>> {
    values
        .iter()
        .map(|(key, value)| Ok((key.clone(), copy_runtime_value(value, source_heap, dest_heap)?)))
        .collect()
}
