#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::compat::sync::Mutex;
use crate::util::fast_map::{FastHashMap, fast_hash_map_new, fast_hash_set_new};
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{
        CallableValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeSet, RuntimeVal, TypedList,
        TypedMap,
    },
    vm::{Module, NativeArgs, NativeEntry, RuntimeCallable, RuntimeModuleState, VmContext},
};

use super::{
    ExecFailure, Executor,
    call::{CallableTarget, callable_target},
    named_call::call_named_arg_name,
    support::{InlineNativeArgs, call_native_entry, call_native_entry_parts_with_args, call_native_entry_with_args},
};

mod positional;
use self::positional::*;

#[cfg(test)]
pub(crate) fn call_runtime_callable_test(
    function: &RuntimeCallable,
    args: &[RuntimeVal],
    ctx: &mut crate::vm::VmContext,
) -> Result<Vec<RuntimeVal>> {
    let state = take_runtime_callable_state(function)?;
    let arg_count = checked_arg_count(args.len())?;
    let register_count = function
        .module
        .functions
        .get(function.function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function.function_index))?
        .register_count;
    let result = match Executor::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        Some(Arc::clone(&function.module)),
        function.function_index,
        Arc::clone(&function.captures),
        state,
        ctx,
        |executor| {
            for (index, arg) in args.iter().cloned().enumerate() {
                executor.seed_param_arg(index, arg)?;
            }
            Ok(arg_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let ExecFailure { error, state } = failure;
            commit_runtime_callable_state(function, state)?;
            return Err(error);
        }
    };
    let super::ExecResult { returns, state } = result;
    commit_runtime_callable_state(function, state)?;
    Ok(returns)
}

