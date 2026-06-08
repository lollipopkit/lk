use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::{CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap},
    vm::{
        NativeArgs, NativeEntry, NativeFunction, NativeRuntime, RuntimeExport, call_runtime_callable_runtime,
        call_runtime_value_runtime,
    },
};

#[derive(Debug)]
pub struct IterModule;

impl Default for IterModule {
    fn default() -> Self {
        Self::new()
    }
}

impl IterModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for IterModule {
    fn name(&self) -> &str {
        "iter"
    }

    fn description(&self) -> &str {
        "List-oriented iterator utilities"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::full_state("map", map, 2),
                RuntimeNativeExport::full_state("filter", filter, 2),
                RuntimeNativeExport::full_state("reduce", reduce, 3),
                RuntimeNativeExport::plain("enumerate", enumerate, 1),
                RuntimeNativeExport::plain("range", range, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("zip", zip, 2),
                RuntimeNativeExport::plain("take", take, 2),
                RuntimeNativeExport::plain("skip", skip, 2),
                RuntimeNativeExport::plain("chain", chain, 2),
                RuntimeNativeExport::plain("flatten", flatten, 1),
                RuntimeNativeExport::plain("unique", unique, 1),
                RuntimeNativeExport::plain("chunk", chunk, 2),
                RuntimeNativeExport::plain("next", next, 1),
                RuntimeNativeExport::plain("collect", collect, 1),
            ],
            &[],
        ))
    }
}

fn map(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.map")?;
    let values = args.as_slice();
    let input = list_snapshot_arg(&values[0], runtime.heap(), "iter.map first argument")?;
    let mut out = Vec::with_capacity(input.len());
    input.for_each_item(|item| {
        let value = item.into_runtime_value(runtime.heap_mut());
        out.push(call_callable(
            &values[1],
            &[value],
            runtime,
            "iter.map second argument",
        )?);
        Ok(())
    })?;
    runtime_list(out, runtime.heap_mut())
}

fn filter(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.filter")?;
    let values = args.as_slice();
    let input = list_snapshot_arg(&values[0], runtime.heap(), "iter.filter first argument")?;
    let mut out = Vec::with_capacity(input.len());
    input.for_each_item(|item| {
        let value = item.into_runtime_value(runtime.heap_mut());
        let keep = call_callable(
            &values[1],
            std::slice::from_ref(&value),
            runtime,
            "iter.filter second argument",
        )?;
        if truthy(&keep) {
            out.push(value);
        }
        Ok(())
    })?;
    runtime_list(out, runtime.heap_mut())
}

fn reduce(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "iter.reduce")?;
    let values = args.as_slice();
    let input = list_snapshot_arg(&values[0], runtime.heap(), "iter.reduce first argument")?;
    let mut acc = values[1].clone();
    input.for_each_item(|item| {
        let value = item.into_runtime_value(runtime.heap_mut());
        let previous = std::mem::replace(&mut acc, RuntimeVal::Nil);
        acc = call_callable(&values[2], &[previous, value], runtime, "iter.reduce third argument")?;
        Ok(())
    })?;
    Ok(acc)
}

fn enumerate(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.enumerate")?;
    let input = list_snapshot_arg(&args.as_slice()[0], runtime.heap(), "iter.enumerate")?;
    let mut out = Vec::with_capacity(input.len());
    let mut index = 0usize;
    input.for_each_item(|item| {
        let value = item.into_runtime_value(runtime.heap_mut());
        out.push(runtime_list(
            vec![RuntimeVal::Int(index as i64), value],
            runtime.heap_mut(),
        )?);
        index += 1;
        Ok(())
    })?;
    runtime_list(out, runtime.heap_mut())
}

fn range(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let (start, end, step) = match values {
        [end] => (0, int_arg(end, "iter.range end")?, 1),
        [start, end] => (int_arg(start, "iter.range start")?, int_arg(end, "iter.range end")?, 1),
        [start, end, step] => (
            int_arg(start, "iter.range start")?,
            int_arg(end, "iter.range end")?,
            int_arg(step, "iter.range step")?,
        ),
        _ => bail!("iter.range expects (end), (start, end), or (start, end, step)"),
    };
    if step == 0 {
        bail!("iter.range step cannot be zero");
    }

    let mut out = Vec::new();
    let mut current = start;
    if step > 0 {
        while current < end {
            out.push(current);
            current += step;
        }
    } else {
        while current > end {
            out.push(current);
            current += step;
        }
    }
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::List(TypedList::Int(out))),
    ))
}

fn zip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.zip")?;
    let values = args.as_slice();
    let pairs = {
        let left = typed_list_arg_ref(&values[0], runtime.heap(), "iter.zip first argument")?;
        let right = typed_list_arg_ref(&values[1], runtime.heap(), "iter.zip second argument")?;
        let count = left.len().min(right.len());
        let mut pairs = Vec::with_capacity(count);
        for index in 0..count {
            let left = typed_list_item_snapshot(left, index).expect("index bounded by count");
            let right = typed_list_item_snapshot(right, index).expect("index bounded by count");
            pairs.push((left, right));
        }
        pairs
    };
    let mut out = Vec::with_capacity(pairs.len());
    for (left, right) in pairs {
        let left = left.into_runtime_value(runtime.heap_mut());
        let right = right.into_runtime_value(runtime.heap_mut());
        out.push(runtime_list(vec![left, right], runtime.heap_mut())?);
    }
    runtime_list(out, runtime.heap_mut())
}

