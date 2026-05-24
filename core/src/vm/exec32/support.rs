use std::{ops::Range, sync::Arc};

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{Module32, NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeModuleState32, VmContext},
};

pub(super) fn set_list_value(list: &mut TypedList, index: usize, value: RuntimeVal) -> Result<()> {
    match list {
        TypedList::Mixed(values) => {
            let Some(slot) = values.get_mut(index) else {
                bail!("list index {} out of bounds", index);
            };
            *slot = value;
        }
        TypedList::Int(values) => match value {
            RuntimeVal::Int(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
            }
            value => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                let mixed = copy_numeric_list_with_replacement(values, index, value, RuntimeVal::Int);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::Float(values) => match value {
            RuntimeVal::Float(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
            }
            value => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                let mixed = copy_numeric_list_with_replacement(values, index, value, RuntimeVal::Float);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::Bool(values) => match value {
            RuntimeVal::Bool(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
            }
            value => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                let mixed = copy_numeric_list_with_replacement(values, index, value, RuntimeVal::Bool);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::String(_) => bail!("internal error: typed string list write must be handled before mutable borrow"),
    }
    Ok(())
}

fn copy_numeric_list_with_replacement<T: Copy>(
    values: &[T],
    index: usize,
    value: RuntimeVal,
    wrap: impl Fn(T) -> RuntimeVal,
) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len());
    for value in &values[..index] {
        mixed.push(wrap(*value));
    }
    mixed.push(value);
    for value in &values[index + 1..] {
        mixed.push(wrap(*value));
    }
    mixed
}

pub(super) fn call_native_entry(
    native: &NativeEntry32,
    args: &[RuntimeVal],
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    shared_module: Option<Arc<Module32>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_native_entry_with_args(native, NativeArgs32::new(args), state, module, shared_module, ctx)
}

pub(super) fn call_native_entry_with_args(
    native: &NativeEntry32,
    native_args: NativeArgs32<'_>,
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    shared_module: Option<Arc<Module32>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let result = match &native.function {
        NativeFunction32::Plain(function)
        | NativeFunction32::Context(function)
        | NativeFunction32::FullState(function) => {
            let mut runtime = match shared_module {
                Some(module) => NativeRuntime32::new_with_shared_module(state, ctx, module),
                None => NativeRuntime32::new(state, ctx, module),
            };
            function(native_args, &mut runtime)
        }
    };
    map_native_error(native, result)
}

pub(super) enum InlineNativeArgs32 {
    Zero,
    One([RuntimeVal; 1]),
    Two([RuntimeVal; 2]),
    Three([RuntimeVal; 3]),
    Four([RuntimeVal; 4]),
    Five([RuntimeVal; 5]),
    Six([RuntimeVal; 6]),
    Seven([RuntimeVal; 7]),
    Eight([RuntimeVal; 8]),
}

impl InlineNativeArgs32 {
    #[inline]
    pub(super) fn as_slice(&self) -> &[RuntimeVal] {
        match self {
            Self::Zero => &[],
            Self::One(values) => values,
            Self::Two(values) => values,
            Self::Three(values) => values,
            Self::Four(values) => values,
            Self::Five(values) => values,
            Self::Six(values) => values,
            Self::Seven(values) => values,
            Self::Eight(values) => values,
        }
    }
}

pub(super) fn move_inline_native_args_from_stack(
    native: &NativeEntry32,
    stack: &mut [RuntimeVal],
    args: Range<usize>,
) -> Result<InlineNativeArgs32> {
    move_inline_native_slots_from_stack(native, stack, args, "argument")
}

pub(super) fn move_inline_native_slots_from_stack(
    native: &NativeEntry32,
    stack: &mut [RuntimeVal],
    slots: Range<usize>,
    label: &str,
) -> Result<InlineNativeArgs32> {
    if slots.end > stack.len() {
        bail!("{} {} window out of bounds", native.name, label);
    }
    Ok(match slots.len() {
        0 => InlineNativeArgs32::Zero,
        1 => InlineNativeArgs32::One([std::mem::take(&mut stack[slots.start])]),
        2 => InlineNativeArgs32::Two([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
        ]),
        3 => InlineNativeArgs32::Three([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
        ]),
        4 => InlineNativeArgs32::Four([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
        ]),
        5 => InlineNativeArgs32::Five([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
        ]),
        6 => InlineNativeArgs32::Six([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
            std::mem::take(&mut stack[slots.start + 5]),
        ]),
        7 => InlineNativeArgs32::Seven([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
            std::mem::take(&mut stack[slots.start + 5]),
            std::mem::take(&mut stack[slots.start + 6]),
        ]),
        8 => InlineNativeArgs32::Eight([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
            std::mem::take(&mut stack[slots.start + 5]),
            std::mem::take(&mut stack[slots.start + 6]),
            std::mem::take(&mut stack[slots.start + 7]),
        ]),
        len => bail!(
            "{} FullState native {} count {} exceeds inline buffer",
            native.name,
            label,
            len
        ),
    })
}

pub(super) fn call_native_entry_parts(
    native: &NativeEntry32,
    args: NativeArgs32<'_>,
    heap: &mut HeapStore,
    globals: &[RuntimeVal],
    module: Option<&Module32>,
    shared_module: Option<Arc<Module32>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_native_entry_parts_with_args(native, args, heap, globals, module, shared_module, ctx)
}

pub(super) fn call_native_entry_parts_with_args(
    native: &NativeEntry32,
    native_args: NativeArgs32<'_>,
    heap: &mut HeapStore,
    globals: &[RuntimeVal],
    module: Option<&Module32>,
    shared_module: Option<Arc<Module32>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let result = match &native.function {
        NativeFunction32::Plain(function) | NativeFunction32::Context(function) => {
            let mut runtime = match shared_module {
                Some(module) => NativeRuntime32::from_parts_with_shared_module(heap, globals, ctx, module),
                None => NativeRuntime32::from_parts(heap, globals, ctx, module),
            };
            function(native_args, &mut runtime)
        }
        NativeFunction32::FullState(_) => {
            bail!("{} requires full runtime state", native.name);
        }
    };
    map_native_error(native, result)
}

fn map_native_error(native: &NativeEntry32, result: Result<RuntimeVal>) -> Result<RuntimeVal> {
    result.map_err(|err| {
        if err.is::<super::LanguageRaise32>() {
            err
        } else {
            anyhow!("native `{}` failed: {err}", native.name)
        }
    })
}

pub(super) fn heap_kind(value: &HeapValue) -> &'static str {
    match value {
        HeapValue::String(_) => "String",
        HeapValue::List(_) => "List",
        HeapValue::Map(_) => "Map",
        HeapValue::Callable(_) => "Callable",
        HeapValue::Task(_) => "Task",
        HeapValue::Channel(_) => "Channel",
        HeapValue::Stream(_) => "Stream",
        HeapValue::StreamCursor(_) => "StreamCursor",
        HeapValue::Object(_) => "Object",
        HeapValue::UpvalCell(_) => "UpvalCell",
        HeapValue::ErrorVal(_) => "Error",
    }
}