pub fn call_runtime_callable_runtime_named_stack(
    function: &RuntimeCallable,
    positional: &[RuntimeVal],
    caller_stack: &[RuntimeVal],
    named_start: usize,
    named_count: u16,
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let state = take_runtime_callable_state(function)?;
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
    let result = match Executor::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        Some(Arc::clone(&function.module)),
        function.function_index,
        Arc::clone(&function.captures),
        state,
        ctx,
        |executor| {
            let heap = &mut executor.state.heap;
            let frame = &mut executor.state.stack[..function_meta.register_count as usize];
            copy_named_stack_args_to_frame(
                function_meta,
                positional,
                caller_stack,
                named_start,
                named_count,
                caller_heap,
                heap,
                frame,
            )?;
            Ok(function_meta.param_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let ExecFailure { error, state } = failure;
            commit_runtime_callable_state(function, state)?;
            return Err(error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &result.state.heap, caller_heap)?;
    commit_runtime_callable_state(function, result.state)?;
    Ok(value)
}

pub fn call_runtime_callable_runtime(
    function: &RuntimeCallable,
    args: &[RuntimeVal],
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_callable_runtime_positional(function, RuntimePositionalArgs::Slice(args), caller_heap, ctx)
}

pub fn call_runtime_value_runtime(
    callee: RuntimeVal,
    args: &[RuntimeVal],
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_value_with_map_args(callee, RuntimePositionalArgs::Slice(args), None, state, module, ctx)
}

pub fn call_runtime_value_runtime_with_receiver(
    callee: RuntimeVal,
    receiver: &RuntimeVal,
    args: &[RuntimeVal],
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_value_with_map_args(
        callee,
        RuntimePositionalArgs::Prefixed {
            first: receiver,
            rest: args,
        },
        None,
        state,
        module,
        ctx,
    )
}

pub fn call_runtime_value_runtime_with_receiver_list_args(
    callee: RuntimeVal,
    receiver: &RuntimeVal,
    args: Option<HeapRef>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let pos = match args {
        Some(handle) => RuntimePositionalArgs::PrefixedList {
            first: receiver,
            rest: handle,
        },
        None => RuntimePositionalArgs::Prefixed {
            first: receiver,
            rest: &[],
        },
    };
    call_runtime_value_with_map_args(callee, pos, None, state, module, ctx)
}

pub fn call_runtime_value_runtime_list_args(
    callee: RuntimeVal,
    args: Option<HeapRef>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let pos = args.map_or(RuntimePositionalArgs::Slice(&[]), RuntimePositionalArgs::ListHandle);
    call_runtime_value_with_map_args(callee, pos, None, state, module, ctx)
}

pub fn call_runtime_value_runtime_named_map(
    callee: RuntimeVal,
    pos: &[RuntimeVal],
    named: Option<crate::val::HeapRef>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_value_with_map_args(callee, RuntimePositionalArgs::Slice(pos), named, state, module, ctx)
}

pub fn call_runtime_value_runtime_named_map_list_args(
    callee: RuntimeVal,
    pos: Option<HeapRef>,
    named: Option<crate::val::HeapRef>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let pos = pos.map_or(RuntimePositionalArgs::Slice(&[]), RuntimePositionalArgs::ListHandle);
    call_runtime_value_with_map_args(callee, pos, named, state, module, ctx)
}

fn call_runtime_value_with_map_args(
    callee: RuntimeVal,
    pos: RuntimePositionalArgs<'_>,
    named: Option<crate::val::HeapRef>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let callee_root = callee;
    let RuntimeVal::Obj(handle) = callee else {
        bail!("runtime callee is not callable");
    };
    let callable = callable_target(
        None,
        state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
        "runtime callee is not callable",
    )?;
    let Some(named_handle) = named else {
        return match callable {
            CallableTarget::Closure {
                function_index,
                captures,
            } => call_closure_value(function_index, captures, pos, state, module, ctx),
            CallableTarget::RuntimeNative { arity, function } => {
                let pos_len = pos.len(&state.heap)?;
                if arity != NativeEntry::VARIADIC && arity != pos_len as u16 {
                    bail!("Native expects {} positional arguments, got {}", arity, pos_len);
                }
                let native = NativeEntry {
                    name: "<runtime-native>".to_string(),
                    arity,
                    function,
                };
                call_runtime_native_positional(&native, pos, state, module, ctx, callee_root)
            }
            CallableTarget::Runtime(function) => {
                call_runtime_callable_runtime_positional(function.as_ref(), pos, &mut state.heap, ctx)
            }
        };
    };
    match callable {
        CallableTarget::Closure {
            function_index,
            captures,
        } => call_closure_value_typed_map(function_index, captures, pos, named_handle, state, module, ctx),
        CallableTarget::RuntimeNative { arity, function } => {
            let named_count = match state
                .heap
                .get(named_handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", named_handle.index()))?
            {
                HeapValue::Map(map) => map.len(),
                _ => bail!("named arguments must be a map"),
            };
            let pos_len = pos.len(&state.heap)?;
            if arity != NativeEntry::VARIADIC && arity != pos_len as u16 {
                bail!("Native expects {} positional arguments, got {}", arity, pos_len);
            }
            let native = NativeEntry {
                name: "<runtime-native>".to_string(),
                arity,
                function,
            };
            call_runtime_native_named_map(&native, pos, named_handle, named_count, state, module, ctx, callee_root)
        }
        CallableTarget::Runtime(function) => call_runtime_callable_runtime_named_map_positional(
            function.as_ref(),
            pos,
            named_handle,
            &mut state.heap,
            ctx,
        ),
    }
}

fn typed_list_arg_len(handle: HeapRef, heap: &HeapStore) -> Result<usize> {
    match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(list) => Ok(list.len()),
        other => bail!("runtime positional arguments must be a list, got {}", other.type_name()),
    }
}

fn typed_list_arg_value(handle: HeapRef, heap: &mut HeapStore, index: usize) -> Result<RuntimeVal> {
    let long_string = match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => {
            return values
                .get(index)
                .cloned()
                .ok_or_else(|| anyhow!("runtime list argument index {index} out of bounds"));
        }
        HeapValue::List(TypedList::Int(values)) => {
            return values
                .get(index)
                .copied()
                .map(RuntimeVal::Int)
                .ok_or_else(|| anyhow!("runtime list argument index {index} out of bounds"));
        }
        HeapValue::List(TypedList::Float(values)) => {
            return values
                .get(index)
                .copied()
                .map(RuntimeVal::Float)
                .ok_or_else(|| anyhow!("runtime list argument index {index} out of bounds"));
        }
        HeapValue::List(TypedList::Bool(values)) => {
            return values
                .get(index)
                .copied()
                .map(RuntimeVal::Bool)
                .ok_or_else(|| anyhow!("runtime list argument index {index} out of bounds"));
        }
        HeapValue::List(TypedList::String(values)) => {
            let value = values
                .get(index)
                .cloned()
                .ok_or_else(|| anyhow!("runtime list argument index {index} out of bounds"))?;
            if let Some(short) = crate::val::ShortStr::new(value.as_ref()) {
                return Ok(RuntimeVal::ShortStr(short));
            }
            value
        }
        other => bail!("runtime positional arguments must be a list, got {}", other.type_name()),
    };
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::String(long_string))))
}