fn take(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.take")?;
    let values = args.as_slice();
    let n = count_arg(&values[1], "iter.take count")?;
    list_slice(&values[0], runtime.heap_mut(), 0, Some(n), "iter.take first argument")
}

fn skip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.skip")?;
    let values = args.as_slice();
    let n = count_arg(&values[1], "iter.skip count")?;
    list_slice(&values[0], runtime.heap_mut(), n, None, "iter.skip first argument")
}

fn chain(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.chain")?;
    let values = args.as_slice();
    let plan = typed_list_concat_preserving_backing(
        typed_list_arg_ref(&values[0], runtime.heap(), "iter.chain first argument")?,
        typed_list_arg_ref(&values[1], runtime.heap(), "iter.chain second argument")?,
    );
    let list = plan.into_typed(runtime.heap_mut());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn flatten(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.flatten")?;
    let plan = flatten_typed_list(
        typed_list_arg_ref(&args.as_slice()[0], runtime.heap(), "iter.flatten")?,
        runtime.heap(),
    )?;
    let list = plan.into_typed(runtime.heap_mut());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn unique(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.unique")?;
    let input = typed_list_arg_ref(&args.as_slice()[0], runtime.heap(), "iter.unique")?;
    let list = unique_typed_list(input, runtime.heap());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn chunk(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.chunk")?;
    let values = args.as_slice();
    let size = count_arg(&values[1], "iter.chunk size")?;
    if size == 0 {
        bail!("iter.chunk size must be positive");
    }
    let chunks = {
        let input = typed_list_arg_ref(&values[0], runtime.heap(), "iter.chunk first argument")?;
        let mut chunks = Vec::new();
        for start in (0..input.len()).step_by(size) {
            chunks.push(typed_list_slice(input, start, Some(size)));
        }
        chunks
    };
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        out.push(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(chunk))));
    }
    runtime_list(out, runtime.heap_mut())
}

fn next(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.next")?;
    first_list_item(&args.as_slice()[0], runtime.heap_mut(), "iter.next")
}

fn collect(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.collect")?;
    let input = typed_list_arg_ref(&args.as_slice()[0], runtime.heap(), "iter.collect")?;
    let input = copy_typed_list(input);
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(input))))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} expects exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
    }
}

fn maybe_typed_list_arg_ref<'a>(value: &RuntimeVal, heap: &'a HeapStore) -> Result<Option<&'a TypedList>> {
    let RuntimeVal::Obj(handle) = value else {
        return Ok(None);
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(Some(list)),
        _ => Ok(None),
    }
}

fn typed_list_arg_ref<'a>(value: &RuntimeVal, heap: &'a HeapStore, context: &str) -> Result<&'a TypedList> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(list),
        _ => bail!("{context} expects a list"),
    }
}

fn list_snapshot_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<RuntimeListSnapshot> {
    Ok(RuntimeListSnapshot::from_typed(typed_list_arg_ref(
        value, heap, context,
    )?))
}

fn copy_typed_list(list: &TypedList) -> TypedList {
    match list {
        TypedList::Mixed(values) => TypedList::Mixed(copy_slice(values)),
        TypedList::Int(values) => TypedList::Int(copy_slice(values)),
        TypedList::Float(values) => TypedList::Float(copy_slice(values)),
        TypedList::Bool(values) => TypedList::Bool(copy_slice(values)),
        TypedList::String(values) => TypedList::String(copy_slice(values)),
    }
}

fn copy_slice<T: Clone>(values: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(values);
    out
}

fn first_list_item(value: &RuntimeVal, heap: &mut HeapStore, context: &str) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::List(list) = value else {
        bail!("{context} expects a list");
    };
    let string = match list {
        TypedList::Mixed(values) => return Ok(values.first().cloned().unwrap_or(RuntimeVal::Nil)),
        TypedList::Int(values) => return Ok(values.first().copied().map(RuntimeVal::Int).unwrap_or(RuntimeVal::Nil)),
        TypedList::Float(values) => {
            return Ok(values
                .first()
                .copied()
                .map(RuntimeVal::Float)
                .unwrap_or(RuntimeVal::Nil));
        }
        TypedList::Bool(values) => return Ok(values.first().copied().map(RuntimeVal::Bool).unwrap_or(RuntimeVal::Nil)),
        TypedList::String(values) => {
            let Some(value) = values.first() else {
                return Ok(RuntimeVal::Nil);
            };
            if let Some(short) = ShortStr::new(value) {
                return Ok(RuntimeVal::ShortStr(short));
            }
            value.clone()
        }
    };
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::String(string))))
}

enum RuntimeListItemSnapshot {
    Value(RuntimeVal),
    String(Arc<str>),
}

impl RuntimeListItemSnapshot {
    fn into_runtime_value(self, heap: &mut HeapStore) -> RuntimeVal {
        match self {
            Self::Value(value) => value,
            Self::String(value) => {
                if let Some(short) = ShortStr::new(&value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(heap.alloc(HeapValue::String(value)))
                }
            }
        }
    }
}

