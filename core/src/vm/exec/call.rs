#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapValue, RuntimeVal},
    vm::{CallWindow, Function, Module, NativeArgs, NativeEntry, VmContext, analysis::PerfCallTargetKind},
};

use super::{
    CallFrame, Executor,
    named_call::move_named_args_to_frame_from_stack,
    runtime_callable,
    support::{call_native_entry, call_native_entry_parts, move_inline_native_args_from_stack},
};

/// What resolving a call site produced (plan M2.5 sub-step ①): a `Runtime`/
/// `RuntimeNative` target still runs to completion synchronously (native
/// re-entry, unaffected by flattening); a `Closure` target instead pushes a
/// `CallFrame` and hands back its function index for the dispatch trampoline in
/// `exec.rs` to switch to.
pub(super) enum CallOutcome {
    Value(RuntimeVal),
    Pushed(u32),
}

pub(super) enum CallableTarget {
    Closure {
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
    },
    RuntimeNative {
        arity: u16,
        function: crate::vm::NativeFunction,
    },
    Runtime(Arc<crate::vm::RuntimeCallable>),
}

pub(super) fn callable_target(
    known_target_kind: Option<PerfCallTargetKind>,
    heap_value: &HeapValue,
    error: &'static str,
) -> Result<CallableTarget> {
    match (known_target_kind.unwrap_or_default(), heap_value) {
        (
            PerfCallTargetKind::Closure,
            HeapValue::Callable(CallableValue::Closure {
                function_index,
                captures,
            }),
        )
        | (
            PerfCallTargetKind::Unknown,
            HeapValue::Callable(CallableValue::Closure {
                function_index,
                captures,
            }),
        ) => Ok(CallableTarget::Closure {
            function_index: *function_index,
            captures: Arc::clone(captures),
        }),
        (PerfCallTargetKind::Native, HeapValue::Callable(CallableValue::RuntimeNative { arity, function, .. }))
        | (PerfCallTargetKind::Unknown, HeapValue::Callable(CallableValue::RuntimeNative { arity, function, .. })) => {
            Ok(CallableTarget::RuntimeNative {
                arity: *arity,
                function: function.clone(),
            })
        }
        (PerfCallTargetKind::Runtime, HeapValue::Callable(CallableValue::Runtime(function)))
        | (PerfCallTargetKind::Unknown, HeapValue::Callable(CallableValue::Runtime(function))) => {
            Ok(CallableTarget::Runtime(Arc::clone(function)))
        }
        (_, HeapValue::Callable(_)) => bail!("{error}"),
        _ => bail!("{error}"),
    }
}

impl Executor {
    pub(super) fn observe_call_target_kind(&self, callee: u16) -> PerfCallTargetKind {
        let Ok(callee) = u8::try_from(callee) else {
            return PerfCallTargetKind::Unknown;
        };
        let Ok(RuntimeVal::Obj(handle)) = self.read(callee) else {
            return PerfCallTargetKind::Unknown;
        };
        match self.state.heap.get(*handle) {
            Some(HeapValue::Callable(CallableValue::Closure { .. })) => PerfCallTargetKind::Closure,
            Some(HeapValue::Callable(CallableValue::RuntimeNative { .. })) => PerfCallTargetKind::Native,
            Some(HeapValue::Callable(CallableValue::Runtime(_))) => PerfCallTargetKind::Runtime,
            _ => PerfCallTargetKind::Unknown,
        }
    }

    pub(super) fn handle_call_error(&mut self, error: anyhow::Error) -> Result<RuntimeVal> {
        if let Some(raise) = error.downcast_ref::<super::LanguageRaise>() {
            if let Err(error) = self.handle_language_raise(raise) {
                self.safepoint()?;
                return Err(error);
            }
            Ok(RuntimeVal::Nil)
        } else {
            self.safepoint()?;
            Err(error)
        }
    }

