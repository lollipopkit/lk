use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapValue, RuntimeVal},
    vm::{CallWindow32, Module32, NativeArgs32, NativeEntry32, VmContext, analysis::PerfCallTargetKind},
};

use super::{
    Executor32,
    named_call::move_named_args32_to_frame_from_stack,
    runtime_callable,
    support::{call_native_entry, call_native_entry_parts, move_inline_native_args_from_stack},
};

pub(super) enum CallableTarget32 {
    Closure {
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
    },
    RuntimeNative32 {
        arity: u16,
        function: crate::vm::NativeFunction32,
    },
    Runtime32(Arc<crate::vm::RuntimeCallable32>),
}

pub(super) fn callable_target32(
    known_target_kind: Option<PerfCallTargetKind>,
    heap_value: &HeapValue,
    error: &'static str,
) -> Result<CallableTarget32> {
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
        ) => Ok(CallableTarget32::Closure {
            function_index: *function_index,
            captures: Arc::clone(captures),
        }),
        (PerfCallTargetKind::Native, HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function, .. }))
        | (PerfCallTargetKind::Unknown, HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function, .. })) => {
            Ok(CallableTarget32::RuntimeNative32 {
                arity: *arity,
                function: function.clone(),
            })
        }
        (PerfCallTargetKind::Runtime, HeapValue::Callable(CallableValue::Runtime32(function)))
        | (PerfCallTargetKind::Unknown, HeapValue::Callable(CallableValue::Runtime32(function))) => {
            Ok(CallableTarget32::Runtime32(Arc::clone(function)))
        }
        (_, HeapValue::Callable(_)) => bail!("{error}"),
        _ => bail!("{error}"),
    }
}

impl Executor32 {
    pub(super) fn observe_call_target_kind(&self, callee: u16) -> PerfCallTargetKind {
        let Ok(callee) = u8::try_from(callee) else {
            return PerfCallTargetKind::Unknown;
        };
        let Ok(RuntimeVal::Obj(handle)) = self.read(callee) else {
            return PerfCallTargetKind::Unknown;
        };
        match self.state.heap.get(*handle) {
            Some(HeapValue::Callable(CallableValue::Closure { .. })) => PerfCallTargetKind::Closure,
            Some(HeapValue::Callable(CallableValue::RuntimeNative32 { .. })) => PerfCallTargetKind::Native,
            Some(HeapValue::Callable(CallableValue::Runtime32(_))) => PerfCallTargetKind::Runtime,
            _ => PerfCallTargetKind::Unknown,
        }
    }

    pub(super) fn handle_call_error(&mut self, error: anyhow::Error) -> Result<RuntimeVal> {
        if let Some(raise) = error.downcast_ref::<super::LanguageRaise32>() {
            self.handle_language_raise(raise)?;
            Ok(RuntimeVal::Nil)
        } else {
            Err(error)
        }
    }

