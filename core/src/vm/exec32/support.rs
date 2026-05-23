use std::{collections::BTreeMap, ops::Range, sync::Arc};

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList},
    vm::{Module32, NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeModuleState32, VmContext},
};

pub(super) fn string_key(key: &RuntimeMapKey) -> Option<&str> {
    key.as_str()
}

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
                let mut mixed = values.iter().copied().map(RuntimeVal::Int).collect::<Vec<_>>();
                let Some(slot) = mixed.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
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
                let mut mixed = values.iter().copied().map(RuntimeVal::Float).collect::<Vec<_>>();
                let Some(slot) = mixed.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
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
                let mut mixed = values.iter().copied().map(RuntimeVal::Bool).collect::<Vec<_>>();
                let Some(slot) = mixed.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::String(_) => bail!("internal error: typed string list write must be handled before mutable borrow"),
    }
    Ok(())
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
        NativeFunction32::RuntimeCallable(function) => super::runtime_callable::call_runtime_callable32_runtime(
            function.as_ref(),
            native_args,
            &mut state.heap,
            ctx,
        ),
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

pub(super) fn inline_native_args_from_stack(
    native: &NativeEntry32,
    stack: &[RuntimeVal],
    args: Range<usize>,
) -> Result<InlineNativeArgs32> {
    inline_native_slots_from_stack(native, stack, args, "argument")
}

pub(super) fn inline_native_slots_from_stack(
    native: &NativeEntry32,
    stack: &[RuntimeVal],
    slots: Range<usize>,
    label: &str,
) -> Result<InlineNativeArgs32> {
    let Some(values) = stack.get(slots.clone()) else {
        bail!("{} {} window out of bounds", native.name, label);
    };
    Ok(match values.len() {
        0 => InlineNativeArgs32::Zero,
        1 => InlineNativeArgs32::One([values[0].clone()]),
        2 => InlineNativeArgs32::Two([values[0].clone(), values[1].clone()]),
        3 => InlineNativeArgs32::Three([values[0].clone(), values[1].clone(), values[2].clone()]),
        4 => InlineNativeArgs32::Four([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
        ]),
        5 => InlineNativeArgs32::Five([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
            values[4].clone(),
        ]),
        6 => InlineNativeArgs32::Six([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
            values[4].clone(),
            values[5].clone(),
        ]),
        7 => InlineNativeArgs32::Seven([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
            values[4].clone(),
            values[5].clone(),
            values[6].clone(),
        ]),
        8 => InlineNativeArgs32::Eight([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
            values[4].clone(),
            values[5].clone(),
            values[6].clone(),
            values[7].clone(),
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
        NativeFunction32::RuntimeCallable(function) => {
            super::runtime_callable::call_runtime_callable32_runtime(function.as_ref(), native_args, heap, ctx)
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

pub(super) fn checked_u8_count(count: u16) -> Result<u8> {
    u8::try_from(count).map_err(|_| anyhow!("capture count {} exceeds u8 encoding", count))
}

pub(super) fn remove_runtime_entry(entries: &mut BTreeMap<RuntimeMapKey, RuntimeVal>, key: &RuntimeMapKey) {
    if entries.remove(key).is_some() {
        return;
    }
    let Some(key) = string_key(key) else {
        return;
    };
    entries.remove(&RuntimeMapKey::String(Arc::<str>::from(key)));
    if let Some(short) = ShortStr::new(key) {
        entries.remove(&RuntimeMapKey::ShortStr(short));
    }
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
