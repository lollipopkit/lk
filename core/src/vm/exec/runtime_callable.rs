#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::compat::sync::Mutex;
use crate::util::value_map::{ValueMap, value_map_new};
use alloc::borrow::Cow;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{
        CallableValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, TypedList, TypedMap,
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
    let state = take_runtime_callable_state(function)
        .map_err(|reason| reason.into_error(&function.module, function.function_index))?;
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

#[allow(clippy::too_many_arguments)]
pub fn call_runtime_callable_runtime_named_stack(
    function: &RuntimeCallable,
    positional: &[RuntimeVal],
    caller_stack: &[RuntimeVal],
    named_start: usize,
    named_count: u16,
    caller_heap: &mut HeapStore,
    caller_module: Option<&Arc<Module>>,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let mode = crossing_mode(caller_module);
    let state = take_runtime_callable_state(function)
        .map_err(|reason| reason.into_error(&function.module, function.function_index))?;
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
                &mode,
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
    // The way back is the same crossing as the way in, and the module is known
    // here without asking anyone: it is the callee's own. Without this
    // `fn make_adder(n) -> (Int) -> Int` could not hand its closure back —
    // returning a function was refused while passing one had just started
    // working.
    let value = copy_runtime_value_with(
        &value,
        &result.state.heap,
        caller_heap,
        &ClosureCopy::Promote(Arc::clone(&function.module)),
    )?;
    commit_runtime_callable_state(function, result.state)?;
    Ok(value)
}

pub fn call_runtime_callable_runtime(
    function: &RuntimeCallable,
    args: &[RuntimeVal],
    caller_heap: &mut HeapStore,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_callable_runtime_positional(function, RuntimePositionalArgs::Slice(args), caller_heap, None, ctx)
}

/// [`call_runtime_callable_runtime`] told which module the arguments come from.
///
/// Only the executor knows that, and only it can say it: a *function* among
/// those arguments is a bare index into the caller's table, so without the
/// caller's module there is nothing to promote it against. Every other caller
/// (a stdlib HOF re-entering the VM, a test) passes `None` and keeps the old
/// refusal.
pub fn call_runtime_callable_runtime_from(
    function: &RuntimeCallable,
    args: &[RuntimeVal],
    caller_heap: &mut HeapStore,
    caller_module: Option<&Arc<Module>>,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    call_runtime_callable_runtime_positional(
        function,
        RuntimePositionalArgs::Slice(args),
        caller_heap,
        caller_module,
        ctx,
    )
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

/// The `Type::method` a dispatch was resolved from, carried for diagnostics
/// only: the table entry itself is just an index, which says nothing useful in
/// an error message.
#[derive(Clone, Copy)]
pub struct TraitMethodRef<'a> {
    pub type_name: &'a str,
    pub method: &'a str,
}

/// Invokes a trait-impl method straight from the runtime method table.
///
/// This is the only way a `MethodImpl` is called. Both variants used to be
/// materialized into a heap `Callable` first, which
/// `call_runtime_value_with_map_args` then immediately destructured back
/// into `(function_index, captures)` — one heap object per dispatch, allocated
/// through `HeapStore::alloc` directly, which (unlike
/// `Executor::alloc_heap_value`) does not even arm the collector. A loop
/// calling a trait method grew ~130 bytes per call and never gave any of it
/// back. Dispatching from the table entry skips the round trip entirely.
pub fn call_trait_method(
    method: &crate::vm::MethodImpl,
    name: TraitMethodRef<'_>,
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
    match method {
        crate::vm::MethodImpl::Local {
            module: declaring,
            function,
        } => {
            let executing = module.ok_or_else(|| anyhow!("trait method dispatch requires Module context"))?;
            if core::ptr::eq(Arc::as_ptr(declaring), executing as *const Module) {
                return call_closure_value(*function, Arc::new(Vec::new()), pos, state, Some(executing), ctx);
            }
            call_foreign_module_method(declaring, *function, name, pos, state, ctx)
        }
        crate::vm::MethodImpl::Imported(callable) => {
            // A method reached while its *own* module is the one executing does
            // not borrow that module's state — it already has it.
            //
            // `take_runtime_callable_state` moves the shared state out of its
            // mutex and leaves a `Default::default()` behind until the call
            // returns, so the mechanism is non-reentrant by construction. And
            // "a method that calls another method on `self`" re-enters by
            // definition: the outer call took the state, the inner call took the
            // empty shell, and the executor refused a module wanting 83 globals
            // against a table of 0 — a message about globals for a program that
            // never mentions one. Every cross-module `impl` was affected the
            // moment one of its methods called another (trait default body,
            // inherent method, inherent calling a trait method: all three).
            //
            // The `Local` arm one branch up already asks exactly this question
            // for the same reason; this arm did not.
            if let Some(executing) = module
                && core::ptr::eq(Arc::as_ptr(&callable.module), executing as *const Module)
            {
                return call_closure_value(
                    callable.function_index,
                    Arc::clone(&callable.captures),
                    pos,
                    state,
                    Some(executing),
                    ctx,
                );
            }
            // Same problem one step further out: module A's method calls into B,
            // and B calls back into A. A is not the module executing here (B
            // is), so the branch above does not fire — but A's state is out on
            // the stack, so borrowing it is impossible too.
            //
            // Borrowing is not the only way to run a foreign body, though.
            // `call_foreign_module_method` exists for exactly this shape: it
            // keeps the *current* heap and swaps in a global table of the
            // declaring module's shape, so it needs the module, not the module's
            // state. Taking that route makes A→B→A work, and a body that writes
            // a global — the one thing the borrowed path could do and this one
            // cannot — is refused there by name instead of corrupted.
            if runtime_callable_module_is_executing(callable.as_ref()) {
                return call_foreign_module_method(&callable.module, callable.function_index, name, pos, state, ctx);
            }
            call_runtime_callable_runtime_positional(callable.as_ref(), pos, &mut state.heap, None, ctx)
        }
    }
}

/// Dispatches an impl method whose body lives in a module *other* than the one
/// whose frame is executing — an `impl` in one file, reached inside a function
/// imported from another.
///
/// The receiver already lives in this heap, and the body's constants come from
/// its own `Function`, so the only things tying a function index to its module
/// are the global table and the `pc`-keyed inline caches. Both are swapped for
/// the duration of the call:
///
/// - **Globals** get a table of the declaring module's shape with only the slots
///   the body is proven to read filled in (`ImplMethod::reads_globals`) — for
///   most methods, none. A *write* cannot be supported at all: it would land in
///   this temporary table and vanish on restore, so a body that writes globals
///   (or that can reach code this analysis cannot see) is refused instead.
/// - **Inline caches** are keyed by `pc` alone, and a foreign function's pcs
///   mean nothing here. A fresh scope for the call keeps the two sets from
///   mixing; the host's caches are restored untouched afterwards.
#[cold]
fn call_foreign_module_method(
    declaring: &Arc<Module>,
    function: u32,
    name: TraitMethodRef<'_>,
    pos: RuntimePositionalArgs<'_>,
    state: &mut RuntimeModuleState,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let decl = declaring.type_info.method_by_function(function).ok_or_else(|| {
        anyhow!(
            "trait method `{}::{}` is not declared by its module",
            name.type_name,
            name.method
        )
    })?;
    if decl.writes_globals {
        bail!(
            "trait method `{}::{}` is declared in a different module than the one currently executing and is not \
             dispatchable across that boundary: it writes a module global (or makes a call whose target cannot be \
             resolved statically), and the write would be made against a temporary copy of its module's globals \
             and lost. Move the `impl` into the module that calls the method, or pass the method in as a value \
             (see docs/vm-cross-module-dispatch.md)",
            name.type_name,
            name.method
        );
    }
    let globals = {
        let ctx_ref = ctx
            .as_deref()
            .ok_or_else(|| anyhow!("cross-module trait method dispatch requires a VM context"))?;
        seed_foreign_globals(declaring, &decl.reads_globals, ctx_ref, &mut state.heap)?
    };
    let saved_globals = core::mem::replace(&mut state.globals, globals);
    let saved_caches = core::mem::take(&mut state.inline_caches);
    // The host frame's globals are off the root set while they sit in this
    // local, and the call below can collect. Pin them the way any host holding
    // heap references across a re-entrant VM call has to.
    let roots_mark = state.host_roots_mark();
    state.host_roots_extend(&saved_globals);
    let result = call_closure_value(
        function,
        Arc::new(Vec::new()),
        pos,
        state,
        Some(declaring.as_ref()),
        ctx,
    );
    state.host_roots_truncate(roots_mark);
    state.globals = saved_globals;
    state.inline_caches = saved_caches;
    result
}

/// A global table shaped like `module`'s, with only `slots` imported into
/// `heap`. Every other entry is `Nil` and unreachable: the caller has proven
/// the body reads nothing else.
fn seed_foreign_globals(
    module: &Module,
    slots: &[u16],
    ctx: &VmContext,
    heap: &mut HeapStore,
) -> Result<Vec<RuntimeVal>> {
    let mut globals = vec![RuntimeVal::Nil; module.globals.len()];
    for &slot in slots {
        let Some(name) = module.globals.get(slot as usize).map(|slot| &slot.name) else {
            bail!("impl method reads global slot {slot} out of bounds for its module");
        };
        if let Some(export) = ctx.get_runtime_global(name.as_ref()) {
            globals[slot as usize] = super::imports::import_runtime_export(export, heap)?;
        }
    }
    Ok(globals)
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
        bail!("this value is not a function");
    };
    let callable = callable_target(
        None,
        state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
        "this value is not a function",
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
                    name: Cow::Borrowed("<runtime-native>"),
                    arity,
                    function,
                };
                call_runtime_native_positional(&native, pos, state, module, ctx, callee_root)
            }
            CallableTarget::Runtime(function) => {
                call_runtime_callable_runtime_positional(function.as_ref(), pos, &mut state.heap, None, ctx)
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
                name: Cow::Borrowed("<runtime-native>"),
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
    callee.captures = Some(captures);
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
    callee.captures = Some(captures);
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
    caller_module: Option<&Arc<Module>>,
    ctx: Option<&mut crate::vm::VmContext>,
) -> Result<RuntimeVal> {
    let mode = crossing_mode(caller_module);
    let state = take_runtime_callable_state(function)
        .map_err(|reason| reason.into_error(&function.module, function.function_index))?;
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
            copy_runtime_positional_args_to_frame(function_meta, pos, caller_heap, heap, frame, &mode)?;
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
    // The way back is the same crossing as the way in, and the module is known
    // here without asking anyone: it is the callee's own. Without this
    // `fn make_adder(n) -> (Int) -> Int` could not hand its closure back —
    // returning a function was refused while passing one had just started
    // working.
    let value = copy_runtime_value_with(
        &value,
        &result.state.heap,
        caller_heap,
        &ClosureCopy::Promote(Arc::clone(&function.module)),
    )?;
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
    let state = take_runtime_callable_state(function)
        .map_err(|reason| reason.into_error(&function.module, function.function_index))?;
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
            copy_runtime_positional_args_with_named_map_to_frame(
                function_meta,
                pos,
                named,
                caller_heap,
                heap,
                frame,
                &ClosureCopy::Reject,
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
    // The way back is the same crossing as the way in, and the module is known
    // here without asking anyone: it is the callee's own. Without this
    // `fn make_adder(n) -> (Int) -> Int` could not hand its closure back —
    // returning a function was refused while passing one had just started
    // working.
    let value = copy_runtime_value_with(
        &value,
        &result.state.heap,
        caller_heap,
        &ClosureCopy::Promote(Arc::clone(&function.module)),
    )?;
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

/// Move a module's state out of its shared cell for the duration of one call.
///
/// `Err` when the state is already out — i.e. this call re-enters a module that
/// is live further up the stack. The caller has to decide what that means; what
/// it must not do is run against the placeholder, which is an empty state that
/// looks perfectly valid and produces "module expected N globals, got 0" several
/// frames later.
fn take_runtime_callable_state(function: &RuntimeCallable) -> Result<RuntimeModuleState, ReentrantModule> {
    let mut cell = function.state.lock().map_err(|_| ReentrantModule::PoisonedLock)?;
    if cell.borrowed_for_call {
        return Err(ReentrantModule::AlreadyExecuting);
    }
    let taken = core::mem::take(&mut *cell);
    cell.borrowed_for_call = true;
    Ok(taken)
}

/// Whether a call into this callable's module is already in progress.
fn runtime_callable_module_is_executing(function: &RuntimeCallable) -> bool {
    function
        .state
        .lock()
        .map(|cell| cell.borrowed_for_call)
        .unwrap_or(false)
}

/// Why a module's state could not be taken.
enum ReentrantModule {
    /// A call into this module is already in progress further up the stack.
    AlreadyExecuting,
    PoisonedLock,
}

impl ReentrantModule {
    fn into_error(self, module: &Module, function_index: u32) -> anyhow::Error {
        match self {
            Self::PoisonedLock => anyhow!("RuntimeCallable state lock poisoned"),
            Self::AlreadyExecuting => anyhow!(
                "function {function_index} of a module with {} globals was re-entered while that module was already \
                 executing further up the call stack, and its state cannot be lent to two frames at once",
                module.globals.len()
            ),
        }
    }
}

#[cfg(test)]
fn checked_arg_count(len: usize) -> Result<u16> {
    u16::try_from(len).map_err(|_| anyhow!("function arg count {} exceeds u16", len))
}

/// How a function value among the arguments is treated on the way across.
///
/// With the caller's module known it is promoted to a callable that carries
/// that module; without it the copy has nothing to attach and refuses, which is
/// the old behaviour for every path that cannot say where the value came from.
fn crossing_mode(caller_module: Option<&Arc<Module>>) -> ClosureCopy {
    caller_module.map_or(ClosureCopy::Reject, |module| ClosureCopy::Promote(Arc::clone(module)))
}

fn copy_runtime_positional_args_to_frame(
    function: &crate::vm::Function,
    pos: RuntimePositionalArgs<'_>,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    frame: &mut [RuntimeVal],
    mode: &ClosureCopy,
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
    copy_runtime_positional_args_into_slots(pos, caller_heap, callee_heap, &mut frame[..expected], mode)
}

fn copy_runtime_positional_args_with_named_map_to_frame(
    function: &crate::vm::Function,
    pos: RuntimePositionalArgs<'_>,
    named: &TypedMap,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    frame: &mut [RuntimeVal],
    mode: &ClosureCopy,
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
    copy_runtime_positional_args_into_slots(pos, caller_heap, callee_heap, &mut frame[..positional_count], mode)?;
    copy_typed_map_named_args_to_frame(function, named, caller_heap, callee_heap, frame)
}

fn copy_runtime_positional_args_into_slots(
    pos: RuntimePositionalArgs<'_>,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    slots: &mut [RuntimeVal],
    mode: &ClosureCopy,
) -> Result<()> {
    match pos {
        RuntimePositionalArgs::Slice(values) => {
            for (slot, value) in slots.iter_mut().zip(values) {
                *slot = copy_runtime_value_with(value, caller_heap, callee_heap, mode)?;
            }
            Ok(())
        }
        RuntimePositionalArgs::ListHandle(handle) => {
            copy_typed_list_arg_handle_to_slots(handle, caller_heap, callee_heap, slots, mode)
        }
        RuntimePositionalArgs::Prefixed { first, rest } => {
            let Some((first_slot, rest_slots)) = slots.split_first_mut() else {
                bail!("runtime positional argument frame is empty");
            };
            *first_slot = copy_runtime_value_with(first, caller_heap, callee_heap, mode)?;
            for (slot, value) in rest_slots.iter_mut().zip(rest) {
                *slot = copy_runtime_value_with(value, caller_heap, callee_heap, mode)?;
            }
            Ok(())
        }
        RuntimePositionalArgs::PrefixedList { first, rest } => {
            let Some((first_slot, rest_slots)) = slots.split_first_mut() else {
                bail!("runtime positional argument frame is empty");
            };
            *first_slot = copy_runtime_value_with(first, caller_heap, callee_heap, mode)?;
            copy_typed_list_arg_handle_to_slots(rest, caller_heap, callee_heap, rest_slots, mode)
        }
    }
}

fn copy_typed_list_arg_handle_to_slots(
    handle: HeapRef,
    caller_heap: &HeapStore,
    callee_heap: &mut HeapStore,
    slots: &mut [RuntimeVal],
    mode: &ClosureCopy,
) -> Result<()> {
    match caller_heap
        .get(handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => {
            for (slot, value) in slots.iter_mut().zip(values) {
                *slot = copy_runtime_value_with(value, caller_heap, callee_heap, mode)?;
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
    mode: &ClosureCopy,
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
        *slot = copy_runtime_value_with(value, caller_heap, callee_heap, mode)?;
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
        frame[positional_count + offset] = copy_runtime_value_with(&pair[1], caller_heap, callee_heap, mode)?;
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

/// A function value crossing into another module, as a callable that carries
/// its own module.
///
/// The problem this solves: a bare `Closure` is a `function_index` into *its
/// own* module's function table, so the moment it lands in another module it
/// indexes a different table — which is why the copy used to refuse it and
/// `apply(double, 5)` across two files did not work at all.
///
/// The promoted callable holds the defining module, so the index means what it
/// meant. What it does *not* hold is that module's live state: the caller's
/// state belongs to a frame further down the Rust stack and cannot be taken
/// while it is running. So the callable gets a **private, empty** state — a
/// fresh heap that its arguments are copied into and its result copied out of,
/// which is exactly what every `RuntimeCallable` call already does.
///
/// That is sound only if the function needs nothing else from its module, and
/// the one thing left is the globals. Hence the refusal below, with the same
/// analysis a cross-module trait dispatch uses
/// ([`crate::vm::analysis::function_global_use`]) — a function that reads a
/// module global would read `nil` here, and one that writes would write into a
/// table nobody will ever look at again. Both are wrong answers rather than
/// slow ones, so they are refused, by name.
fn promote_crossing_closure(
    module: &Arc<Module>,
    function_index: u32,
    captures: &[RuntimeVal],
    source_heap: &HeapStore,
) -> Result<HeapValue> {
    let global_use = crate::vm::analysis::function_global_use(module, function_index);
    // A lambda has no name, and "`#3` cannot be passed out" would be useless —
    // so an anonymous one is described by what it is instead.
    let name = module
        .functions
        .get(function_index as usize)
        .and_then(|function| function.debug_name.clone())
        .map_or_else(
            || alloc::string::String::from("this lambda"),
            |name| alloc::format!("`{name}`"),
        );
    match global_use {
        crate::vm::analysis::GlobalUse::Writes => bail!(
            "{name} cannot be passed out of the module that defined it: it writes a module global. A function \
             that crosses a module boundary runs against a fresh state, so the write would land in a table \
             nobody reads again. Return the new value instead of storing it"
        ),
        // Not the same refusal as a write, and worth its own sentence: nothing
        // is known to be wrong here, only unproven. `println` lands in this
        // case — a builtin arrives through a register, and a call this walk
        // cannot follow could reach anything, including a global.
        crate::vm::analysis::GlobalUse::OpaqueCall => bail!(
            "{name} cannot be passed out of the module that defined it: it makes a call this check cannot follow \
             (a builtin such as `println`, a function held in a variable, or a method), so it cannot be shown to \
             leave its module's globals alone — and a function that crosses a module boundary runs against a \
             fresh state where they are all nil. Do that work on this side of the boundary, or return the value \
             and let the caller print it"
        ),
        crate::vm::analysis::GlobalUse::Reads(reads) => {
            if let Some(slot) = reads.first() {
                let global = module
                    .globals
                    .get(*slot as usize)
                    .map(|slot| slot.name.to_string())
                    .unwrap_or_else(|| alloc::format!("#{slot}"));
                bail!(
                    "{name} cannot be passed out of the module that defined it: it reads the module global \
                     `{global}`, and a function that crosses a module boundary runs against a fresh state where \
                     that global is nil. Pass the value in as an argument instead"
                );
            }
        }
    }
    // The globals table is the module's shape, filled with nil: the executor
    // checks the width on entry, and the analysis above has already proven that
    // no slot is read.
    let mut state = RuntimeModuleState {
        globals: alloc::vec![RuntimeVal::Nil; module.globals.len()],
        ..RuntimeModuleState::default()
    };
    let mut copied = Vec::with_capacity(captures.len());
    for value in captures {
        // The captures come along, into the callable's own heap: a promoted
        // `|x| x + n` has to keep its `n`, and a capture that is itself a
        // function of this module promotes the same way.
        copied.push(copy_runtime_value_with(
            value,
            source_heap,
            &mut state.heap,
            &ClosureCopy::Promote(Arc::clone(module)),
        )?);
    }
    Ok(HeapValue::Callable(CallableValue::Runtime(Arc::new(
        RuntimeCallable::with_shared_captures(
            Arc::clone(module),
            function_index,
            Arc::new(copied),
            Arc::new(Mutex::new(state)),
        ),
    ))))
}

/// How a deep copy treats plain `Closure` values (`function_index` +
/// captures, no module attached).
#[derive(Clone)]
pub enum ClosureCopy {
    /// Reject: the destination may run a *different* module, where the bare
    /// `function_index` would be meaningless, and the copy does not know which
    /// module the value came from. A channel payload is the case left here.
    Reject,
    /// Copy structurally (`function_index` kept, captures deep-copied): only
    /// sound when the destination provably executes the *same* `Module` —
    /// the `spawn`/`go` snapshot is the use case.
    SameModule,
    /// Promote to a [`RuntimeCallable`] carrying this module: the value is
    /// crossing into another module, and a function that knows its own module
    /// is callable from anywhere.
    ///
    /// This is what makes `apply(double, 5)` work across files. The promotion
    /// is refused for a function whose reachable subtree touches its module's
    /// globals — see [`promote_crossing_closure`] for why that is the line.
    Promote(Arc<Module>),
}

pub fn copy_runtime_value(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<RuntimeVal> {
    copy_runtime_value_with(value, source_heap, dest_heap, &ClosureCopy::Reject)
}

/// Same-module deep copy: closures are copied structurally. See
/// [`ClosureCopy::SameModule`] for when this is sound.
pub fn copy_runtime_value_same_module(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
) -> Result<RuntimeVal> {
    copy_runtime_value_with(value, source_heap, dest_heap, &ClosureCopy::SameModule)
}

fn copy_runtime_value_with(
    value: &RuntimeVal,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: &ClosureCopy,
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
    mode: &ClosureCopy,
) -> Result<HeapValue> {
    Ok(match value {
        HeapValue::String(value) => HeapValue::String(Arc::clone(value)),
        HeapValue::Bytes(value) => HeapValue::Bytes(Arc::clone(value)),
        HeapValue::List(values) => HeapValue::List(copy_typed_list(values, source_heap, dest_heap, mode)?),
        HeapValue::Map(values) => HeapValue::Map(copy_typed_map(values, source_heap, dest_heap, mode)?),
        // No member carries a heap handle — see `imports::import_runtime_set`.
        HeapValue::Set(values) => HeapValue::Set(values.clone()),
        HeapValue::Object(object) => {
            let mut fields = value_map_new();
            for (key, value) in &object.fields {
                fields.insert(
                    Arc::clone(key),
                    copy_runtime_value_with(value, source_heap, dest_heap, mode)?,
                );
            }
            HeapValue::Object(RuntimeObject::new(Arc::clone(&object.ty), fields))
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
            // The old text was "cannot copy closure without module context",
            // which names a parameter of this function and nothing the program
            // did. What the program did is hand a function to another module —
            // as an argument to an imported function, or as a channel payload —
            // and a bare closure is a `function_index` into *its own* module's
            // table, meaningless once it lands anywhere else.
            //
            // The export direction already solves this: `import_runtime_export`
            // promotes a crossing closure to a `RuntimeCallable`, which carries
            // its module with it. The argument direction cannot yet, because the
            // promotion also wants the caller module's shared state and the entry
            // module has none — see the task tracking the module-bound callable
            // that would close it.
            ClosureCopy::Reject => bail!(
                "a function value cannot be passed out of the module that defined it here (a channel payload). A \
                 function carries an index into its own module's table, and this crossing does not record which \
                 module that is. Passing a function *as an argument* to an imported function does work — send the \
                 data through the channel and call the function on the other side"
            ),
            ClosureCopy::Promote(module) => {
                return promote_crossing_closure(module, *function_index, captures, source_heap);
            }
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

fn copy_typed_list(
    values: &TypedList,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: &ClosureCopy,
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
    mode: &ClosureCopy,
) -> Result<TypedMap> {
    Ok(match values {
        TypedMap::Mixed(values) => TypedMap::Mixed(copy_runtime_entries(values, source_heap, dest_heap, mode)?),
        TypedMap::StringMixed(values) => {
            let mut out = value_map_new();
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

fn copy_string_map_values<T: Copy>(values: &ValueMap<Arc<str>, T>) -> ValueMap<Arc<str>, T> {
    let mut out = value_map_new();
    for (key, value) in values {
        out.insert(Arc::clone(key), *value);
    }
    out
}

fn copy_runtime_entries(
    values: &ValueMap<RuntimeMapKey, RuntimeVal>,
    source_heap: &HeapStore,
    dest_heap: &mut HeapStore,
    mode: &ClosureCopy,
) -> Result<ValueMap<RuntimeMapKey, RuntimeVal>> {
    let mut out = value_map_new();
    for (key, value) in values {
        out.insert(
            key.clone(),
            copy_runtime_value_with(value, source_heap, dest_heap, mode)?,
        );
    }
    Ok(out)
}