    pub(super) fn call_function(
        &mut self,
        module: Option<&Module32>,
        window: CallWindow32,
        known_target_kind: Option<PerfCallTargetKind>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let module = module.ok_or_else(|| anyhow!("Call requires Module32 execution"))?;
        let callee = self
            .read(u8::try_from(window.callee.as_usize()).map_err(|_| anyhow!("call callee register overflow"))?)?
            .clone();
        let RuntimeVal::Obj(handle) = callee else {
            bail!("{} is not a function", self.runtime_value_display_string(&callee)?);
        };
        let callable = callable_target32(
            known_target_kind,
            self.state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            "Call callee is not callable",
        )?;

        match callable {
            CallableTarget32::Closure {
                function_index,
                captures,
            } => self.call_closure_window(module, function_index, captures, window, ctx),
            CallableTarget32::RuntimeNative32 { arity, function } => {
                if arity != NativeEntry32::VARIADIC && arity != window.arg_count {
                    bail!(
                        "Native expects {} positional arguments, got {}",
                        arity,
                        window.arg_count
                    );
                }
                let native = NativeEntry32 {
                    name: "<runtime-native32>".to_string(),
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
                        NativeArgs32::new(&state.stack[args]),
                        &mut state.heap,
                        &state.globals,
                        Some(module),
                        self.shared_module.clone(),
                        ctx.as_deref_mut(),
                    )
                };
                result.or_else(|error| self.handle_call_error(error))
            }
            CallableTarget32::Runtime32(function) => {
                let args = self.call_args_stack_range(window)?;
                let result = runtime_callable::call_runtime_callable32_runtime(
                    function.as_ref(),
                    &self.state.stack[args],
                    &mut self.state.heap,
                    ctx.as_deref_mut(),
                );
                result.or_else(|error| self.handle_call_error(error))
            }
        }
    }

    pub(super) fn call_closure_window(
        &mut self,
        module: &Module32,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        window: CallWindow32,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
        if function.param_count != window.arg_count {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                window.arg_count
            );
        }

        self.call_closure_stack_args(module, function_index, captures, window, ctx)
    }

    pub(super) fn call_closure_named_stack_args(
        &mut self,
        module: &Module32,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        window: CallWindow32,
        named_count: u16,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;

        let positional = self.call_args_stack_range(window)?;
        let named_start = positional.end;
        let saved_base = self.frame_base;
        let saved_top = self.state.stack_top;
        let saved_pc = self.pc;
        let saved_captures = std::mem::replace(&mut self.captures, captures);
        let saved_register_count = self.register_count;
        let saved_handler_depth = self.handler_stack.len();
        let result = (|| {
            let new_base = self.state.stack_top;
            let new_top = new_base + function.register_count as usize;
            if self.state.stack.len() < new_top {
                self.state.stack.resize(new_top, RuntimeVal::Nil);
            }
            let (caller_stack, callee_stack) = self.state.stack.split_at_mut(new_base);
            let callee_frame = &mut callee_stack[..function.register_count as usize];
            callee_frame.fill(RuntimeVal::Nil);
            move_named_args32_to_frame_from_stack(
                function,
                positional,
                caller_stack,
                named_start,
                named_count,
                &self.state.heap,
                callee_frame,
            )?;
            self.frame_base = new_base;
            self.register_count = function.register_count;
            self.state.stack_top = new_top;
            self.pc = 0;
            self.run_function_inner(function, Some(module), ctx)
        })();
        self.frame_base = saved_base;
        self.register_count = saved_register_count;
        self.state.stack_top = saved_top;
        self.pc = saved_pc;
        self.captures = saved_captures;
        self.handler_stack.truncate(saved_handler_depth);
        match result {
            Ok(returns) => Ok(returns.into_first()),
            Err(error) => {
                if let Some(raise) = error.downcast_ref::<super::LanguageRaise32>() {
                    self.handle_language_raise(raise)?;
                    Ok(RuntimeVal::Nil)
                } else {
                    Err(error)
                }
            }
        }
    }

    fn call_closure_stack_args(
        &mut self,
        module: &Module32,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        window: CallWindow32,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
        if function.param_count != window.arg_count {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                window.arg_count
            );
        }

        let arg_range = self.call_args_stack_range(window)?;
        let saved_base = self.frame_base;
        let saved_top = self.state.stack_top;
        let saved_pc = self.pc;
        let saved_captures = std::mem::replace(&mut self.captures, captures);
        let saved_register_count = self.register_count;
        let saved_handler_depth = self.handler_stack.len();
        let result = (|| {
            let new_base = self.state.stack_top;
            let new_top = new_base + function.register_count as usize;
            if self.state.stack.len() < new_top {
                self.state.stack.resize(new_top, RuntimeVal::Nil);
            }
            let (caller_stack, callee_stack) = self.state.stack.split_at_mut(new_base);
            let callee_frame = &mut callee_stack[..function.register_count as usize];
            let param_count = window.arg_count as usize;
            callee_frame[..param_count].fill(RuntimeVal::Nil);
            if arg_range.end > caller_stack.len() {
                bail!("call args range {}..{} out of bounds", arg_range.start, arg_range.end);
            }
            for (slot, arg_index) in callee_frame[..param_count].iter_mut().zip(arg_range) {
                *slot = std::mem::take(&mut caller_stack[arg_index]);
            }
            callee_frame[param_count..].fill(RuntimeVal::Nil);
            self.frame_base = new_base;
            self.register_count = function.register_count;
            self.state.stack_top = new_top;
            self.pc = 0;
            self.run_function_inner(function, Some(module), ctx)
        })();
        self.frame_base = saved_base;
        self.register_count = saved_register_count;
        self.state.stack_top = saved_top;
        self.pc = saved_pc;
        self.captures = saved_captures;
        self.handler_stack.truncate(saved_handler_depth);
        match result {
            Ok(returns) => Ok(returns.into_first()),
            Err(error) => {
                if let Some(raise) = error.downcast_ref::<super::LanguageRaise32>() {
                    self.handle_language_raise(raise)?;
                    Ok(RuntimeVal::Nil)
                } else {
                    Err(error)
                }
            }
        }
    }
}
