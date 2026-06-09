use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::metadata::StdlibModuleMetadata;

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct BytesModule;

impl BytesModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BytesModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for BytesModule {
    fn name(&self) -> &str {
        "bytes"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "from_list" => from_list, 1,
                plain "from_string" => from_string, 1,
                plain "len" => len, 1,
                plain "is_empty" => is_empty, 1,
                plain "get" => get, 2,
                plain "slice" => slice, NativeEntry::VARIADIC,
                plain "to_list" => to_list, 1,
                plain "to_string_utf8" => to_string_utf8, 1,
                plain "to_string_lossy" => to_string_lossy, 1,
                plain "concat" => concat, 2,
                plain "eq" => eq, 2,
            ],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    lk_stdlib_common::metadata::register_stdlib_module_metadata(metadata())?;
    registry.register_module("bytes", Box::new(BytesModule::new()))
}

pub fn metadata() -> StdlibModuleMetadata {
    lk_stdlib_common::stdlib_module_metadata!(bytes, [to_string_utf8 => String])
}

pub fn runtime_bytes_value(bytes: impl Into<Arc<[u8]>>, heap: &mut HeapStore) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::Bytes(bytes.into())))
}

pub fn runtime_bytes_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<[u8]>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects Bytes");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Bytes(value) => Ok(value.clone()),
        other => bail!("{context} expects Bytes, got {}", other.type_name()),
    }
}

pub fn runtime_bytes_or_string_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<[u8]>> {
    match runtime_bytes_arg(value, heap, context) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Ok(Arc::<[u8]>::from(runtime_string_arg(value, heap, context)?.as_bytes())),
    }
}

fn from_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.from_list()")?;
    let values = byte_list_arg(args.get(0).expect("checked arity"), runtime.heap(), "bytes.from_list()")?;
    Ok(runtime_bytes_value(values, runtime.heap_mut()))
}

fn from_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.from_string()")?;
    let value = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "bytes.from_string()",
    )?;
    Ok(runtime_bytes_value(
        Arc::<[u8]>::from(value.as_bytes()),
        runtime.heap_mut(),
    ))
}

fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.len()")?;
    let value = runtime_bytes_arg(args.get(0).expect("checked arity"), runtime.heap(), "bytes.len()")?;
    Ok(RuntimeVal::Int(value.len() as i64))
}

fn is_empty(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.is_empty()")?;
    let value = runtime_bytes_arg(args.get(0).expect("checked arity"), runtime.heap(), "bytes.is_empty()")?;
    Ok(RuntimeVal::Bool(value.is_empty()))
}

fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "bytes.get()")?;
    let values = args.as_slice();
    let bytes = runtime_bytes_arg(&values[0], runtime.heap(), "bytes.get()")?;
    let index = usize_arg(&values[1], "bytes.get() index")?;
    Ok(bytes
        .get(index)
        .copied()
        .map(|value| RuntimeVal::Int(value as i64))
        .unwrap_or(RuntimeVal::Nil))
}

fn slice(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.len() != 2 && args.len() != 3 {
        bail!("bytes.slice() expects 2 or 3 arguments: bytes, start[, end]");
    }
    let values = args.as_slice();
    let bytes = runtime_bytes_arg(&values[0], runtime.heap(), "bytes.slice()")?;
    let start = usize_arg(&values[1], "bytes.slice() start")?.min(bytes.len());
    let end = if let Some(value) = values.get(2) {
        usize_arg(value, "bytes.slice() end")?.min(bytes.len())
    } else {
        bytes.len()
    };
    if end < start {
        bail!("bytes.slice() end must be greater than or equal to start");
    }
    let slice = bytes[start..end].to_vec();
    Ok(runtime_bytes_value(slice, runtime.heap_mut()))
}

fn to_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.to_list()")?;
    let bytes = runtime_bytes_arg(args.get(0).expect("checked arity"), runtime.heap(), "bytes.to_list()")?;
    let list = TypedList::Int(bytes.iter().copied().map(i64::from).collect());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn to_string_utf8(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.to_string_utf8()")?;
    let bytes = runtime_bytes_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "bytes.to_string_utf8()",
    )?;
    let value = std::str::from_utf8(&bytes).map_err(|err| anyhow!("bytes are not valid UTF-8: {err}"))?;
    Ok(runtime_string_value(value, runtime.heap_mut()))
}

fn to_string_lossy(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 1, "bytes.to_string_lossy()")?;
    let bytes = runtime_bytes_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "bytes.to_string_lossy()",
    )?;
    Ok(runtime_string_value(
        &String::from_utf8_lossy(&bytes),
        runtime.heap_mut(),
    ))
}

fn concat(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "bytes.concat()")?;
    let values = args.as_slice();
    let left = runtime_bytes_arg(&values[0], runtime.heap(), "bytes.concat() first argument")?;
    let right = runtime_bytes_arg(&values[1], runtime.heap(), "bytes.concat() second argument")?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    out.extend_from_slice(&left);
    out.extend_from_slice(&right);
    Ok(runtime_bytes_value(out, runtime.heap_mut()))
}

fn eq(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::runtime_native::expect_arity(args, 2, "bytes.eq()")?;
    let values = args.as_slice();
    let left = runtime_bytes_arg(&values[0], runtime.heap(), "bytes.eq() first argument")?;
    let right = runtime_bytes_arg(&values[1], runtime.heap(), "bytes.eq() second argument")?;
    Ok(RuntimeVal::Bool(left == right))
}

fn byte_list_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Vec<u8>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list of bytes");
    };
    let list = match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(list) => list,
        other => bail!("{context} expects a list of bytes, got {}", other.type_name()),
    };
    match list {
        TypedList::Int(values) => values.iter().map(|value| checked_byte(*value, context)).collect(),
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| match value {
                RuntimeVal::Int(value) => checked_byte(*value, context),
                other => bail!("{context} expects Int items, got {:?}", other.kind()),
            })
            .collect(),
        _ => bail!("{context} expects Int items"),
    }
}

fn checked_byte(value: i64, context: &str) -> Result<u8> {
    u8::try_from(value).map_err(|_| anyhow!("{context} expects byte values in 0..=255, got {value}"))
}

fn usize_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    match value {
        RuntimeVal::Int(value) if *value >= 0 => Ok(*value as usize),
        other => bail!("{context} expects a non-negative integer, got {:?}", other.kind()),
    }
}
