use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapValue, RuntimeVal},
    vm::{CallWindow32, Module32, NativeArgs32, NativeEntry32, VmContext, analysis::PerfCallTargetKind},
};

use super::{
    Executor32,
    named_call::write_named_args32_to_frame_from_stack,
    runtime_callable,
    support::{call_native_entry, call_native_entry_parts, inline_native_args_from_stack},
};

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
        let callable = match (
            known_target_kind.unwrap_or_default(),
            self.state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
        ) {
            (
                PerfCallTargetKind::Closure,
                HeapValue::Callable(CallableValue::Closure {
                    function_index,
                    captures,
                }),
            ) => CallableValue::Closure {
                function_index: *function_index,
                captures: captures.clone(),
            },
            (PerfCallTargetKind::Native, HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function })) => {
                CallableValue::RuntimeNative32 {
                    arity: *arity,
                    function: function.clone(),
                }
            }
            (PerfCallTargetKind::Runtime, HeapValue::Callable(CallableValue::Runtime32(function))) => {
                CallableValue::Runtime32(function.clone())
            }
            (_, HeapValue::Callable(callable)) => callable.clone(),
            _ => bail!("Call callee is not callable"),
        };

        match callable {
            CallableValue::Closure {
                function_index,
                captures,
            } => self.call_closure_window(module, function_index, captures, window, ctx),
            CallableValue::RuntimeNative32 { arity, function } => {
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
                    let args = inline_native_args_from_stack(&native, &self.state.stack, args)?;
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
            CallableValue::Runtime32(function) => {
                let args = self.call_args_stack_range(window)?;
                let result = runtime_callable::call_runtime_callable32_runtime(
                    function.as_ref(),
                    NativeArgs32::new(&self.state.stack[args]),
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
        captures: Vec<RuntimeVal>,
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
        captures: Vec<RuntimeVal>,
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
        let new_base = self.state.stack_top;
        let new_top = new_base + function.register_count as usize;
        if self.state.stack.len() < new_top {
            self.state.stack.resize(new_top, RuntimeVal::Nil);
        }
        let (caller_stack, callee_stack) = self.state.stack.split_at_mut(new_base);
        let positional = &caller_stack[positional];
        let callee_frame = &mut callee_stack[..function.register_count as usize];
        callee_frame.fill(RuntimeVal::Nil);
        write_named_args32_to_frame_from_stack(
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
        let result = self.run_function_inner(function, Some(module), ctx);
        self.frame_base = saved_base;
        self.register_count = saved_register_count;
        self.state.stack_top = saved_top;
        self.pc = saved_pc;
        self.captures = saved_captures;
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
        captures: Vec<RuntimeVal>,
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
        let new_base = self.state.stack_top;
        let new_top = new_base + function.register_count as usize;
        if self.state.stack.len() < new_top {
            self.state.stack.resize(new_top, RuntimeVal::Nil);
        }
        let (caller_stack, callee_stack) = self.state.stack.split_at_mut(new_base);
        let args = &caller_stack[arg_range];
        let callee_frame = &mut callee_stack[..function.register_count as usize];
        callee_frame[..window.arg_count as usize].clone_from_slice(args);
        callee_frame[window.arg_count as usize..].fill(RuntimeVal::Nil);
        self.frame_base = new_base;
        self.register_count = function.register_count;
        self.state.stack_top = new_top;
        self.pc = 0;
        let result = self.run_function_inner(function, Some(module), ctx);
        self.frame_base = saved_base;
        self.register_count = saved_register_count;
        self.state.stack_top = saved_top;
        self.pc = saved_pc;
        self.captures = saved_captures;
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