fn typed_list_item_snapshot(list: &TypedList, index: usize) -> Option<RuntimeListItemSnapshot> {
    Some(match list {
        TypedList::Mixed(values) => RuntimeListItemSnapshot::Value(values.get(index)?.clone()),
        TypedList::Int(values) => RuntimeListItemSnapshot::Value(RuntimeVal::Int(*values.get(index)?)),
        TypedList::Float(values) => RuntimeListItemSnapshot::Value(RuntimeVal::Float(*values.get(index)?)),
        TypedList::Bool(values) => RuntimeListItemSnapshot::Value(RuntimeVal::Bool(*values.get(index)?)),
        TypedList::String(values) => RuntimeListItemSnapshot::String(Arc::clone(values.get(index)?)),
    })
}

fn list_slice(
    value: &RuntimeVal,
    heap: &mut HeapStore,
    start: usize,
    limit: Option<usize>,
    context: &str,
) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list");
    };
    let list = {
        let value = heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::List(list) = value else {
            bail!("{context} expects a list");
        };
        typed_list_slice(list, start, limit)
    };
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::List(list))))
}

fn typed_list_slice(list: &TypedList, start: usize, limit: Option<usize>) -> TypedList {
    let len = list.len();
    let start = start.min(len);
    let end = limit.map_or(len, |limit| start.saturating_add(limit).min(len));
    match list {
        TypedList::Mixed(values) => TypedList::Mixed(copy_slice(&values[start..end])),
        TypedList::Int(values) => TypedList::Int(copy_slice(&values[start..end])),
        TypedList::Float(values) => TypedList::Float(copy_slice(&values[start..end])),
        TypedList::Bool(values) => TypedList::Bool(copy_slice(&values[start..end])),
        TypedList::String(values) => TypedList::String(copy_slice(&values[start..end])),
    }
}

enum RuntimeListSnapshot {
    Mixed(Vec<RuntimeVal>),
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
}

impl RuntimeListSnapshot {
    fn from_typed(list: &TypedList) -> Self {
        match list {
            TypedList::Mixed(values) => Self::Mixed(copy_slice(values)),
            TypedList::Int(values) => Self::Int(copy_slice(values)),
            TypedList::Float(values) => Self::Float(copy_slice(values)),
            TypedList::Bool(values) => Self::Bool(copy_slice(values)),
            TypedList::String(values) => Self::String(copy_slice(values)),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::String(values) => values.len(),
        }
    }

    fn for_each_item(self, mut f: impl FnMut(RuntimeListItemSnapshot) -> Result<()>) -> Result<()> {
        match self {
            Self::Mixed(values) => {
                for value in values {
                    f(RuntimeListItemSnapshot::Value(value))?;
                }
            }
            Self::Int(values) => {
                for value in values {
                    f(RuntimeListItemSnapshot::Value(RuntimeVal::Int(value)))?;
                }
            }
            Self::Float(values) => {
                for value in values {
                    f(RuntimeListItemSnapshot::Value(RuntimeVal::Float(value)))?;
                }
            }
            Self::Bool(values) => {
                for value in values {
                    f(RuntimeListItemSnapshot::Value(RuntimeVal::Bool(value)))?;
                }
            }
            Self::String(values) => {
                for value in values {
                    f(RuntimeListItemSnapshot::String(value))?;
                }
            }
        }
        Ok(())
    }

    fn into_typed(self) -> TypedList {
        match self {
            Self::Mixed(values) => TypedList::Mixed(values),
            Self::Int(values) => TypedList::Int(values),
            Self::Float(values) => TypedList::Float(values),
            Self::Bool(values) => TypedList::Bool(values),
            Self::String(values) => TypedList::String(values),
        }
    }

    fn append_to_mixed_output(self, out: &mut Vec<RuntimeVal>, heap: &mut HeapStore) {
        match self {
            Self::Mixed(values) => out.extend(values),
            Self::Int(values) => out.extend(values.into_iter().map(RuntimeVal::Int)),
            Self::Float(values) => out.extend(values.into_iter().map(RuntimeVal::Float)),
            Self::Bool(values) => out.extend(values.into_iter().map(RuntimeVal::Bool)),
            Self::String(values) => out.extend(values.into_iter().map(|value| {
                if let Some(short) = ShortStr::new(&value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(heap.alloc(HeapValue::String(value)))
                }
            })),
        }
    }
}

enum ListConcatPlan {
    Ready(TypedList),
    Mixed {
        left: RuntimeListSnapshot,
        right: RuntimeListSnapshot,
    },
}

impl ListConcatPlan {
    fn into_typed(self, heap: &mut HeapStore) -> TypedList {
        match self {
            Self::Ready(list) => list,
            Self::Mixed { left, right } => {
                let mut values = Vec::with_capacity(left.len() + right.len());
                left.append_to_mixed_output(&mut values, heap);
                right.append_to_mixed_output(&mut values, heap);
                TypedList::Mixed(values)
            }
        }
    }
}

