use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapValue, RuntimeVal},
    vm::{CallWindow32, Module32, NativeArgs32, NativeEntry32, RegisterIndex, VmContext},
};

use super::{Executor32, runtime_callable, support::call_native_entry};

impl Executor32 {
    pub(super) fn call_function(
        &mut self,
        module: Option<&Module32>,
        window: CallWindow32,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let module = module.ok_or_else(|| anyhow!("Call requires Module32 execution"))?;
        let callee = self
            .frame
            .read(window.callee)
            .cloned()
            .ok_or_else(|| anyhow!("call callee register {} out of bounds", window.callee.as_usize()))?;
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
                call_native_entry(
                    &native,
                    self.frame.call_args(window),
                    &[],
                    &mut self.state,
                    Some(module),
                    ctx.as_deref_mut(),
                )
            }
            CallableValue::Runtime32(function) => {
                let args = self.frame.call_args(window);
                runtime_callable::call_runtime_callable32_runtime(
                    function.as_ref(),
                    NativeArgs32::new(args),
                    &mut self.state.heap,
                    ctx.as_deref_mut(),
                )
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

        let mut callee = Executor32::new(function.register_count);
        self.frame.copy_call_args_to_frame(window, &mut callee.frame);
        callee.state = std::mem::take(&mut self.state);
        callee.captures = captures;

        match callee.run_function_inner(function, Some(module), ctx) {
            Ok(returns) => {
                let result = callee.finish(returns);
                self.state = result.state;
                Ok(result.returns.into_iter().next().unwrap_or(RuntimeVal::Nil))
            }
            Err(error) => {
                self.state = callee.state;
                Err(error)
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

        let mut callee = Executor32::new(function.register_count);
        for (index, value) in args.enumerate() {
            callee.frame.write(RegisterIndex::new(index as u16), value);
        }
        callee.state = std::mem::take(&mut self.state);
        callee.captures = captures;

        match callee.run_function_inner(function, Some(module), ctx) {
            Ok(returns) => {
                let result = callee.finish(returns);
                self.state = result.state;
                Ok(result.returns.into_iter().next().unwrap_or(RuntimeVal::Nil))
            }
            Err(error) => {
                self.state = callee.state;
                Err(error)
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
        let args = self.frame.call_args(window);
        call_native_entry(native, args, &[], &mut self.state, Some(module), ctx.as_deref_mut())
    }
}