fn copy_list_handle_into_slots(handle: HeapRef, heap: &mut HeapStore, frame: &mut [RuntimeVal]) -> Result<()> {
    let long_string_values = match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => {
            for (slot, value) in frame.iter_mut().zip(values) {
                *slot = *value;
            }
            return Ok(());
        }
        HeapValue::List(TypedList::Int(values)) => {
            for (slot, &value) in frame.iter_mut().zip(values) {
                *slot = RuntimeVal::Int(value);
            }
            return Ok(());
        }
        HeapValue::List(TypedList::Float(values)) => {
            for (slot, &value) in frame.iter_mut().zip(values) {
                *slot = RuntimeVal::Float(value);
            }
            return Ok(());
        }
        HeapValue::List(TypedList::Bool(values)) => {
            for (slot, &value) in frame.iter_mut().zip(values) {
                *slot = RuntimeVal::Bool(value);
            }
            return Ok(());
        }
        HeapValue::List(TypedList::String(values)) => {
            let mut long_values = Vec::new();
            for (index, value) in values.iter().enumerate() {
                match crate::val::ShortStr::new(value.as_ref()) {
                    Some(short) => frame[index] = RuntimeVal::ShortStr(short),
                    None => long_values.push((index, Arc::clone(value))),
                }
            }
            long_values
        }
        other => bail!("runtime positional arguments must be a list, got {}", other.type_name()),
    };
    for (index, value) in long_string_values {
        frame[index] = RuntimeVal::Obj(heap.alloc(HeapValue::String(value)));
    }
    Ok(())
}

fn call_closure_value(
    function_index: u32,
    captures: Arc<Vec<RuntimeVal>>,
    pos: RuntimePositionalArgs<'_>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let module = module.ok_or_else(|| anyhow!("closure callable requires Module context"))?;
    let function = module
        .functions
        .get(function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
    let mut ctx = ctx;
    let mut callee = Executor::new(function.register_count);
    callee.state = core::mem::take(state);
    callee.captures = captures;
    let saved_top = callee.state.stack_top;
    let result = (|| {
        let new_base = saved_top;
        let new_top = new_base + function.register_count as usize;
        if callee.state.stack.len() < new_top {
            callee.state.stack.resize(new_top, RuntimeVal::Nil);
        }
        let frame = &mut callee.state.stack[new_base..new_top];
        frame.fill(RuntimeVal::Nil);
        if function.param_count != pos.len(&callee.state.heap)? as u16 {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                pos.len(&callee.state.heap)?
            );
        }
        pos.copy_into_frame(&mut callee.state.heap, frame)?;
        callee.frame_base = new_base;
        callee.register_count = function.register_count;
        callee.state.stack_top = new_top;
        callee.pc = 0;
        callee.run_function_inner(function, function_index, Some(module), &mut ctx)
    })();
    callee.state.stack_top = saved_top;
    *state = callee.state;
    match result {
        Ok(returns) => Ok(returns.into_first()),
        Err(error) => Err(error),
    }
}