fn typed_list_concat_preserving_backing(left: &TypedList, right: &TypedList) -> ListConcatPlan {
    match (left, right) {
        (TypedList::Int(left), TypedList::Int(right)) => {
            ListConcatPlan::Ready(TypedList::Int(copy_concat(left, right)))
        }
        (TypedList::Float(left), TypedList::Float(right)) => {
            ListConcatPlan::Ready(TypedList::Float(copy_concat(left, right)))
        }
        (TypedList::Bool(left), TypedList::Bool(right)) => {
            ListConcatPlan::Ready(TypedList::Bool(copy_concat(left, right)))
        }
        (TypedList::String(left), TypedList::String(right)) => {
            ListConcatPlan::Ready(TypedList::String(copy_concat(left, right)))
        }
        (left, right) => ListConcatPlan::Mixed {
            left: RuntimeListSnapshot::from_typed(left),
            right: RuntimeListSnapshot::from_typed(right),
        },
    }
}

fn copy_concat<T: Clone>(left: &[T], right: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    out.extend_from_slice(left);
    out.extend_from_slice(right);
    out
}

enum FlattenItem {
    List(RuntimeListSnapshot),
    Value(RuntimeVal),
}

enum FlattenPlan {
    Ready(TypedList),
    Items(Vec<FlattenItem>),
}

impl FlattenPlan {
    fn into_typed(self, heap: &mut HeapStore) -> TypedList {
        match self {
            Self::Ready(list) => list,
            Self::Items(items) => flatten_items_into_typed(items, heap),
        }
    }
}

fn flatten_typed_list(input: &TypedList, heap: &HeapStore) -> Result<FlattenPlan> {
    let TypedList::Mixed(values) = input else {
        return Ok(FlattenPlan::Ready(RuntimeListSnapshot::from_typed(input).into_typed()));
    };
    let mut items = Vec::with_capacity(values.len());
    for value in values.iter() {
        if let Some(list) = maybe_typed_list_arg_ref(value, heap)? {
            items.push(FlattenItem::List(RuntimeListSnapshot::from_typed(list)));
        } else {
            items.push(FlattenItem::Value(value.clone()));
        }
    }
    Ok(FlattenPlan::Items(items))
}

fn flatten_items_into_typed(items: Vec<FlattenItem>, heap: &mut HeapStore) -> TypedList {
    let mut typed_out: Option<RuntimeListSnapshot> = None;
    let mut mixed_out: Option<Vec<RuntimeVal>> = None;
    for item in items {
        match item {
            FlattenItem::List(list) => {
                if let Some(out) = mixed_out.as_mut() {
                    list.append_to_mixed_output(out, heap);
                } else {
                    typed_out = Some(match typed_out.take() {
                        Some(current) => concat_list_snapshots(current, list, heap),
                        None => list,
                    });
                }
            }
            FlattenItem::Value(value) => {
                let out = mixed_out.get_or_insert_with(|| {
                    let Some(list) = typed_out.take() else {
                        return Vec::new();
                    };
                    let mut values = Vec::with_capacity(list.len());
                    list.append_to_mixed_output(&mut values, heap);
                    values
                });
                out.push(value);
            }
        }
    }
    match mixed_out {
        Some(values) => crate::typed_list_from_values(values, heap),
        None => typed_out
            .map(RuntimeListSnapshot::into_typed)
            .unwrap_or_else(|| TypedList::Mixed(Vec::new())),
    }
}

fn concat_list_snapshots(
    left: RuntimeListSnapshot,
    right: RuntimeListSnapshot,
    heap: &mut HeapStore,
) -> RuntimeListSnapshot {
    match (left, right) {
        (RuntimeListSnapshot::Int(left), RuntimeListSnapshot::Int(right)) => {
            RuntimeListSnapshot::Int(copy_concat_owned(left, right))
        }
        (RuntimeListSnapshot::Float(left), RuntimeListSnapshot::Float(right)) => {
            RuntimeListSnapshot::Float(copy_concat_owned(left, right))
        }
        (RuntimeListSnapshot::Bool(left), RuntimeListSnapshot::Bool(right)) => {
            RuntimeListSnapshot::Bool(copy_concat_owned(left, right))
        }
        (RuntimeListSnapshot::String(left), RuntimeListSnapshot::String(right)) => {
            RuntimeListSnapshot::String(copy_concat_owned(left, right))
        }
        (left, right) => {
            let mut values = Vec::with_capacity(left.len() + right.len());
            left.append_to_mixed_output(&mut values, heap);
            right.append_to_mixed_output(&mut values, heap);
            RuntimeListSnapshot::Mixed(values)
        }
    }
}

fn copy_concat_owned<T>(left: Vec<T>, right: Vec<T>) -> Vec<T> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    out.extend(left);
    out.extend(right);
    out
}

fn unique_typed_list(input: &TypedList, heap: &HeapStore) -> TypedList {
    match input {
        TypedList::Mixed(values) => unique_mixed_values(values, heap),
        TypedList::Int(values) => TypedList::Int(unique_copy_values(values)),
        TypedList::Float(values) => TypedList::Float(unique_copy_values(values)),
        TypedList::Bool(values) => TypedList::Bool(unique_copy_values(values)),
        TypedList::String(values) => TypedList::String(unique_arc_values(values)),
    }
}

