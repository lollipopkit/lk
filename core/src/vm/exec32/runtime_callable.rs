use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{
        CallableValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, TypedList, TypedMap,
    },
    vm::{Module32, NativeArgs32, NativeEntry32, RuntimeCallable32, RuntimeModuleState32, VmContext},
};

use super::{
    Exec32Failure, Executor32,
    call::{CallableTarget32, callable_target32},
    named_call::call_named_arg_name,
    support::{call_native_entry, call_native_entry_with_args},
};

const MAX_INLINE_POSITIONAL_ARGS32: usize = u8::MAX as usize + 1;

#[cfg(test)]
pub(crate) fn call_runtime_callable32_test(
    function: &RuntimeCallable32,
    args: &[RuntimeVal],
    ctx: &mut crate::vm::VmContext,
) -> Result<Vec<RuntimeVal>> {
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
            let Exec32Failure { error, state } = failure;
            commit_runtime_callable32_state(function, state)?;
            return Err(error);
        }
    };
    let super::Exec32Result { returns, state } = result;
    commit_runtime_callable32_state(function, state)?;
    Ok(returns)
}

pub fn call_runtime_callable32_runtime_named_map(
    function: &RuntimeCallable32,
    pos: NativeArgs32<'_>,
    named: crate::val::HeapRef,
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
            copy_named_map_args32_to_frame(function_meta, pos, &named, caller_heap, heap, frame)?;
            Ok(function_meta.param_count)
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let Exec32Failure { error, state } = failure;
            commit_runtime_callable32_state(function, state)?;
            return Err(error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &mut result.state.heap, caller_heap)?;
    commit_runtime_callable32_state(function, result.state)?;
    Ok(value)
}

