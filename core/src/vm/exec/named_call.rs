#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use core::ops::Range;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{HeapStore, HeapValue, RuntimeVal},
    vm::{CallWindow, Function, Module, NativeArgs, NativeEntry, VmContext, analysis::PerfCallTargetKind},
};

use super::{
    Executor,
    call::{CallOutcome, CallableTarget, callable_target},
    runtime_callable,
    support::{
        call_native_entry_parts_with_args, call_native_entry_with_args, move_inline_native_args_from_stack,
        move_inline_native_slots_from_stack,
    },
};

pub(super) fn move_named_args_to_frame_from_stack(
    function: &Function,
    positional: Range<usize>,
    caller_stack: &mut [RuntimeVal],
    named_start: usize,
    named_count: u16,
    heap: &HeapStore,
    frame: &mut [RuntimeVal],
) -> Result<()> {
    if frame.len() < function.param_count as usize {
        bail!(
            "callee frame has {} slots, function requires {} params",
            frame.len(),
            function.param_count
        );
    }

    if named_count == 0 {
        if function.param_count != positional.len() as u16 {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                positional.len()
            );
        }
        move_range_into_frame(caller_stack, positional, &mut frame[..function.param_count as usize])?;
        return Ok(());
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

    move_range_into_frame(caller_stack, positional, &mut frame[..positional_count])?;
    let mut seen = vec![false; function.param_count as usize - positional_count];
    let named_end = named_start + named_count as usize * 2;
    if named_end > caller_stack.len() {
        bail!("CallNamed argument window {}..{} out of bounds", named_start, named_end);
    }
    for pair_start in (named_start..named_end).step_by(2) {
        let offset = {
            let name = call_named_arg_name(&caller_stack[pair_start], heap)?;
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
        caller_stack[pair_start] = RuntimeVal::Nil;
        frame[positional_count + offset] = core::mem::take(&mut caller_stack[pair_start + 1]);
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

fn move_range_into_frame(caller_stack: &mut [RuntimeVal], range: Range<usize>, frame: &mut [RuntimeVal]) -> Result<()> {
    if range.end > caller_stack.len() {
        bail!("call argument window {}..{} out of bounds", range.start, range.end);
    }
    for (slot, value_index) in frame.iter_mut().zip(range) {
        *slot = core::mem::take(&mut caller_stack[value_index]);
    }
    Ok(())
}

pub(crate) fn call_named_arg_name<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Result<&'a str> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(value.as_str()),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(value.as_ref()),
            _ => bail!("CallNamed argument name must be a string"),
        },
        _ => bail!("CallNamed argument name must be a string"),
    }
}

impl Executor {
    #[cold]
    pub(super) fn call_function_named(
        &mut self,
        module: Option<&Module>,
        window: CallWindow,
        named_count: u16,
        known_target_kind: Option<PerfCallTargetKind>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<CallOutcome> {
        let module = module.ok_or_else(|| anyhow!("CallNamed requires Module execution"))?;
        let callee = *self
            .read(u8::try_from(window.callee.as_usize()).map_err(|_| anyhow!("call callee register overflow"))?)?;
        let RuntimeVal::Obj(handle) = callee else {
            bail!("CallNamed callee is not callable");
        };
        let callable = callable_target(
            known_target_kind,
            self.state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            "CallNamed callee is not callable",
        )?;
        match callable {
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
                let named_start = args.end;
                let result = if native.function.requires_full_state() {
                    let args = move_inline_native_args_from_stack(&native, &mut self.state.stack, args.clone())?;
                    let named_end = named_start + named_count as usize * 2;
                    let named_stack = move_inline_native_slots_from_stack(
                        &native,
                        &mut self.state.stack,
                        named_start..named_end,
                        "named argument",
                    )?;
                    let native_args =
                        NativeArgs::new_with_named_stack(args.as_slice(), named_stack.as_slice(), 0, named_count);
                    call_native_entry_with_args(
                        &native,
                        native_args,
                        &mut self.state,
                        Some(module),
                        self.shared_module.clone(),
                        ctx.as_deref_mut(),
                    )
                } else {
                    let native_args = NativeArgs::new_with_named_stack(
                        &self.state.stack[args],
                        &self.state.stack,
                        named_start,
                        named_count,
                    );
                    call_native_entry_parts_with_args(
                        &native,
                        native_args,
                        &mut self.state.heap,
                        &self.state.globals,
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
            CallableTarget::Closure {
                function_index,
                captures,
            } => {
                let function = module
                    .functions
                    .get(function_index as usize)
                    .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))?;
                self.push_call_frame_named(function_index, function, captures, window, named_count)?;
                Ok(CallOutcome::Pushed(function_index))
            }
            CallableTarget::Runtime(function) => {
                let args = self.call_args_stack_range(window)?;
                let named_start = args.end;
                let result = runtime_callable::call_runtime_callable_runtime_named_stack(
                    function.as_ref(),
                    &self.state.stack[args],
                    &self.state.stack,
                    named_start,
                    named_count,
                    &mut self.state.heap,
                    ctx.as_deref_mut(),
                );
                result
                    .or_else(|error| self.handle_call_error(error))
                    .map(CallOutcome::Value)
            }
        }
    }
}
