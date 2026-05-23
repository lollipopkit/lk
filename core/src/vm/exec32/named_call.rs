use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{CallableValue, HeapStore, HeapValue, RuntimeVal, TypedMap},
    vm::{CallWindow32, Function32, Module32, NativeArgs32, NativeEntry32, VmContext},
};

use super::{
    Executor32, runtime_callable,
    support::{call_native_entry, call_native_entry_parts, inline_native_args_from_stack},
};

/// Write named arguments from a `&TypedMap` directly into a callee frame,
/// bypassing the intermediate `Vec<(Arc<str>, RuntimeVal)>`.
///
/// This is the direct-writer variant — call from a code path where you already
/// hold a `&TypedMap` reference (e.g. the dynamic `__lk_call_method_named` builtin).
/// All typed-map variants are handled without requiring `&mut HeapStore`.
pub(crate) fn write_named_args32_to_frame_from_typed_map(
    function: &Function32,
    positional: &[RuntimeVal],
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
    if positional.len() != positional_count {
        bail!(
            "Function expects {} positional arguments before named arguments, got {}",
            positional_count,
            positional.len()
        );
    }
    frame[..positional_count].clone_from_slice(positional);
    let named_slot_count = function.param_count as usize - positional_count;
    let mut seen = vec![false; named_slot_count];

    macro_rules! place_named {
        ($name:expr, $value:expr) => {{
            let name_str: &str = ($name).as_ref();
            let Some(offset) = function.param_names[positional_count..]
                .iter()
                .position(|p| p.as_ref() == name_str)
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

pub(super) fn write_named_args32_to_frame(
    function: &Function32,
    positional: &[RuntimeVal],
    named: &[(Arc<str>, RuntimeVal)],
    frame: &mut [RuntimeVal],
) -> Result<()> {
    if frame.len() < function.param_count as usize {
        bail!(
            "callee frame has {} slots, function requires {} params",
            frame.len(),
            function.param_count
        );
    }

    if named.is_empty() {
        if function.param_count != positional.len() as u16 {
            bail!(
                "Function expects {} positional arguments, got {}",
                function.param_count,
                positional.len()
            );
        }
        frame[..positional.len()].clone_from_slice(positional);
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

    frame[..positional_count].clone_from_slice(positional);
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
        frame[positional_count + offset] = value.clone();
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

pub(super) fn write_named_args32_to_frame_from_stack(
    function: &Function32,
    positional: &[RuntimeVal],
    caller_stack: &[RuntimeVal],
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
        frame[..positional.len()].clone_from_slice(positional);
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

    frame[..positional_count].clone_from_slice(positional);
    let mut seen = vec![false; function.param_count as usize - positional_count];
    let named_end = named_start + named_count as usize * 2;
    let Some(named_slots) = caller_stack.get(named_start..named_end) else {
        bail!("CallNamed argument window {}..{} out of bounds", named_start, named_end);
    };
    for pair in named_slots.chunks_exact(2) {
        let name = call_named_arg_name(&pair[0], heap)?;
        let Some(offset) = function.param_names[positional_count..]
            .iter()
            .position(|param| param.as_ref() == name)
        else {
            bail!("unknown named argument `{name}`");
        };
        if std::mem::replace(&mut seen[offset], true) {
            bail!("duplicate named argument `{name}`");
        }
        frame[positional_count + offset] = pair[1].clone();
    }

    if let Some(index) = seen.iter().position(|seen| !*seen) {
        bail!(
            "missing required named argument `{}`",
            function.param_names[positional_count + index]
        );
    }
    Ok(())
}

fn call_named_arg_name<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Result<&'a str> {
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
                let result = if native.function.requires_full_state() {
                    let args = inline_native_args_from_stack(&native, &self.state.stack, args)?;
                    call_native_entry(
                        &native,
                        args.as_slice(),
                        &named,
                        &mut self.state,
                        Some(module),
                        ctx.as_deref_mut(),
                    )
                } else {
                    let state = &mut self.state;
                    call_native_entry_parts(
                        &native,
                        NativeArgs32::new(&state.stack[args]),
                        &named,
                        &mut state.heap,
                        &state.globals,
                        Some(module),
                        ctx.as_deref_mut(),
                    )
                };
                result.or_else(|error| self.handle_call_error(error))
            }
            CallableValue::Closure {
                function_index,
                captures,
            } => self.call_closure_named_stack_args(module, function_index, captures, window, named_count, ctx),
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
        }
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