fn call_closure_value_typed_map(
    function_index: u32,
    captures: Arc<Vec<RuntimeVal>>,
    pos: RuntimePositionalArgs<'_>,
    named: crate::val::HeapRef,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let module = module.ok_or_else(|| anyhow!("closure callable requires Module context"))?;
    let function = module
        .functions
        .get(function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
    let mut ctx = ctx;
    let mut callee = Executor::new(function.register_count);
    callee.state = core::mem::take(state);
    callee.captures = captures;
    let saved_top = callee.state.stack_top;
    let result = (|| {
        let new_base = saved_top;
        let new_top = new_base + function.register_count as usize;
        if callee.state.stack.len() < new_top {
            callee.state.stack.resize(new_top, RuntimeVal::Nil);
        }
        let frame = &mut callee.state.stack[new_base..new_top];
        frame.fill(RuntimeVal::Nil);
        let positional_count = function.positional_param_count as usize;
        let pos_len = pos.len(&callee.state.heap)?;
        if pos_len != positional_count {
            bail!(
                "Function expects {} positional arguments before named arguments, got {}",
                positional_count,
                pos_len
            );
        }
        pos.copy_into_frame(&mut callee.state.heap, &mut frame[..positional_count])?;
        let heap_value = callee
            .state
            .heap
            .get(named)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", named.index()))?;
        let HeapValue::Map(named) = heap_value else {
            bail!("named arguments must be a map");
        };
        write_named_args_to_frame_from_typed_map(function, named, frame)?;
        callee.frame_base = new_base;
        callee.register_count = function.register_count;
        callee.state.stack_top = new_top;
        callee.pc = 0;
        callee.run_function_inner(function, function_index, Some(module), &mut ctx)
    })();
    callee.state.stack_top = saved_top;
    *state = callee.state;
    match result {
        Ok(returns) => Ok(returns.into_first()),
        Err(error) => Err(error),
    }
}

fn call_runtime_callable_runtime_positional(
    function: &RuntimeCallable,
    pos: RuntimePositionalArgs<'_>,
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let state = take_runtime_callable_state(function)?;
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
    let result = match Executor::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        Some(Arc::clone(&function.module)),
        function.function_index,
        Arc::clone(&function.captures),
        state,
        ctx,
        |executor| {
            let heap = &mut executor.state.heap;
            let frame = &mut executor.state.stack[..function_meta.register_count as usize];
            copy_runtime_positional_args_to_frame(function_meta, pos, caller_heap, heap, frame)?;
            Ok(function_meta.param_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let ExecFailure { error, state } = failure;
            commit_runtime_callable_state(function, state)?;
            return Err(error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &result.state.heap, caller_heap)?;
    commit_runtime_callable_state(function, result.state)?;
    Ok(value)
}

fn call_runtime_callable_runtime_named_map_positional(
    function: &RuntimeCallable,
    pos: RuntimePositionalArgs<'_>,
    named: crate::val::HeapRef,
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let state = take_runtime_callable_state(function)?;
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
    let result = match Executor::new(register_count).run_module_function_with_state_recoverable(
        function.module.as_ref(),
        Some(Arc::clone(&function.module)),
        function.function_index,
        Arc::clone(&function.captures),
        state,
        ctx,
        |executor| {
            let named = match caller_heap
                .get(named)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", named.index()))?
            {
                HeapValue::Map(map) => map,
                _ => bail!("named arguments must be a map"),
            };
            let heap = &mut executor.state.heap;
            let frame = &mut executor.state.stack[..function_meta.register_count as usize];
            copy_runtime_positional_args_with_named_map_to_frame(function_meta, pos, named, caller_heap, heap, frame)?;
            Ok(function_meta.param_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let ExecFailure { error, state } = failure;
            commit_runtime_callable_state(function, state)?;
            return Err(error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &result.state.heap, caller_heap)?;
    commit_runtime_callable_state(function, result.state)?;
    Ok(value)
}

fn commit_runtime_callable_state(function: &RuntimeCallable, next_state: RuntimeModuleState) -> Result<()> {
    let mut state = function
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeCallable state lock poisoned"))?;
    *state = next_state;
    Ok(())
}

fn take_runtime_callable_state(function: &RuntimeCallable) -> Result<RuntimeModuleState> {
    let mut state = function
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeCallable state lock poisoned"))?;
    Ok(core::mem::take(&mut *state))
}

#[cfg(test)]
fn checked_arg_count(len: usize) -> Result<u16> {
    u16::try_from(len).map_err(|_| anyhow!("function arg count {} exceeds u16", len))
}

fn copy_runtime_positional_args_to_frame(
    function: &crate::vm::Function,
    pos: RuntimePositionalArgs<'_>,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    frame: &mut [RuntimeVal],
) -> Result<()> {
    if frame.len() < function.param_count as usize {
        bail!(
            "callee frame has {} slots, function requires {} params",
            frame.len(),
            function.param_count
        );
    }
    let expected = function.param_count as usize;
    let actual = pos.len(caller_heap)?;
    if actual != expected {
        bail!("Function expects {} positional arguments, got {}", expected, actual);
    }
    copy_runtime_positional_args_into_slots(pos, caller_heap, callee_heap, &mut frame[..expected])
}

fn copy_runtime_positional_args_with_named_map_to_frame(
    function: &crate::vm::Function,
    pos: RuntimePositionalArgs<'_>,
    named: &TypedMap,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    frame: &mut [RuntimeVal],
) -> Result<()> {
    if frame.len() < function.param_count as usize {
        bail!(
            "callee frame has {} slots, function requires {} params",
            frame.len(),
            function.param_count
        );
    }
    if function.param_names.len() != function.param_count as usize {
        bail!("Function does not expose named parameter metadata");
    }
    let positional_count = function.positional_param_count as usize;
    let actual = pos.len(caller_heap)?;
    if actual != positional_count {
        bail!(
            "Function expects {} positional arguments before named arguments, got {}",
            positional_count,
            actual
        );
    }
    copy_runtime_positional_args_into_slots(pos, caller_heap, callee_heap, &mut frame[..positional_count])?;
    copy_typed_map_named_args_to_frame(function, named, caller_heap, callee_heap, frame)
}

fn copy_runtime_positional_args_into_slots(
    pos: RuntimePositionalArgs<'_>,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    slots: &mut [RuntimeVal],
) -> Result<()> {
    match pos {
        RuntimePositionalArgs::Slice(values) => {
            for (slot, value) in slots.iter_mut().zip(values) {
                *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
            }
            Ok(())
        }
        RuntimePositionalArgs::ListHandle(handle) => {
            copy_typed_list_arg_handle_to_slots(handle, caller_heap, callee_heap, slots)
        }
        RuntimePositionalArgs::Prefixed { first, rest } => {
            let Some((first_slot, rest_slots)) = slots.split_first_mut() else {
                bail!("runtime positional argument frame is empty");
            };
            *first_slot = copy_runtime_value(first, caller_heap, callee_heap)?;
            for (slot, value) in rest_slots.iter_mut().zip(rest) {
                *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
            }
            Ok(())
        }
        RuntimePositionalArgs::PrefixedList { first, rest } => {
            let Some((first_slot, rest_slots)) = slots.split_first_mut() else {
                bail!("runtime positional argument frame is empty");
            };
            *first_slot = copy_runtime_value(first, caller_heap, callee_heap)?;
            copy_typed_list_arg_handle_to_slots(rest, caller_heap, callee_heap, rest_slots)
        }
    }
}

fn copy_typed_list_arg_handle_to_slots(
    handle: HeapRef,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    slots: &mut [RuntimeVal],
) -> Result<()> {
    match caller_heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => {
            for (slot, value) in slots.iter_mut().zip(values) {
                *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
            }
        }
        HeapValue::List(TypedList::Int(values)) => {
            for (slot, &value) in slots.iter_mut().zip(values) {
                *slot = RuntimeVal::Int(value);
            }
        }
        HeapValue::List(TypedList::Float(values)) => {
            for (slot, &value) in slots.iter_mut().zip(values) {
                *slot = RuntimeVal::Float(value);
            }
        }
        HeapValue::List(TypedList::Bool(values)) => {
            for (slot, &value) in slots.iter_mut().zip(values) {
                *slot = RuntimeVal::Bool(value);
            }
        }
        HeapValue::List(TypedList::String(values)) => {
            for (slot, value) in slots.iter_mut().zip(values) {
                *slot = match crate::val::ShortStr::new(value.as_ref()) {
                    Some(short) => RuntimeVal::ShortStr(short),
                    None => RuntimeVal::Obj(callee_heap.alloc(HeapValue::String(Arc::clone(value)))),
                };
            }
        }
        other => bail!("runtime positional arguments must be a list, got {}", other.type_name()),
    }
    Ok(())
}

fn copy_typed_map_named_args_to_frame(
    function: &crate::vm::Function,
    named: &TypedMap,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    frame: &mut [RuntimeVal],
) -> Result<()> {
    let positional_count = function.positional_param_count as usize;
    let mut seen = vec![false; function.param_count as usize - positional_count];

    macro_rules! place_named {
        ($name:expr, $value:expr) => {{
            let name_str: &str = ($name).as_ref();
            let Some(offset) = function.param_names[positional_count..]
                .iter()
                .position(|param| param.as_ref() == name_str)
            else {
                bail!("unknown named argument `{name_str}`");
            };
            if core::mem::replace(&mut seen[offset], true) {
                bail!("duplicate named argument `{name_str}`");
            }
            frame[positional_count + offset] = $value;
        }};
    }

    match named {
        TypedMap::StringMixed(values) => {
            for (name, value) in values {
                place_named!(name, copy_runtime_value(value, caller_heap, callee_heap)?);
            }
        }
        TypedMap::StringInt(values) => {
            for (name, &value) in values {
                place_named!(name, RuntimeVal::Int(value));
            }
        }
        TypedMap::StringFloat(values) => {
            for (name, &value) in values {
                place_named!(name, RuntimeVal::Float(value));
            }
        }
        TypedMap::StringBool(values) => {
            for (name, &value) in values {
                place_named!(name, RuntimeVal::Bool(value));
            }
        }
        TypedMap::Mixed(values) => {
            for (key, value) in values {
                let Some(name) = key.as_arc_str() else {
                    bail!("named argument key must be a string");
                };
                place_named!(name, copy_runtime_value(value, caller_heap, callee_heap)?);
            }
        }
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

fn write_named_args_to_frame_from_typed_map(
    function: &crate::vm::Function,
    named: &TypedMap,
    frame: &mut [RuntimeVal],
) -> Result<()> {
    if frame.len() < function.param_count as usize {
        bail!(
            "callee frame has {} slots, function requires {} params",
            frame.len(),
            function.param_count
        );
    }
    if function.param_names.len() != function.param_count as usize {
        bail!("Function does not expose named parameter metadata");
    }
    let positional_count = function.positional_param_count as usize;
    let mut seen = vec![false; function.param_count as usize - positional_count];

    macro_rules! place_named {
        ($name:expr, $value:expr) => {{
            let name_str: &str = ($name).as_ref();
            let Some(offset) = function.param_names[positional_count..]
                .iter()
                .position(|param| param.as_ref() == name_str)
            else {
                bail!("unknown named argument `{name_str}`");
            };
            if core::mem::replace(&mut seen[offset], true) {
                bail!("duplicate named argument `{name_str}`");
            }
            frame[positional_count + offset] = $value;
        }};
    }

    match named {
        TypedMap::StringMixed(values) => {
            for (name, value) in values {
                place_named!(name, *value);
            }
        }
        TypedMap::StringInt(values) => {
            for (name, &value) in values {
                place_named!(name, RuntimeVal::Int(value));
            }
        }
        TypedMap::StringFloat(values) => {
            for (name, &value) in values {
                place_named!(name, RuntimeVal::Float(value));
            }
        }
        TypedMap::StringBool(values) => {
            for (name, &value) in values {
                place_named!(name, RuntimeVal::Bool(value));
            }
        }
        TypedMap::Mixed(values) => {
            for (key, value) in values {
                let Some(name) = key.as_arc_str() else {
                    bail!("named argument key must be a string");
                };
                place_named!(name, *value);
            }
        }
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_named_stack_args_to_frame(
    function: &crate::vm::Function,
    positional: &[RuntimeVal],
    caller_stack: &[RuntimeVal],
    named_start: usize,
    named_count: u16,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    frame: &mut [RuntimeVal],
) -> Result<()> {
    if frame.len() < function.param_count as usize {
        bail!(
            "callee frame has {} slots, function requires {} params",
            frame.len(),
            function.param_count
        );
    }
    if function.param_names.len() != function.param_count as usize {
        bail!("Function does not expose named parameter metadata");
    }
    let positional_count = function.positional_param_count as usize;
    if positional.len() != positional_count {
        bail!(
            "Function expects {} positional arguments before named arguments, got {}",
            positional_count,
            positional.len()
        );
    }

    for (slot, value) in frame.iter_mut().take(positional_count).zip(positional) {
        *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
    }
    let mut seen = vec![false; function.param_count as usize - positional_count];
    let named_end = named_start + named_count as usize * 2;
    let Some(named_slots) = caller_stack.get(named_start..named_end) else {
        bail!("CallNamed argument window {}..{} out of bounds", named_start, named_end);
    };

    for pair in named_slots.chunks_exact(2) {
        let offset = {
            let name = call_named_arg_name(&pair[0], caller_heap)?;
            let Some(offset) = function.param_names[positional_count..]
                .iter()
                .position(|param| param.as_ref() == name)
            else {
                bail!("unknown named argument `{name}`");
            };
            if core::mem::replace(&mut seen[offset], true) {
                bail!("duplicate named argument `{name}`");
            }
            offset
        };
        frame[positional_count + offset] = copy_runtime_value(&pair[1], caller_heap, callee_heap)?;
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

pub fn runtime_value_to_callable_shared(
    value: &RuntimeVal,
    heap: &HeapStore,
    module: Arc<Module>,
    state: Arc<Mutex<RuntimeModuleState>>,
) -> Option<RuntimeCallable> {
    if let RuntimeVal::Obj(handle) = value
        && let Some(value) = heap.get(*handle)
        && let HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) = value
    {
        return Some(RuntimeCallable::with_shared_captures(
            module,
            *function_index,
            Arc::clone(captures),
            state,
        ));
    }
    None
}

/// How a deep copy treats plain `Closure` values (`function_index` +
/// captures, no module attached).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClosureCopy {
    /// Reject: the destination may run a *different* module, where the bare
    /// `function_index` would be meaningless (channel payloads, cross-VM
    /// imports use the promote-to-`RuntimeCallable` path instead).
    Reject,
    /// Copy structurally (`function_index` kept, captures deep-copied): only
    /// sound when the destination provably executes the *same* `Module` —
    /// the `spawn`/`go` snapshot is the use case.
    SameModule,
}

pub fn copy_runtime_value(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<RuntimeVal> {
    copy_runtime_value_with(value, source_heap, dest_heap, ClosureCopy::Reject)
}

/// Same-module deep copy: closures are copied structurally. See
/// [`ClosureCopy::SameModule`] for when this is sound.
pub fn copy_runtime_value_same_module(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<RuntimeVal> {
    copy_runtime_value_with(value, source_heap, dest_heap, ClosureCopy::SameModule)
}

fn copy_runtime_value_with(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
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
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            copy_heap_value(value, source_heap, dest_heap, mode).map(|value| RuntimeVal::Obj(dest_heap.alloc(value)))
        }
    }
}

fn copy_heap_value(
    value: &HeapValue,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
) -> Result<HeapValue> {
    Ok(match value {
        HeapValue::String(value) => HeapValue::String(Arc::clone(value)),
        HeapValue::Bytes(value) => HeapValue::Bytes(Arc::clone(value)),
        HeapValue::List(values) => HeapValue::List(copy_typed_list(values, source_heap, dest_heap, mode)?),
        HeapValue::Map(values) => HeapValue::Map(copy_typed_map(values, source_heap, dest_heap, mode)?),
        HeapValue::Set(values) => HeapValue::Set(copy_runtime_set(values, source_heap, dest_heap, mode)?),
        HeapValue::Object(object) => {
            let mut fields = fast_hash_map_new();
            for (key, value) in &object.fields {
                fields.insert(
                    Arc::clone(key),
                    copy_runtime_value_with(value, source_heap, dest_heap, mode)?,
                );
            }
            HeapValue::Object(RuntimeObject::new(Arc::clone(&object.type_name), fields))
        }
        HeapValue::Callable(CallableValue::RuntimeNative { name, arity, function }) => {
            HeapValue::Callable(CallableValue::RuntimeNative {
                name: name.clone(),
                arity: *arity,
                function: function.clone(),
            })
        }
        HeapValue::Callable(CallableValue::Runtime(function)) => {
            HeapValue::Callable(CallableValue::Runtime(Arc::clone(function)))
        }
        HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) => match mode {
            ClosureCopy::Reject => bail!("cannot copy closure without module context"),
            ClosureCopy::SameModule => {
                let mut copied = Vec::with_capacity(captures.len());
                for value in captures.iter() {
                    copied.push(copy_runtime_value_with(value, source_heap, dest_heap, mode)?);
                }
                HeapValue::Callable(CallableValue::Closure {
                    function_index: *function_index,
                    captures: Arc::new(copied),
                })
            }
        },
        HeapValue::Task(value) => HeapValue::Task(value.clone()),
        HeapValue::Channel(value) => HeapValue::Channel(value.clone()),
        HeapValue::Stream(value) => HeapValue::Stream(value.clone()),
        HeapValue::StreamCursor(value) => HeapValue::StreamCursor(value.clone()),
        HeapValue::Slice(value) => HeapValue::Slice(Arc::new(crate::val::SliceValue {
            source: copy_runtime_value_with(&value.source, source_heap, dest_heap, mode)?,
            kind: value.kind,
            start: value.start,
            len: value.len,
        })),
        HeapValue::Resource(value) => HeapValue::Resource(value.clone()),
        HeapValue::UpvalCell(value) => {
            HeapValue::UpvalCell(copy_runtime_value_with(value, source_heap, dest_heap, mode)?)
        }
        HeapValue::ErrorVal(error) => HeapValue::ErrorVal(crate::val::ErrorVal {
            message: Arc::clone(&error.message),
            trace: {
                let mut trace = Vec::with_capacity(error.trace.len());
                for value in &error.trace {
                    trace.push(copy_runtime_value_with(value, source_heap, dest_heap, mode)?);
                }
                trace
            },
        }),
    })
}

fn copy_runtime_set(
    values: &RuntimeSet,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
) -> Result<RuntimeSet> {
    let mut out = fast_hash_set_new();
    for key in values.entries() {
        out.insert(copy_runtime_map_key(key, source_heap, dest_heap, mode)?);
    }
    Ok(RuntimeSet::from_entries(out))
}

fn copy_typed_list(
    values: &TypedList,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
) -> Result<TypedList> {
    Ok(match values {
        TypedList::Mixed(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(copy_runtime_value_with(value, source_heap, dest_heap, mode)?);
            }
            TypedList::Mixed(out)
        }
        TypedList::Int(values) => TypedList::Int(copy_slice(values)),
        TypedList::Float(values) => TypedList::Float(copy_slice(values)),
        TypedList::Bool(values) => TypedList::Bool(copy_slice(values)),
        TypedList::String(values) => TypedList::String(copy_slice(values)),
    })
}

fn copy_typed_map(
    values: &TypedMap,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => TypedMap::Mixed(copy_runtime_entries(values, source_heap, dest_heap, mode)?),
        TypedMap::StringMixed(values) => {
            let mut out = fast_hash_map_new();
            for (key, value) in values {
                out.insert(
                    Arc::clone(key),
                    copy_runtime_value_with(value, source_heap, dest_heap, mode)?,
                );
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(values) => TypedMap::StringInt(copy_string_map_values(values)),
        TypedMap::StringFloat(values) => TypedMap::StringFloat(copy_string_map_values(values)),
        TypedMap::StringBool(values) => TypedMap::StringBool(copy_string_map_values(values)),
    })
}

fn copy_slice<T: Clone>(values: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(values);
    out
}

fn copy_string_map_values<T: Copy>(values: &FastHashMap<Arc<str>, T>) -> FastHashMap<Arc<str>, T> {
    let mut out = fast_hash_map_new();
    for (key, value) in values {
        out.insert(Arc::clone(key), *value);
    }
    out
}

fn copy_runtime_entries(
    values: &FastHashMap<RuntimeMapKey, RuntimeVal>,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
) -> Result<FastHashMap<RuntimeMapKey, RuntimeVal>> {
    let mut out = fast_hash_map_new();
    for (key, value) in values {
        out.insert(
            copy_runtime_map_key(key, source_heap, dest_heap, mode)?,
            copy_runtime_value_with(value, source_heap, dest_heap, mode)?,
        );
    }
    Ok(out)
}

fn copy_runtime_map_key(
    key: &RuntimeMapKey,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: ClosureCopy,
) -> Result<RuntimeMapKey> {
    Ok(match key {
        RuntimeMapKey::Nil => RuntimeMapKey::Nil,
        RuntimeMapKey::Bool(value) => RuntimeMapKey::Bool(*value),
        RuntimeMapKey::Int(value) => RuntimeMapKey::Int(*value),
        RuntimeMapKey::ShortStr(value) => RuntimeMapKey::ShortStr(*value),
        RuntimeMapKey::String(value) => RuntimeMapKey::String(Arc::clone(value)),
        RuntimeMapKey::Obj(handle) => {
            match copy_runtime_value_with(&RuntimeVal::Obj(*handle), source_heap, dest_heap, mode)? {
                RuntimeVal::Obj(handle) => RuntimeMapKey::Obj(handle),
                _ => unreachable!("object map key copy must stay an object"),
            }
        }
    })
}