    #[cold]
    pub(super) fn call_function(
        &mut self,
        module: Option<&Module>,
        window: CallWindow,
        known_target_kind: Option<PerfCallTargetKind>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<CallOutcome> {
        let module = module.ok_or_else(|| anyhow!("Call requires Module execution"))?;
        let callee = *self
            .read(u8::try_from(window.callee.as_usize()).map_err(|_| anyhow!("call callee register overflow"))?)?;
        let RuntimeVal::Obj(handle) = callee else {
            bail!("{} is not a function", self.runtime_value_display_string(&callee)?);
        };
        let callable = callable_target(
            known_target_kind,
            self.state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            "Call callee is not callable",
        )?;

        match callable {
            CallableTarget::Closure {
                function_index,
                captures,
            } => {
                let function = checked_positional_function(module, function_index, window.arg_count)?;
                self.push_call_frame(function_index, function, captures, window)?;
                Ok(CallOutcome::Pushed(function_index))
            }
            CallableTarget::RuntimeNative { arity, function } => {
                if arity != NativeEntry::VARIADIC && arity != window.arg_count {
                    bail!(
                        "Native expects {} positional arguments, got {}",
                        arity,
                        window.arg_count
                    );
                }
                let native = NativeEntry {
                    name: "<runtime-native>".to_string(),
                    arity,
                    function,
                };
                let args = self.call_args_stack_range(window)?;
                let result = if native.function.requires_full_state() {
                    let args = move_inline_native_args_from_stack(&native, &mut self.state.stack, args)?;
                    call_native_entry(
                        &native,
                        args.as_slice(),
                        &mut self.state,
                        Some(module),
                        self.shared_module.clone(),
                        ctx.as_deref_mut(),
                    )
                } else {
                    let state = &mut self.state;
                    call_native_entry_parts(
                        &native,
                        NativeArgs::new(&state.stack[args]),
                        &mut state.heap,
                        &state.globals,
                        Some(module),
                        self.shared_module.clone(),
                        ctx.as_deref_mut(),
                    )
                };
                self.sync_heap_gc_threshold();
                result
                    .or_else(|error| self.handle_call_error(error))
                    .map(CallOutcome::Value)
            }
            CallableTarget::Runtime(function) => {
                let args = self.call_args_stack_range(window)?;
                let result = runtime_callable::call_runtime_callable_runtime(
                    function.as_ref(),
                    &self.state.stack[args],
                    &mut self.state.heap,
                    ctx.as_deref_mut(),
                );
                result
                    .or_else(|error| self.handle_call_error(error))
                    .map(CallOutcome::Value)
            }
        }
    }

    #[cold]
    pub(super) fn call_direct_function(
        &mut self,
        module: Option<&Module>,
        function_index: u32,
        window: CallWindow,
    ) -> Result<()> {
        let module = module.ok_or_else(|| anyhow!("CallDirect requires Module execution"))?;
        let captures = Arc::clone(&self.empty_captures);
        let function = checked_positional_function(module, function_index, window.arg_count)?;
        self.push_call_frame(function_index, function, captures, window)
    }

    /// Push a suspended caller `CallFrame` and switch the executor's "current
    /// frame" fields to the callee (plan M2.5 sub-step ①). Replaces what the
    /// old recursive `call_closure_stack_args` did before its nested
    /// `run_function_inner` call — the callee's code doesn't run here; the
    /// dispatch trampoline in `exec.rs` picks it up on the next loop turn.
    fn push_call_frame(
        &mut self,
        function_index: u32,
        function: &Function,
        captures: Arc<Vec<RuntimeVal>>,
        window: CallWindow,
    ) -> Result<()> {
        let arg_range = self.call_args_stack_range(window)?;
        // Checked before any state mutation: a failure here (call depth cap)
        // must leave the caller's frame untouched, exactly like the old
        // `self.enter_lk_call().and_then(...)` short-circuit.
        self.enter_lk_call()?;
        let new_base = self.state.stack_top;
        let new_top = new_base + function.register_count as usize;
        if self.state.stack.len() < new_top {
            self.state.stack.resize(new_top, RuntimeVal::Nil);
        }
        let reg_count = function.register_count as usize;
        self.state.stack[new_base..new_base + reg_count].fill(RuntimeVal::Nil);
        let param_count = window.arg_count as usize;
        for i in 0..param_count {
            let src = arg_range.start + i;
            let dst = new_base + i;
            self.state.stack[dst] = core::mem::take(&mut self.state.stack[src]);
        }
        self.frames.push(CallFrame {
            function_index: self.current_function_index,
            pc: self.pc,
            frame_base: self.frame_base,
            register_count: self.register_count,
            captures: core::mem::replace(&mut self.captures, captures),
            handler_depth: self.handler_stack.len(),
            window,
            named_count: 0,
            stack_top: self.state.stack_top,
        });
        self.current_function_index = function_index;
        self.frame_base = new_base;
        self.register_count = function.register_count;
        self.state.stack_top = new_top;
        self.pc = 0;
        Ok(())
    }

    /// Named-argument counterpart of `push_call_frame` (plan M2.5 sub-step
    /// ②): replaces what the old recursive `call_closure_named_stack_args`
    /// did before its nested `run_function_inner` call.
    pub(super) fn push_call_frame_named(
        &mut self,
        function_index: u32,
        function: &Function,
        captures: Arc<Vec<RuntimeVal>>,
        window: CallWindow,
        named_count: u16,
    ) -> Result<()> {
        let positional = self.call_args_stack_range(window)?;
        let named_start = positional.end;
        // Checked before any state mutation, same rationale as `push_call_frame`.
        self.enter_lk_call()?;
        let new_base = self.state.stack_top;
        let new_top = new_base + function.register_count as usize;
        if self.state.stack.len() < new_top {
            self.state.stack.resize(new_top, RuntimeVal::Nil);
        }
        let (caller_stack, callee_stack) = self.state.stack.split_at_mut(new_base);
        let callee_frame = &mut callee_stack[..function.register_count as usize];
        callee_frame.fill(RuntimeVal::Nil);
        move_named_args_to_frame_from_stack(
            function,
            positional,
            caller_stack,
            named_start,
            named_count,
            &self.state.heap,
            callee_frame,
        )?;
        self.frames.push(CallFrame {
            function_index: self.current_function_index,
            pc: self.pc,
            frame_base: self.frame_base,
            register_count: self.register_count,
            captures: core::mem::replace(&mut self.captures, captures),
            handler_depth: self.handler_stack.len(),
            window,
            named_count,
            stack_top: self.state.stack_top,
        });
        self.current_function_index = function_index;
        self.frame_base = new_base;
        self.register_count = function.register_count;
        self.state.stack_top = new_top;
        self.pc = 0;
        Ok(())
    }
}

/// Record a call frame for an error that is propagating out of `function`
/// (uncaught here). Runs only on the error path — successful calls never touch
/// the traceback, so this is zero-cost for normal execution. Anonymous
/// functions (no `debug_name`) are skipped. Reuses the `VmContext` call-stack;
/// the top level formats it via `call_stack_report`, and `pcall` clears it when
/// it catches (plan M2.2 traceback). Also used by the flattened unwind loop
/// in `exec.rs` (`unwind_flat_run`, plan M2.5 sub-step ①).
pub(super) fn push_traceback_frame(ctx: &mut Option<&mut VmContext>, function: &Function) {
    if let Some(ctx) = ctx.as_deref_mut()
        && let Some(name) = function.debug_name.as_ref()
    {
        ctx.push_call_frame(Arc::clone(name), None::<Arc<str>>);
    }
}

fn checked_positional_function(module: &Module, function_index: u32, arg_count: u16) -> Result<&Function> {
    let function = module
        .functions
        .get(function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
    if function.param_count != arg_count {
        bail!(
            "Function expects {} positional arguments, got {}",
            function.param_count,
            arg_count
        );
    }
    Ok(function)
}
