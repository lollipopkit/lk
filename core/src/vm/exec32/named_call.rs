use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapValue, RuntimeVal},
    vm::{CallWindow32, Function32, Module32, NativeArgs32, NativeEntry32, VmContext},
};

use super::{Executor32, runtime_callable, support::call_native_entry_parts};

pub(super) fn order_named_args32_from_slice(
    function: &Function32,
    positional: &[RuntimeVal],
    named: &[(Arc<str>, RuntimeVal)],
) -> Result<Vec<RuntimeVal>> {
    if named.is_empty() {
        if function.param_count != positional.len() as u16 {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                positional.len()
            );
        }
        return Ok(positional.to_vec());
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

    let mut ordered = positional.to_vec();
    ordered.resize(function.param_count as usize, RuntimeVal::Nil);
    let mut seen = vec![false; function.param_count as usize - positional_count];
    for (name, value) in named {
        let Some(offset) = function.param_names[positional_count..]
            .iter()
            .position(|param| param.as_ref() == name.as_ref())
        else {
            bail!("unknown named argument `{name}`");
        };
        if std::mem::replace(&mut seen[offset], true) {
            bail!("duplicate named argument `{name}`");
        }
        ordered[positional_count + offset] = value.clone();
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(ordered)
}

impl Executor32 {
    pub(super) fn call_function_named(
        &mut self,
        module: Option<&Module32>,
        window: CallWindow32,
        named_count: u16,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<RuntimeVal> {
        let module = module.ok_or_else(|| anyhow!("CallNamed requires Module32 execution"))?;
        let callee = self
            .read(u8::try_from(window.callee.as_usize()).map_err(|_| anyhow!("call callee register overflow"))?)?
            .clone();
        let RuntimeVal::Obj(handle) = callee else {
            bail!("CallNamed callee is not callable");
        };
        let callable = match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Callable(callable) => callable.clone(),
            _ => bail!("CallNamed callee is not callable"),
        };
        match callable {
            CallableValue::Native { function_index, arity } => {
                self.call_native_named(module, function_index, arity, window, named_count, ctx)
            }
            CallableValue::RuntimeNative32 { arity, function } => {
                if arity != NativeEntry32::VARIADIC && arity != window.arg_count {
                    bail!(
                        "Native expects {} positional arguments, got {}",
                        arity,
                        window.arg_count
                    );
                }
                let named = self.read_named_call_args(window, named_count)?;
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
                    &named,
                    &mut state.heap,
                    &state.globals,
                    Some(module),
                    ctx.as_deref_mut(),
                );
                result.or_else(|error| self.handle_call_error(error))
            }
            CallableValue::Closure {
                function_index,
                captures,
            } => {
                let function = module
                    .functions
                    .get(function_index as usize)
                    .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
                let named = self.read_named_call_args(window, named_count)?;
                let positional = self.call_args_stack_range(window)?;
                let args = order_named_args32_from_slice(function, &self.state.stack[positional], &named)?;
                self.call_closure_args(module, function_index, captures, args.into_iter(), ctx)
            }
            CallableValue::Runtime32(function) => {
                let named = self.read_named_call_args(window, named_count)?;
                let args = self.call_args_stack_range(window)?;
                let result = runtime_callable::call_runtime_callable32_runtime_named(
                    function.as_ref(),
                    NativeArgs32::new(&self.state.stack[args]),
                    &named,
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

    fn call_native_named(
        &mut self,
        module: &Module32,
        native_index: u32,
        arity: u16,
        window: CallWindow32,
        named_count: u16,
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
        let named = self.read_named_call_args(window, named_count)?;
        let args = self.call_args_stack_range(window)?;
        let state = &mut self.state;
        let result = call_native_entry_parts(
            native,
            NativeArgs32::new(&state.stack[args]),
            &named,
            &mut state.heap,
            &state.globals,
            Some(module),
            ctx.as_deref_mut(),
        );
        result.or_else(|error| self.handle_call_error(error))
    }

    fn read_named_call_args(&self, window: CallWindow32, named_count: u16) -> Result<Vec<(Arc<str>, RuntimeVal)>> {
        let mut named = Vec::with_capacity(named_count as usize);
        let mut index = window.callee.as_usize() as u16 + 1 + window.arg_count;
        for _ in 0..named_count {
            let name = self.read(u8::try_from(index).map_err(|_| anyhow!("register {} exceeds u8", index))?)?;
            let name = match name {
                RuntimeVal::ShortStr(value) => Arc::<str>::from(value.as_str()),
                RuntimeVal::Obj(handle) => match self
                    .state
                    .heap
                    .get(*handle)
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                {
                    HeapValue::String(value) => value.clone(),
                    _ => bail!("CallNamed argument name must be a string"),
                },
                _ => bail!("CallNamed argument name must be a string"),
            };
            let value = self
                .read(u8::try_from(index + 1).map_err(|_| anyhow!("register {} exceeds u8", index + 1))?)?
                .clone();
            named.push((name, value));
            index += 2;
        }
        Ok(named)
    }
}
