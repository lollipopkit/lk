use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapValue, RuntimeVal},
    vm::{CallWindow32, Module32, NativeArgs32, NativeEntry32, VmContext},
};

use super::{Executor32, runtime_callable, support::call_native_entry_parts};

impl Executor32 {
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
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let module = module.ok_or_else(|| anyhow!("Call requires Module32 execution"))?;
        let callee = self
            .read(u8::try_from(window.callee.as_usize()).map_err(|_| anyhow!("call callee register overflow"))?)?
            .clone();
        let RuntimeVal::Obj(handle) = callee else {
            bail!("{} is not a function", self.runtime_value_display_string(&callee)?);
        };
        let callable = match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Callable(callable) => callable.clone(),
            _ => bail!("Call callee is not callable"),
        };

        match callable {
            CallableValue::Closure {
                function_index,
                captures,
            } => self.call_closure_window(module, function_index, captures, window, ctx),
            CallableValue::Native { function_index, arity } => {
                self.call_native(module, function_index, arity, window, ctx)
            }
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
                let state = &mut self.state;
                let result = call_native_entry_parts(
                    &native,
                    NativeArgs32::new(&state.stack[args]),
                    &[],
                    &mut state.heap,
                    &state.globals,
                    Some(module),
                    ctx.as_deref_mut(),
                );
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
            CallableValue::Aot(_) => {
                bail!("AOT callable is not implemented in Executor32 yet")
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
        for offset in 0..window.arg_count as usize {
            self.state.stack[new_base + offset] = self.state.stack[arg_range.start + offset].clone();
        }
        self.state.stack[new_base + window.arg_count as usize..new_top].fill(RuntimeVal::Nil);
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
            Ok(returns) => Ok(returns.into_iter().next().unwrap_or(RuntimeVal::Nil)),
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

    pub(super) fn call_closure_args(
        &mut self,
        module: &Module32,
        function_index: u32,
        captures: Vec<RuntimeVal>,
        args: impl ExactSizeIterator<Item = RuntimeVal>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
        if function.param_count != args.len() as u16 {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                args.len()
            );
        }

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
        self.state.stack[new_base..new_top].fill(RuntimeVal::Nil);
        for (index, value) in args.enumerate() {
            self.state.stack[new_base + index] = value;
        }
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
            Ok(returns) => Ok(returns.into_iter().next().unwrap_or(RuntimeVal::Nil)),
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

    fn call_native(
        &mut self,
        module: &Module32,
        native_index: u32,
        arity: u16,
        window: CallWindow32,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        if arity != NativeEntry32::VARIADIC && arity != window.arg_count {
            bail!(
                "Function expects {} positional arguments, got {}",
                arity,
                window.arg_count
            );
        }
        let native = module
            .natives
            .get(native_index as usize)
            .ok_or_else(|| anyhow!("native index {} out of bounds", native_index))?;
        let args = self.call_args_stack_range(window)?;
        let state = &mut self.state;
        let result = call_native_entry_parts(
            native,
            NativeArgs32::new(&state.stack[args]),
            &[],
            &mut state.heap,
            &state.globals,
            Some(module),
            ctx.as_deref_mut(),
        );
        result.or_else(|error| self.handle_call_error(error))
    }
}
