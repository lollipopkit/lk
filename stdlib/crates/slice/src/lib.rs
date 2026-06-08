use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::{HeapStore, HeapValue, RuntimeVal, SliceKind, SliceValue, TypedList},
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}
pub use lk_stdlib_common::typed_list_from_values;

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

/// Byte-oriented slices over lists and strings.
///
/// `slice.from_string()` and `slice.sub()` operate on byte offsets. String slices
/// may split a multibyte UTF-8 character; `slice.to_string()` validates the byte
/// range and returns an error when the selected range is not valid UTF-8.
#[derive(Debug)]
pub struct SliceModule;

impl SliceModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SliceModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for SliceModule {
    fn name(&self) -> &str {
        "slice"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("from_list", from_list, 1),
                RuntimeNativeExport::plain("from_string", from_string, 1),
                RuntimeNativeExport::plain("len", len, 1),
                RuntimeNativeExport::plain("is_empty", is_empty, 1),
                RuntimeNativeExport::plain("get", get, 2),
                RuntimeNativeExport::plain("sub", sub, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("to_list", to_list, 1),
                RuntimeNativeExport::plain("to_string", to_string, 1),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("slice", Box::new(SliceModule::new()))
}

fn from_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "slice.from_list()")?;
    let source = args.get(0).expect("checked arity").clone();
    let len = list_arg(&source, runtime.heap(), "slice.from_list()")?.len();
    Ok(slice_value(source, SliceKind::List, 0, len, runtime.heap_mut()))
}

fn from_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "slice.from_string()")?;
    let source = args.get(0).expect("checked arity").clone();
    let text = runtime_string_arg(&source, runtime.heap(), "slice.from_string()")?;
    Ok(slice_value(
        source,
        SliceKind::String,
        0,
        text.len(),
        runtime.heap_mut(),
    ))
}

fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "slice.len()")?;
    Ok(RuntimeVal::Int(
        slice_arg(args.get(0).expect("checked arity"), runtime.heap(), "slice.len()")?.len as i64,
    ))
}

fn is_empty(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "slice.is_empty()")?;
    Ok(RuntimeVal::Bool(
        slice_arg(args.get(0).expect("checked arity"), runtime.heap(), "slice.is_empty()")?.len == 0,
    ))
}

fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "slice.get()")?;
    let values = args.as_slice();
    let slice = slice_arg(&values[0], runtime.heap(), "slice.get()")?;
    let index = usize_arg(&values[1], "slice.get() index")?;
    if index >= slice.len {
        return Ok(RuntimeVal::Nil);
    }
    match slice.kind {
        SliceKind::List => {
            let item = {
                let list = list_arg(&slice.source, runtime.heap(), "slice.get() source")?;
                list_item(list, slice.start + index)
            };
            Ok(item
                .map(|item| item.into_runtime(runtime.heap_mut()))
                .unwrap_or(RuntimeVal::Nil))
        }
        SliceKind::String => {
            let text = runtime_string_arg(&slice.source, runtime.heap(), "slice.get() source")?;
            let Some(byte) = text.as_bytes().get(slice.start + index) else {
                return Ok(RuntimeVal::Nil);
            };
            Ok(RuntimeVal::Int(*byte as i64))
        }
    }
}

fn sub(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.len() != 2 && args.len() != 3 {
        bail!("slice.sub() expects 2 or 3 arguments: slice, start[, end]");
    }
    let values = args.as_slice();
    let slice = slice_arg(&values[0], runtime.heap(), "slice.sub()")?;
    let start = usize_arg(&values[1], "slice.sub() start")?.min(slice.len);
    let end = if let Some(end) = values.get(2) {
        usize_arg(end, "slice.sub() end")?.min(slice.len)
    } else {
        slice.len
    };
    let len = end.saturating_sub(start);
    Ok(slice_value(
        slice.source.clone(),
        slice.kind,
        slice.start + start,
        len,
        runtime.heap_mut(),
    ))
}

fn to_list(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "slice.to_list()")?;
    let slice = slice_arg(args.get(0).expect("checked arity"), runtime.heap(), "slice.to_list()")?;
    let values = match slice.kind {
        SliceKind::List => {
            let items = {
                let list = list_arg(&slice.source, runtime.heap(), "slice.to_list() source")?;
                (0..slice.len)
                    .filter_map(|index| list_item(list, slice.start + index))
                    .collect::<Vec<_>>()
            };
            items
                .into_iter()
                .map(|item| item.into_runtime(runtime.heap_mut()))
                .collect()
        }
        SliceKind::String => {
            let text = runtime_string_arg(&slice.source, runtime.heap(), "slice.to_list() source")?;
            text.as_bytes()[slice.start..slice.start + slice.len]
                .iter()
                .copied()
                .map(|byte| RuntimeVal::Int(byte as i64))
                .collect()
        }
    };
    let list = crate::typed_list_from_values(values, runtime.heap());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn to_string(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "slice.to_string()")?;
    let slice = slice_arg(args.get(0).expect("checked arity"), runtime.heap(), "slice.to_string()")?;
    match slice.kind {
        SliceKind::String => {
            let text = runtime_string_arg(&slice.source, runtime.heap(), "slice.to_string() source")?;
            let bytes = &text.as_bytes()[slice.start..slice.start + slice.len];
            let value =
                std::str::from_utf8(bytes).map_err(|_| anyhow!("slice.to_string() range is not valid UTF-8"))?;
            Ok(runtime_string_value(value, runtime.heap_mut()))
        }
        SliceKind::List => bail!("slice.to_string() expects a string slice"),
    }
}

fn slice_value(source: RuntimeVal, kind: SliceKind, start: usize, len: usize, heap: &mut HeapStore) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::Slice(Arc::new(SliceValue {
        source,
        kind,
        start,
        len,
    }))))
}

fn slice_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<SliceValue>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a Slice");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Slice(slice) => Ok(slice.clone()),
        other => bail!("{context} expects a Slice, got {}", other.type_name()),
    }
}

fn list_arg<'a>(value: &RuntimeVal, heap: &'a HeapStore, context: &str) -> Result<&'a TypedList> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(list) => Ok(list),
        other => bail!("{context} expects a list, got {}", other.type_name()),
    }
}

enum ListItem {
    Runtime(RuntimeVal),
    String(Arc<str>),
}

impl ListItem {
    fn into_runtime(self, heap: &mut HeapStore) -> RuntimeVal {
        match self {
            Self::Runtime(value) => value,
            Self::String(value) => runtime_string_value(&value, heap),
        }
    }
}

fn list_item(list: &TypedList, index: usize) -> Option<ListItem> {
    match list {
        TypedList::Mixed(values) => values.get(index).cloned().map(ListItem::Runtime),
        TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int).map(ListItem::Runtime),
        TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float).map(ListItem::Runtime),
        TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool).map(ListItem::Runtime),
        TypedList::String(values) => values.get(index).cloned().map(ListItem::String),
    }
}

fn usize_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    match value {
        RuntimeVal::Int(value) if *value >= 0 => Ok(*value as usize),
        other => bail!("{context} expects a non-negative integer, got {:?}", other.kind()),
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
