use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList},
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
    named: &[(String, RuntimeVal)],
    state: &mut RuntimeModuleState32,
    module: Option<&Module32>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let native_args = NativeArgs32::new_with_named(args, named);
    let result = match &native.function {
        NativeFunction32::Plain(function) | NativeFunction32::Context(function) => {
            let mut runtime = NativeRuntime32 { state, ctx, module };
            function(native_args, &mut runtime)
        }
        NativeFunction32::RuntimeCallable(function) => super::runtime_callable::call_runtime_callable32_runtime(
            function.as_ref(),
            native_args,
            &mut state.heap,
            ctx,
        ),
    };
    result.map_err(|err| anyhow!("native `{}` failed: {err}", native.name))
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
    }
}
