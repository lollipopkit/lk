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
        TypedList::OwnedRuntime(_) => {
            bail!("OwnedRuntime list cannot appear in exec32 active heap — bridge conversion missing");
        }
    }
    Ok(())
}

pub(super) fn call_native_entry(
    native: &NativeEntry32,
    args: &[RuntimeVal],
    named: &[(Arc<str>, RuntimeVal)],
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let native_args = NativeArgs32::new_with_named(args, named);
    let result = match &native.function {
        NativeFunction32::Plain(function)
        | NativeFunction32::Context(function)
        | NativeFunction32::FullState(function) => {
            let mut runtime = NativeRuntime32::new(state, ctx, module);
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
        }
    }
}

pub(super) fn inline_native_args_from_stack(
    native: &NativeEntry32,
    stack: &[RuntimeVal],
    args: Range<usize>,
) -> Result<InlineNativeArgs32> {
    let Some(values) = stack.get(args.clone()) else {
        bail!("{} argument window out of bounds", native.name);
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
        len => bail!(
            "{} FullState native arity {} exceeds inline argument buffer",
            native.name,
            len
        ),
    })
}

pub(super) fn call_native_entry_parts(
    native: &NativeEntry32,
    args: NativeArgs32<'_>,
    named: &[(Arc<str>, RuntimeVal)],
    heap: &mut HeapStore,
    globals: &[RuntimeVal],
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let native_args = if named.is_empty() {
        args
    } else {
        NativeArgs32::new_with_named(args.as_slice(), named)
    };
    let result = match &native.function {
        NativeFunction32::Plain(function) | NativeFunction32::Context(function) => {
            let mut runtime = NativeRuntime32::from_parts(heap, globals, ctx, module);
            function(native_args, &mut runtime)
        }
        NativeFunction32::FullState(_) => {
            bail!("{} requires full runtime state", native.name);
        }
        NativeFunction32::RuntimeCallable(function) => {
            if named.is_empty() {
                super::runtime_callable::call_runtime_callable32_runtime(function.as_ref(), native_args, heap, ctx)
            } else {
                super::runtime_callable::call_runtime_callable32_runtime_named(
                    function.as_ref(),
                    native_args,
                    named,
                    heap,
                    ctx,
                )
            }
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
