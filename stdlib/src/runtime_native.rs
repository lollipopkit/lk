use anyhow::{Result, anyhow};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeVal, ShortStr, de, val_to_runtime_val},
    vm::{NativeArgs32, NativeRuntime32},
};
use std::sync::Arc;

pub(crate) fn parse_format32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
    name: &str,
    format: de::Format,
) -> Result<RuntimeVal> {
    if args.len() != 1 {
        return Err(anyhow!("{name}(data) requires 1 argument"));
    }
    let data = runtime_string_arg(args.get(0).expect("checked arity"), &runtime.state.heap, name)?;
    let parsed = de::parse_with_format(data.as_ref(), Some(format))?;
    val_to_runtime_val(&parsed, runtime.heap_mut())
}

pub(crate) fn runtime_string_arg(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<Arc<str>> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(Arc::<str>::from(value.as_str())),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(value.clone()),
            other => Err(anyhow!("{name} expects string argument, got {}", other.type_name())),
        },
        other => Err(anyhow!("{name} expects string argument, got {:?}", other.kind())),
    }
}

pub(crate) fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}