fn unique_mixed_values(values: &[RuntimeVal], heap: &HeapStore) -> TypedList {
    let mut out: Vec<RuntimeVal> = Vec::with_capacity(values.len());
    for value in values {
        if !out.iter().any(|existing| runtime_values_equal(existing, value, heap)) {
            out.push(value.clone());
        }
    }
    crate::typed_list_from_values(out, heap)
}

fn unique_copy_values<T>(values: &[T]) -> Vec<T>
where
    T: Copy + PartialEq,
{
    let mut out = Vec::with_capacity(values.len());
    for value in values.iter().copied() {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

fn unique_arc_values(values: &[std::sync::Arc<str>]) -> Vec<std::sync::Arc<str>> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        if !out
            .iter()
            .any(|existing: &std::sync::Arc<str>| existing.as_ref() == value.as_ref())
        {
            out.push(Arc::clone(value));
        }
    }
    out
}

fn runtime_list(values: Vec<RuntimeVal>, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let list = crate::typed_list_from_values(values, heap);
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::List(list))))
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be an integer")),
    }
}

fn count_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    let value = int_arg(value, context)?;
    if value < 0 {
        bail!("{context} must be non-negative");
    }
    usize::try_from(value).map_err(|_| anyhow!("{context} is too large"))
}

fn truthy(value: &RuntimeVal) -> bool {
    !matches!(value, RuntimeVal::Nil | RuntimeVal::Bool(false))
}

fn call_callable(
    callable_value: &RuntimeVal,
    args: &[RuntimeVal],
    runtime: &mut NativeRuntime<'_>,
    context: &str,
) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = callable_value else {
        bail!("{context} must be callable");
    };

    enum IterCallableTarget {
        Runtime(Arc<lk_core::vm::RuntimeCallable>),
        Closure,
        RuntimeNative { arity: u16, function: NativeFunction },
    }

    let target = match runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Callable(CallableValue::Runtime(function)) => IterCallableTarget::Runtime(Arc::clone(function)),
        HeapValue::Callable(CallableValue::Closure { .. }) => IterCallableTarget::Closure,
        HeapValue::Callable(CallableValue::RuntimeNative { arity, function, .. }) => {
            IterCallableTarget::RuntimeNative {
                arity: *arity,
                function: function.clone(),
            }
        }
        _ => bail!("{context} must be callable"),
    };

    match target {
        IterCallableTarget::Runtime(function) => {
            let (heap, ctx) = runtime.heap_ctx_mut();
            call_runtime_callable_runtime(function.as_ref(), args, heap, ctx)
        }
        IterCallableTarget::Closure => {
            if let Some((state, ctx, module)) = runtime.state_ctx_module_mut() {
                return call_runtime_value_runtime(RuntimeVal::Obj(*handle), args, state, module, ctx);
            }
            bail!("{context} closure requires active RuntimeModuleState")
        }
        IterCallableTarget::RuntimeNative { arity, function } => {
            let entry = NativeEntry {
                name: context.to_string(),
                arity,
                function,
            };
            if !entry.accepts_arity(args.len() as u16) {
                bail!("{context} expects {arity} arguments, got {}", args.len());
            }
            call_runtime_native_entry(&entry, args, runtime)
        }
    }
}

fn call_runtime_native_entry(
    entry: &NativeEntry,
    args: &[RuntimeVal],
    runtime: &mut NativeRuntime<'_>,
) -> Result<RuntimeVal> {
    match &entry.function {
        NativeFunction::Plain(function) | NativeFunction::Context(function) | NativeFunction::FullState(function) => {
            function(NativeArgs::new(args), runtime)
        }
    }
}

fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> bool {
    if left == right {
        return true;
    }
    let (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) = (left, right) else {
        return false;
    };
    let (Some(left), Some(right)) = (heap.get(*left), heap.get(*right)) else {
        return false;
    };
    match (left, right) {
        (HeapValue::String(left), HeapValue::String(right)) => left == right,
        (HeapValue::List(left), HeapValue::List(right)) => runtime_lists_equal(left, right, heap),
        (HeapValue::Map(left), HeapValue::Map(right)) => runtime_maps_equal(left, right, heap),
        _ => false,
    }
}

fn runtime_maps_equal(left: &TypedMap, right: &TypedMap, heap: &HeapStore) -> bool {
    if left.len() != right.len() {
        return false;
    }
    match left {
        TypedMap::Mixed(entries) => entries
            .iter()
            .all(|(key, value)| runtime_map_value_equal(right, key, value, heap)),
        TypedMap::StringMixed(entries) => entries
            .iter()
            .all(|(key, value)| runtime_map_value_equal(right, &RuntimeMapKey::String(key.clone()), value, heap)),
        TypedMap::StringInt(entries) => entries.iter().all(|(key, value)| {
            runtime_map_value_equal(
                right,
                &RuntimeMapKey::String(key.clone()),
                &RuntimeVal::Int(*value),
                heap,
            )
        }),
        TypedMap::StringFloat(entries) => entries.iter().all(|(key, value)| {
            runtime_map_value_equal(
                right,
                &RuntimeMapKey::String(key.clone()),
                &RuntimeVal::Float(*value),
                heap,
            )
        }),
        TypedMap::StringBool(entries) => entries.iter().all(|(key, value)| {
            runtime_map_value_equal(
                right,
                &RuntimeMapKey::String(key.clone()),
                &RuntimeVal::Bool(*value),
                heap,
            )
        }),
    }
}