pub fn call_runtime_callable32_runtime_named_stack(
    function: &RuntimeCallable32,
    positional: &[RuntimeVal],
    caller_stack: &[RuntimeVal],
    named_start: usize,
    named_count: u16,
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
        Some(Arc::clone(&function.module)),
        function.function_index,
        Arc::clone(&function.captures),
        state,
        ctx,
        |executor| {
            let heap = &mut executor.state.heap;
            let frame = &mut executor.state.stack[..function_meta.register_count as usize];
            copy_named_stack_args32_to_frame(
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
            let Exec32Failure { error, state } = failure;
            commit_runtime_callable32_state(function, state)?;
            return Err(error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &mut result.state.heap, caller_heap)?;
    commit_runtime_callable32_state(function, result.state)?;
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
        Some(Arc::clone(&function.module)),
        function.function_index,
        Arc::clone(&function.captures),
        state,
        ctx,
        |executor| {
            let function_meta = function
                .module
                .functions
                .get(function.function_index as usize)
                .ok_or_else(|| anyhow!("function index {} out of bounds", function.function_index))?;
            let heap = &mut executor.state.heap;
            let frame = &mut executor.state.stack[..function_meta.register_count as usize];
            copy_native_args32_to_frame(function_meta, args, caller_heap, heap, frame)?;
            if args.has_named() {
                Ok(function_meta.param_count)
            } else {
                Ok(arg_count)
            }
        },
    ) {
        Ok(result) => result,
        Err(failure) => {
            let Exec32Failure { error, state } = failure;
            commit_runtime_callable32_state(function, state)?;
            return Err(error);
        }
    };
    let value = result.returns.first().cloned().unwrap_or(RuntimeVal::Nil);
    let value = copy_runtime_value(&value, &mut result.state.heap, caller_heap)?;
    commit_runtime_callable32_state(function, result.state)?;
    Ok(value)
}

pub fn call_runtime_value32_runtime(
    callee: RuntimeVal,
    args: &[RuntimeVal],
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_value32_with_map_args(callee, RuntimePositionalArgs::Slice(args), None, state, module, ctx)
}

pub fn call_runtime_value32_runtime_with_receiver(
    callee: RuntimeVal,
    receiver: &RuntimeVal,
    args: &[RuntimeVal],
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_value32_with_map_args(
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

pub fn call_runtime_value32_runtime_with_receiver_list_args(
    callee: RuntimeVal,
    receiver: &RuntimeVal,
    args: Option<HeapRef>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
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
    call_runtime_value32_with_map_args(callee, pos, None, state, module, ctx)
}

pub fn call_runtime_value32_runtime_list_args(
    callee: RuntimeVal,
    args: Option<HeapRef>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let pos = args.map_or(RuntimePositionalArgs::Slice(&[]), RuntimePositionalArgs::ListHandle);
    call_runtime_value32_with_map_args(callee, pos, None, state, module, ctx)
}

pub fn call_runtime_value32_runtime_named_map(
    callee: RuntimeVal,
    pos: &[RuntimeVal],
    named: Option<crate::val::HeapRef>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_value32_with_map_args(callee, RuntimePositionalArgs::Slice(pos), named, state, module, ctx)
}

pub fn call_runtime_value32_runtime_named_map_list_args(
    callee: RuntimeVal,
    pos: Option<HeapRef>,
    named: Option<crate::val::HeapRef>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let pos = pos.map_or(RuntimePositionalArgs::Slice(&[]), RuntimePositionalArgs::ListHandle);
    call_runtime_value32_with_map_args(callee, pos, named, state, module, ctx)
}

fn call_runtime_value32_with_map_args(
    callee: RuntimeVal,
    pos: RuntimePositionalArgs<'_>,
    named: Option<crate::val::HeapRef>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = callee else {
        bail!("runtime callee is not callable");
    };
    let callable = callable_target32(
        None,
        state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
        "runtime callee is not callable",
    )?;
    let Some(named_handle) = named else {
        return match callable {
            CallableTarget32::Closure {
                function_index,
                captures,
            } => call_closure_value32(function_index, captures, pos, state, module, ctx),
            CallableTarget32::RuntimeNative32 { arity, function } => {
                let pos_len = pos.len(&state.heap)?;
                if arity != NativeEntry32::VARIADIC && arity != pos_len as u16 {
                    bail!("Native expects {} positional arguments, got {}", arity, pos_len);
                }
                let native = NativeEntry32 {
                    name: "<runtime-native32>".to_string(),
                    arity,
                    function,
                };
                let pos = pos.materialize_inline(&mut state.heap)?;
                call_native_entry(&native, pos.as_slice(), state, module, None, ctx)
            }
            CallableTarget32::Runtime32(function) => {
                let pos = pos.materialize_inline(&mut state.heap)?;
                call_runtime_callable32_runtime(
                    function.as_ref(),
                    NativeArgs32::new(pos.as_slice()),
                    &mut state.heap,
                    ctx,
                )
            }
        };
    };
    match callable {
        CallableTarget32::Closure {
            function_index,
            captures,
        } => call_closure_value32_typed_map(function_index, captures, pos, named_handle, state, module, ctx),
        CallableTarget32::RuntimeNative32 { arity, function } => {
            let named_count = match state
                .heap
                .get(named_handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", named_handle.index()))?
            {
                HeapValue::Map(map) => map.len(),
                _ => bail!("named arguments must be a map"),
            };
            let pos_len = pos.len(&state.heap)?;
            if arity != NativeEntry32::VARIADIC && arity != pos_len as u16 {
                bail!("Native expects {} positional arguments, got {}", arity, pos_len);
            }
            let native = NativeEntry32 {
                name: "<runtime-native32>".to_string(),
                arity,
                function,
            };
            let pos = pos.materialize_inline(&mut state.heap)?;
            call_native_entry_with_args(
                &native,
                NativeArgs32::new_with_named_map_handle(pos.as_slice(), named_handle, named_count),
                state,
                module,
                None,
                ctx,
            )
        }
        CallableTarget32::Runtime32(function) => {
            let pos = pos.materialize_inline(&mut state.heap)?;
            call_runtime_callable32_runtime_named_map(
                function.as_ref(),
                NativeArgs32::new(pos.as_slice()),
                named_handle,
                &mut state.heap,
                ctx,
            )
        }
    }
}

#[derive(Clone, Copy)]
enum RuntimePositionalArgs<'a> {
    Slice(&'a [RuntimeVal]),
    ListHandle(HeapRef),
    Prefixed {
        first: &'a RuntimeVal,
        rest: &'a [RuntimeVal],
    },
    PrefixedList {
        first: &'a RuntimeVal,
        rest: HeapRef,
    },
}

enum RuntimePositionalSlice<'a> {
    Borrowed(&'a [RuntimeVal]),
    Inline {
        values: Box<[RuntimeVal; MAX_INLINE_POSITIONAL_ARGS32]>,
        len: usize,
    },
}

impl<'a> RuntimePositionalSlice<'a> {
    fn as_slice(&self) -> &[RuntimeVal] {
        match self {
            Self::Borrowed(values) => values,
            Self::Inline { values, len } => &values[..*len],
        }
    }
}

impl<'a> RuntimePositionalArgs<'a> {
    fn len(self, heap: &HeapStore) -> Result<usize> {
        match self {
            Self::Slice(values) => Ok(values.len()),
            Self::ListHandle(handle) => typed_list_arg_len(handle, heap),
            Self::Prefixed { rest, .. } => Ok(rest.len() + 1),
            Self::PrefixedList { rest, .. } => Ok(typed_list_arg_len(rest, heap)? + 1),
        }
    }

    fn materialize_inline(self, heap: &mut HeapStore) -> Result<RuntimePositionalSlice<'a>> {
        match self {
            Self::Slice(values) => Ok(RuntimePositionalSlice::Borrowed(values)),
            Self::ListHandle(handle) => {
                let len = typed_list_arg_len(handle, heap)?;
                let mut values = inline_positional_buffer(len)?;
                copy_list_handle_into_slots(handle, heap, &mut values[..len])?;
                Ok(RuntimePositionalSlice::Inline { values, len })
            }
            Self::Prefixed { first, rest } => {
                let len = rest.len() + 1;
                let mut values = inline_positional_buffer(len)?;
                values[0] = first.clone();
                for (slot, value) in values[1..len].iter_mut().zip(rest) {
                    *slot = value.clone();
                }
                Ok(RuntimePositionalSlice::Inline { values, len })
            }
            Self::PrefixedList { first, rest } => {
                let rest_len = typed_list_arg_len(rest, heap)?;
                let len = rest_len + 1;
                let mut values = inline_positional_buffer(len)?;
                values[0] = first.clone();
                copy_list_handle_into_slots(rest, heap, &mut values[1..len])?;
                Ok(RuntimePositionalSlice::Inline { values, len })
            }
        }
    }

    fn copy_into_frame(self, heap: &mut HeapStore, frame: &mut [RuntimeVal]) -> Result<()> {
        match self {
            Self::Slice(values) => {
                for (slot, value) in frame.iter_mut().zip(values) {
                    *slot = value.clone();
                }
                Ok(())
            }
            Self::ListHandle(handle) => copy_list_handle_into_slots(handle, heap, frame),
            Self::PrefixedList { first, rest } => {
                frame[0] = first.clone();
                copy_list_handle_into_slots(rest, heap, &mut frame[1..])
            }
            Self::Prefixed { first, rest } => {
                frame[0] = first.clone();
                for (slot, value) in frame[1..1 + rest.len()].iter_mut().zip(rest) {
                    *slot = value.clone();
                }
                Ok(())
            }
        }
    }
}

fn inline_positional_buffer(len: usize) -> Result<Box<[RuntimeVal; MAX_INLINE_POSITIONAL_ARGS32]>> {
    if len > MAX_INLINE_POSITIONAL_ARGS32 {
        bail!(
            "runtime positional argument count {} exceeds inline call buffer {}",
            len,
            MAX_INLINE_POSITIONAL_ARGS32
        );
    }
    Ok(Box::new(std::array::from_fn(|_| RuntimeVal::Nil)))
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

fn copy_list_handle_into_slots(handle: HeapRef, heap: &mut HeapStore, frame: &mut [RuntimeVal]) -> Result<()> {
    let long_string_values = match heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => {
            for (slot, value) in frame.iter_mut().zip(values) {
                *slot = value.clone();
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

fn call_closure_value32(
    function_index: u32,
    captures: Arc<Vec<RuntimeVal>>,
    pos: RuntimePositionalArgs<'_>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let module = module.ok_or_else(|| anyhow!("closure callable requires Module32 context"))?;
    let function = module
        .functions
        .get(function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
    let mut ctx = ctx;
    let mut callee = Executor32::new(function.register_count);
    callee.state = std::mem::take(state);
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
        callee.run_function_inner(function, Some(module), &mut ctx)
    })();
    callee.state.stack_top = saved_top;
    *state = callee.state;
    match result {
        Ok(returns) => Ok(returns.into_first()),
        Err(error) => Err(error),
    }
}

fn call_closure_value32_typed_map(
    function_index: u32,
    captures: Arc<Vec<RuntimeVal>>,
    pos: RuntimePositionalArgs<'_>,
    named: crate::val::HeapRef,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let module = module.ok_or_else(|| anyhow!("closure callable requires Module32 context"))?;
    let function = module
        .functions
        .get(function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
    let mut ctx = ctx;
    let mut callee = Executor32::new(function.register_count);
    callee.state = std::mem::take(state);
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
        write_named_args32_to_frame_from_typed_map(function, named, frame)?;
        callee.frame_base = new_base;
        callee.register_count = function.register_count;
        callee.state.stack_top = new_top;
        callee.pc = 0;
        callee.run_function_inner(function, Some(module), &mut ctx)
    })();
    callee.state.stack_top = saved_top;
    *state = callee.state;
    match result {
        Ok(returns) => Ok(returns.into_first()),
        Err(error) => Err(error),
    }
}

fn commit_runtime_callable32_state(function: &RuntimeCallable32, next_state: RuntimeModuleState32) -> Result<()> {
    let mut state = function
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeCallable32 state lock poisoned"))?;
    *state = next_state;
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

fn copy_native_args32_to_frame(
    function: &crate::vm::Function32,
    args: NativeArgs32<'_>,
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

    if !args.has_named() {
        if function.param_count != args.len() as u16 {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                args.len()
            );
        }
        for (slot, value) in frame.iter_mut().take(args.len()).zip(args.into_iter()) {
            *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
        }
        return Ok(());
    }

    if function.param_names.len() != function.param_count as usize {
        bail!("Function does not expose named parameter metadata");
    }
    let positional_count = function.positional_param_count as usize;
    if args.len() != positional_count {
        bail!(
            "Function expects {} positional arguments before named arguments, got {}",
            positional_count,
            args.len()
        );
    }

    for (slot, value) in frame.iter_mut().take(positional_count).zip(args.into_iter()) {
        *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
    }

    let mut seen = vec![false; function.param_count as usize - positional_count];
    args.try_for_each_named(caller_heap, |name, value| {
        let Some(offset) = function.param_names[positional_count..]
            .iter()
            .position(|param| param.as_ref() == name)
        else {
            bail!("unknown named argument `{name}`");
        };
        if std::mem::replace(&mut seen[offset], true) {
            bail!("duplicate named argument `{name}`");
        }
        frame[positional_count + offset] = copy_runtime_value(value, caller_heap, callee_heap)?;
        Ok(())
    })?;

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

fn write_named_args32_to_frame_from_typed_map(
    function: &crate::vm::Function32,
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
            if std::mem::replace(&mut seen[offset], true) {
                bail!("duplicate named argument `{name_str}`");
            }
            frame[positional_count + offset] = $value;
        }};
    }

    match named {
        TypedMap::StringMixed(values) => {
            for (name, value) in values {
                place_named!(name, value.clone());
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
                place_named!(name, value.clone());
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

fn copy_named_map_args32_to_frame(
    function: &crate::vm::Function32,
    positional: NativeArgs32<'_>,
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
    if positional.len() != positional_count {
        bail!(
            "Function expects {} positional arguments before named arguments, got {}",
            positional_count,
            positional.len()
        );
    }

    for (slot, value) in frame.iter_mut().take(positional_count).zip(positional.into_iter()) {
        *slot = copy_runtime_value(value, caller_heap, callee_heap)?;
    }
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
            if std::mem::replace(&mut seen[offset], true) {
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

fn copy_named_stack_args32_to_frame(
    function: &crate::vm::Function32,
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
            if std::mem::replace(&mut seen[offset], true) {
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

pub fn runtime_value_to_callable32_shared(
    value: &RuntimeVal,
    heap: &HeapStore,
    module: Arc<Module32>,
    state: Arc<Mutex<RuntimeModuleState32>>,
) -> Option<RuntimeCallable32> {
    if let RuntimeVal::Obj(handle) = value
        && let Some(value) = heap.get(*handle)
        && let HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        }) = value
    {
        return Some(RuntimeCallable32::with_shared_captures(
            module,
            *function_index,
            Arc::clone(captures),
            state,
        ));
    }
    None
}

pub fn copy_runtime_value(
    value: &RuntimeVal,
    source_heap: &HeapStore,
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
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            copy_heap_value(value, source_heap, dest_heap).map(|value| RuntimeVal::Obj(dest_heap.alloc(value)))
        }
    }
}

fn copy_heap_value(value: &HeapValue, source_heap: &HeapStore, dest_heap: &mut HeapStore) -> Result<HeapValue> {
    Ok(match value {
        HeapValue::String(value) => HeapValue::String(Arc::clone(value)),
        HeapValue::List(values) => HeapValue::List(copy_typed_list(values, source_heap, dest_heap)?),
        HeapValue::Map(values) => HeapValue::Map(copy_typed_map(values, source_heap, dest_heap)?),
        HeapValue::Object(object) => {
            let fields = object
                .fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), copy_runtime_value(value, source_heap, dest_heap)?)))
                .collect::<Result<_>>()?;
            HeapValue::Object(RuntimeObject::new(Arc::clone(&object.type_name), fields))
        }
        HeapValue::Callable(CallableValue::RuntimeNative32 { name, arity, function }) => {
            HeapValue::Callable(CallableValue::RuntimeNative32 {
                name: name.clone(),
                arity: *arity,
                function: function.clone(),
            })
        }
        HeapValue::Callable(CallableValue::Runtime32(function)) => {
            HeapValue::Callable(CallableValue::Runtime32(Arc::clone(function)))
        }
        HeapValue::Callable(CallableValue::Closure { .. }) => {
            bail!("cannot copy closure without module context")
        }
        HeapValue::Task(value) => HeapValue::Task(value.clone()),
        HeapValue::Channel(value) => HeapValue::Channel(value.clone()),
        HeapValue::Stream(value) => HeapValue::Stream(value.clone()),
        HeapValue::StreamCursor(value) => HeapValue::StreamCursor(value.clone()),
        HeapValue::UpvalCell(value) => HeapValue::UpvalCell(copy_runtime_value(&value, source_heap, dest_heap)?),
        HeapValue::ErrorVal(error) => HeapValue::ErrorVal(crate::val::ErrorVal {
            message: Arc::clone(&error.message),
            trace: error
                .trace
                .iter()
                .map(|value| copy_runtime_value(value, source_heap, dest_heap))
                .collect::<Result<_>>()?,
        }),
    })
}

fn copy_typed_list(values: &TypedList, source_heap: &HeapStore, dest_heap: &mut HeapStore) -> Result<TypedList> {
    Ok(match values {
        TypedList::Mixed(values) => TypedList::Mixed(
            values
                .iter()
                .map(|value| copy_runtime_value(value, source_heap, dest_heap))
                .collect::<Result<_>>()?,
        ),
        TypedList::Int(values) => TypedList::Int(values.clone()),
        TypedList::Float(values) => TypedList::Float(values.clone()),
        TypedList::Bool(values) => TypedList::Bool(values.clone()),
        TypedList::String(values) => TypedList::String(values.clone()),
    })
}

fn copy_typed_map(values: &TypedMap, source_heap: &HeapStore, dest_heap: &mut HeapStore) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => TypedMap::Mixed(copy_runtime_entries(values, source_heap, dest_heap)?),
        TypedMap::StringMixed(values) => TypedMap::StringMixed(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), copy_runtime_value(value, source_heap, dest_heap)?)))
                .collect::<Result<_>>()?,
        ),
        TypedMap::StringInt(values) => TypedMap::StringInt(values.clone()),
        TypedMap::StringFloat(values) => TypedMap::StringFloat(values.clone()),
        TypedMap::StringBool(values) => TypedMap::StringBool(values.clone()),
    })
}

fn copy_runtime_entries(
    values: &std::collections::BTreeMap<RuntimeMapKey, RuntimeVal>,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<std::collections::BTreeMap<RuntimeMapKey, RuntimeVal>> {
    values
        .iter()
        .map(|(key, value)| Ok((key.clone(), copy_runtime_value(value, source_heap, dest_heap)?)))
        .collect()
}