fn runtime_map_value_equal(right: &TypedMap, key: &RuntimeMapKey, left: &RuntimeVal, heap: &HeapStore) -> bool {
    right
        .get(key)
        .is_some_and(|right| runtime_values_equal(left, &right, heap))
}

fn runtime_lists_equal(left: &TypedList, right: &TypedList, heap: &HeapStore) -> bool {
    if left.len() != right.len() {
        return false;
    }
    match (left, right) {
        (TypedList::Int(left), TypedList::Int(right)) => return left == right,
        (TypedList::Float(left), TypedList::Float(right)) => return left == right,
        (TypedList::Bool(left), TypedList::Bool(right)) => return left == right,
        (TypedList::String(left), TypedList::String(right)) => return left == right,
        _ => {}
    }
    (0..left.len()).all(|index| runtime_list_items_equal(left, index, right, index, heap))
}

fn runtime_list_items_equal(
    left: &TypedList,
    left_index: usize,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> bool {
    match (left, right) {
        (TypedList::Mixed(left), TypedList::Mixed(right)) => {
            runtime_values_equal(&left[left_index], &right[right_index], heap)
        }
        (TypedList::Mixed(left), TypedList::String(right)) => {
            runtime_value_equals_string(&left[left_index], &right[right_index], heap)
        }
        (TypedList::String(left), TypedList::Mixed(right)) => {
            runtime_value_equals_string(&right[right_index], &left[left_index], heap)
        }
        (TypedList::Int(left), _) => {
            runtime_list_runtime_item_equal(RuntimeVal::Int(left[left_index]), right, right_index, heap)
        }
        (TypedList::Float(left), _) => {
            runtime_list_runtime_item_equal(RuntimeVal::Float(left[left_index]), right, right_index, heap)
        }
        (TypedList::Bool(left), _) => {
            runtime_list_runtime_item_equal(RuntimeVal::Bool(left[left_index]), right, right_index, heap)
        }
        (TypedList::String(left), _) => runtime_list_string_item_equal(&left[left_index], right, right_index, heap),
        (TypedList::Mixed(left), _) => {
            runtime_list_runtime_item_equal(left[left_index].clone(), right, right_index, heap)
        }
    }
}

fn runtime_list_runtime_item_equal(left: RuntimeVal, right: &TypedList, right_index: usize, heap: &HeapStore) -> bool {
    match right {
        TypedList::Mixed(right) => runtime_values_equal(&left, &right[right_index], heap),
        TypedList::Int(right) => runtime_values_equal(&left, &RuntimeVal::Int(right[right_index]), heap),
        TypedList::Float(right) => runtime_values_equal(&left, &RuntimeVal::Float(right[right_index]), heap),
        TypedList::Bool(right) => runtime_values_equal(&left, &RuntimeVal::Bool(right[right_index]), heap),
        TypedList::String(right) => runtime_value_equals_string(&left, &right[right_index], heap),
    }
}

fn runtime_list_string_item_equal(left: &Arc<str>, right: &TypedList, right_index: usize, heap: &HeapStore) -> bool {
    match right {
        TypedList::Mixed(right) => runtime_value_equals_string(&right[right_index], left, heap),
        TypedList::String(right) => left == &right[right_index],
        _ => false,
    }
}

fn runtime_value_equals_string(value: &RuntimeVal, expected: &str, heap: &HeapStore) -> bool {
    match value {
        RuntimeVal::ShortStr(value) => value.as_str() == expected,
        RuntimeVal::Obj(handle) => {
            matches!(heap.get(*handle), Some(HeapValue::String(value)) if value.as_ref() == expected)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register_stdlib_modules;
    use lk_core::{
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        vm::{NativeFunction, ProgramResult, RuntimeModuleState, VmContext},
    };
    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = lk_core::module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn run_value(source: &str) -> Result<RuntimeVal> {
        Ok(run(source)?.first_return().clone())
    }

    fn expect_list(value: &RuntimeVal, heap: &HeapStore) -> Vec<RuntimeVal> {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("expected runtime list heap value");
        };
        match list {
            TypedList::Mixed(values) => values.clone(),
            TypedList::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            TypedList::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            TypedList::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            TypedList::String(values) => values
                .iter()
                .map(|value| RuntimeVal::ShortStr(lk_core::val::ShortStr::new(value).expect("short test string")))
                .collect(),
        }
    }

    fn expect_return_list(result: &ProgramResult) -> Vec<RuntimeVal> {
        expect_list(result.first_return(), result.state.heap())
    }

    fn iter_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&IterModule::new(), name)
    }

    #[test]
    fn iter_exports_use_runtime_native_abi() -> Result<()> {
        for name in ["map", "filter", "reduce"] {
            let (_, function) = iter_native(name)?;
            assert!(matches!(function, NativeFunction::FullState(_)));
        }
        for name in [
            "enumerate",
            "range",
            "zip",
            "take",
            "skip",
            "chain",
            "flatten",
            "unique",
            "chunk",
            "next",
            "collect",
        ] {
            let (_, function) = iter_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        Ok(())
    }

    #[test]
    fn iter_sequence_ops_run_on_exec() -> Result<()> {
        assert_eq!(
            expect_return_list(&run("use iter; return iter.range(0, 6, 2);")?),
            vec![RuntimeVal::Int(0), RuntimeVal::Int(2), RuntimeVal::Int(4)]
        );
        let result = run("use iter; return iter.zip([1,2], [\"a\",\"b\",\"c\"]);")?;
        let zipped = expect_return_list(&result);
        assert_eq!(zipped.len(), 2);
        assert_eq!(
            expect_list(&zipped[0], result.state.heap()),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::ShortStr(lk_core::val::ShortStr::new("a").expect("short"))
            ]
        );
        assert_eq!(
            expect_list(&zipped[1], result.state.heap()),
            vec![
                RuntimeVal::Int(2),
                RuntimeVal::ShortStr(lk_core::val::ShortStr::new("b").expect("short"))
            ]
        );
        assert_eq!(
            expect_return_list(&run(
                "use iter; return iter.chain(iter.take([1,2,3], 2), iter.skip([4,5,6], 1));"
            )?),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(5),
                RuntimeVal::Int(6)
            ]
        );
        Ok(())
    }

    #[test]
    fn iter_list_shape_ops_run_on_exec() -> Result<()> {
        assert_eq!(
            expect_return_list(&run(
                "use iter; let a = [1,2]; let b = [3]; let c = [4]; return iter.flatten([a,b,c]);"
            )?),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4)
            ]
        );
        assert_eq!(
            expect_return_list(&run("use iter; return iter.unique([1,1,2,2,3]);")?),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2), RuntimeVal::Int(3)]
        );
        let result = run("use iter; return iter.chunk([1,2,3,4,5], 2);")?;
        let chunks = expect_return_list(&result);
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            expect_list(&chunks[0], result.state.heap()),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2)]
        );
        assert_eq!(
            expect_list(&chunks[1], result.state.heap()),
            vec![RuntimeVal::Int(3), RuntimeVal::Int(4)]
        );
        assert_eq!(expect_list(&chunks[2], result.state.heap()), vec![RuntimeVal::Int(5)]);
        Ok(())
    }

    #[test]
    fn iter_higher_order_ops_call_runtime_closures() -> Result<()> {
        assert_eq!(
            expect_return_list(&run("use iter; return iter.map([1,2,3], fn(x) => x * 2);")?),
            vec![RuntimeVal::Int(2), RuntimeVal::Int(4), RuntimeVal::Int(6)]
        );
        assert_eq!(
            expect_return_list(&run(
                "use iter; return iter.filter([1,2,3,4], fn(x) => x % 2 == 0);"
            )?),
            vec![RuntimeVal::Int(2), RuntimeVal::Int(4)]
        );
        assert_eq!(
            run_value("use iter; return iter.reduce([1,2,3], 0, fn(acc, x) => acc + x);")?,
            RuntimeVal::Int(6)
        );
        Ok(())
    }

    #[test]
    fn iter_map_materializes_long_string_items_lazily_for_callback() -> Result<()> {
        fn fail_on_first(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
            bail!("stop after first item");
        }

        let (_, function) = iter_native("map")?;
        let NativeFunction::FullState(function) = function else {
            panic!("map must use FullState RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let input = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-map-first"),
            Arc::<str>::from("long-map-second"),
        ])));
        let callback = state
            .heap_mut()
            .alloc(HeapValue::Callable(CallableValue::RuntimeNative {
                name: Arc::<str>::from("fail_on_first"),
                arity: 1,
                function: NativeFunction::Plain(fail_on_first),
            }));
        let args = [RuntimeVal::Obj(input), RuntimeVal::Obj(callback)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let err = function(NativeArgs::new(&args), &mut runtime).expect_err("callback should fail");

        assert!(err.to_string().contains("stop after first item"));
        assert_eq!(runtime.heap().len(), 3);
        Ok(())
    }

    #[test]
    fn iter_direct_runtime_call_preserves_typed_lists() -> Result<()> {
        let (_, function) = iter_native("range")?;
        let NativeFunction::Plain(function) = function else {
            panic!("range must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let args = [RuntimeVal::Int(1), RuntimeVal::Int(4)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let result = function(NativeArgs::new(&args), &mut runtime)?;
        assert_eq!(
            expect_list(&result, runtime.heap()),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2), RuntimeVal::Int(3)]
        );
        Ok(())
    }

    #[test]
    fn iter_take_skip_slice_typed_string_lists_without_materializing_items() -> Result<()> {
        let long = Arc::<str>::from("long-string-value");
        for (name, args) in [
            ("take", [RuntimeVal::Nil, RuntimeVal::Int(1)]),
            ("skip", [RuntimeVal::Nil, RuntimeVal::Int(1)]),
        ] {
            let (_, function) = iter_native(name)?;
            let NativeFunction::Plain(function) = function else {
                panic!("{name} must use plain RuntimeNative");
            };
            let mut state = RuntimeModuleState::default();
            let list = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
                Arc::clone(&long),
                Arc::<str>::from("tail"),
            ])));
            let mut args = args;
            args[0] = RuntimeVal::Obj(list);
            let mut runtime = NativeRuntime::new(&mut state, None, None);

            let result = function(NativeArgs::new(&args), &mut runtime)?;

            let RuntimeVal::Obj(handle) = result else {
                panic!("expected list result");
            };
            let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
                panic!("expected typed string list result");
            };
            assert_eq!(values.len(), 1);
            assert_eq!(runtime.heap().len(), 2);
        }
        Ok(())
    }

    #[test]
    fn iter_chain_preserves_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, function) = iter_native("chain")?;
        let NativeFunction::Plain(function) = function else {
            panic!("chain must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let left = state
            .heap_mut()
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-left-value",
            )])));
        let right = state
            .heap_mut()
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-right-value",
            )])));
        let args = [RuntimeVal::Obj(left), RuntimeVal::Obj(right)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected list result");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("expected typed string list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(runtime.heap().len(), 3);
        Ok(())
    }

    #[test]
    fn iter_chunk_preserves_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, function) = iter_native("chunk")?;
        let NativeFunction::Plain(function) = function else {
            panic!("chunk must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let input = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-one-value"),
            Arc::<str>::from("long-two-value"),
            Arc::<str>::from("long-three-value"),
        ])));
        let args = [RuntimeVal::Obj(input), RuntimeVal::Int(2)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(outer) = result else {
            panic!("expected outer list");
        };
        let Some(HeapValue::List(TypedList::Mixed(chunks))) = runtime.heap().get(outer) else {
            panic!("expected mixed outer list");
        };
        assert_eq!(chunks.len(), 2);
        for chunk in chunks {
            let RuntimeVal::Obj(handle) = chunk else {
                panic!("expected chunk list object");
            };
            assert!(matches!(
                runtime.heap().get(*handle),
                Some(HeapValue::List(TypedList::String(_)))
            ));
        }
        assert_eq!(runtime.heap().len(), 4);
        Ok(())
    }

    #[test]
    fn iter_zip_materializes_only_used_long_string_items() -> Result<()> {
        let (_, function) = iter_native("zip")?;
        let NativeFunction::Plain(function) = function else {
            panic!("zip must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let left = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-left-used"),
            Arc::<str>::from("long-left-unused"),
        ])));
        let right = state
            .heap_mut()
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-right-used",
            )])));
        let args = [RuntimeVal::Obj(left), RuntimeVal::Obj(right)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(outer) = result else {
            panic!("expected outer list");
        };
        let Some(HeapValue::List(TypedList::Mixed(pairs))) = runtime.heap().get(outer) else {
            panic!("expected mixed outer list");
        };
        assert_eq!(pairs.len(), 1);
        let RuntimeVal::Obj(pair) = pairs[0] else {
            panic!("expected pair list");
        };
        let Some(HeapValue::List(TypedList::String(pair_values))) = runtime.heap().get(pair) else {
            panic!("expected typed string pair list");
        };
        assert_eq!(pair_values.len(), 2);
        assert_eq!(runtime.heap().len(), 6);
        Ok(())
    }

    #[test]
    fn iter_collect_preserves_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, function) = iter_native("collect")?;
        let NativeFunction::Plain(function) = function else {
            panic!("collect must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let input = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-collect-one"),
            Arc::<str>::from("long-collect-two"),
        ])));
        let args = [RuntimeVal::Obj(input)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected list result");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("expected typed string list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(runtime.heap().len(), 2);
        Ok(())
    }

    #[test]
    fn iter_flatten_preserves_nested_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, function) = iter_native("flatten")?;
        let NativeFunction::Plain(function) = function else {
            panic!("flatten must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let first = state
            .heap_mut()
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-flatten-one",
            )])));
        let second = state
            .heap_mut()
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-flatten-two",
            )])));
        let outer = state.heap_mut().alloc(HeapValue::List(TypedList::Mixed(vec![
            RuntimeVal::Obj(first),
            RuntimeVal::Obj(second),
        ])));
        let args = [RuntimeVal::Obj(outer)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected list result");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("expected typed string list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(runtime.heap().len(), 4);
        Ok(())
    }

    #[test]
    fn iter_unique_preserves_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, function) = iter_native("unique")?;
        let NativeFunction::Plain(function) = function else {
            panic!("unique must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let input = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-unique-one"),
            Arc::<str>::from("long-unique-one"),
            Arc::<str>::from("long-unique-two"),
        ])));
        let args = [RuntimeVal::Obj(input)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected list result");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("expected typed string list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(runtime.heap().len(), 2);
        Ok(())
    }

    #[test]
    fn iter_collect_and_next_accept_lists_only() -> Result<()> {
        assert_eq!(run_value("use iter; return iter.next([7,8]);")?, RuntimeVal::Int(7));
        assert_eq!(run_value("use iter; return iter.next([]);")?, RuntimeVal::Nil);
        assert_eq!(
            expect_return_list(&run("use iter; return iter.collect([1,2]);")?),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2)]
        );
        Ok(())
    }
}
