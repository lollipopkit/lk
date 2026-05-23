use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{CallableValue, HeapStore, HeapValue, RuntimeVal, ShortStr, TypedList},
    vm::{
        NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeExport32,
        call_runtime_callable32_runtime, call_runtime_value32_runtime,
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

impl Module for IterModule {
    fn name(&self) -> &str {
        "iter"
    }

    fn description(&self) -> &str {
        "List-oriented iterator utilities"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::full_state("map", map32, 2),
                RuntimeNativeExport32::full_state("filter", filter32, 2),
                RuntimeNativeExport32::full_state("reduce", reduce32, 3),
                RuntimeNativeExport32::plain("enumerate", enumerate32, 1),
                RuntimeNativeExport32::plain("range", range32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("zip", zip32, 2),
                RuntimeNativeExport32::plain("take", take32, 2),
                RuntimeNativeExport32::plain("skip", skip32, 2),
                RuntimeNativeExport32::plain("chain", chain32, 2),
                RuntimeNativeExport32::plain("flatten", flatten32, 1),
                RuntimeNativeExport32::plain("unique", unique32, 1),
                RuntimeNativeExport32::plain("chunk", chunk32, 2),
                RuntimeNativeExport32::plain("next", next32, 1),
                RuntimeNativeExport32::plain("collect", collect32, 1),
            ],
            &[],
        ))
    }
}

fn map32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.map")?;
    let values = args.as_slice();
    let input = list_items(&values[0], runtime.heap_mut(), "iter.map first argument")?;
    let mut out = Vec::with_capacity(input.len());
    for value in input {
        out.push(call_callable(
            &values[1],
            &[value],
            runtime,
            "iter.map second argument",
        )?);
    }
    runtime_list(out, runtime.heap_mut())
}

fn filter32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.filter")?;
    let values = args.as_slice();
    let input = list_items(&values[0], runtime.heap_mut(), "iter.filter first argument")?;
    let mut out = Vec::with_capacity(input.len());
    for value in input {
        let keep = call_callable(
            &values[1],
            std::slice::from_ref(&value),
            runtime,
            "iter.filter second argument",
        )?;
        if truthy(&keep) {
            out.push(value);
        }
    }
    runtime_list(out, runtime.heap_mut())
}

fn reduce32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "iter.reduce")?;
    let values = args.as_slice();
    let input = list_items(&values[0], runtime.heap_mut(), "iter.reduce first argument")?;
    let mut acc = values[1].clone();
    for value in input {
        acc = call_callable(&values[2], &[acc, value], runtime, "iter.reduce third argument")?;
    }
    Ok(acc)
}

fn enumerate32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let input = one_list(args, runtime, "iter.enumerate")?;
    let mut out = Vec::with_capacity(input.len());
    for (index, value) in input.into_iter().enumerate() {
        out.push(runtime_list(
            vec![RuntimeVal::Int(index as i64), value],
            runtime.heap_mut(),
        )?);
    }
    runtime_list(out, runtime.heap_mut())
}

fn range32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

fn zip32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.zip")?;
    let values = args.as_slice();
    let left = typed_list_arg(&values[0], runtime.heap(), "iter.zip first argument")?;
    let right = typed_list_arg(&values[1], runtime.heap(), "iter.zip second argument")?;
    let count = left.len().min(right.len());
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let left = typed_list_item(&left, index, runtime.heap_mut()).expect("index bounded by count");
        let right = typed_list_item(&right, index, runtime.heap_mut()).expect("index bounded by count");
        out.push(runtime_list(vec![left, right], runtime.heap_mut())?);
    }
    runtime_list(out, runtime.heap_mut())
}

fn take32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.take")?;
    let values = args.as_slice();
    let n = count_arg(&values[1], "iter.take count")?;
    list_slice(&values[0], runtime.heap_mut(), 0, Some(n), "iter.take first argument")
}

fn skip32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.skip")?;
    let values = args.as_slice();
    let n = count_arg(&values[1], "iter.skip count")?;
    list_slice(&values[0], runtime.heap_mut(), n, None, "iter.skip first argument")
}

fn chain32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.chain")?;
    let values = args.as_slice();
    let left = typed_list_arg(&values[0], runtime.heap(), "iter.chain first argument")?;
    let right = typed_list_arg(&values[1], runtime.heap(), "iter.chain second argument")?;
    let list = typed_list_concat_preserving_backing(left, right, runtime.heap_mut());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn flatten32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.flatten")?;
    let input = typed_list_arg(&args.as_slice()[0], runtime.heap(), "iter.flatten")?;
    let list = flatten_typed_list(input, runtime.heap_mut())?;
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn unique32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.unique")?;
    let input = typed_list_arg(&args.as_slice()[0], runtime.heap(), "iter.unique")?;
    let list = unique_typed_list(input, runtime.heap());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn chunk32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "iter.chunk")?;
    let values = args.as_slice();
    let input = typed_list_arg(&values[0], runtime.heap(), "iter.chunk first argument")?;
    let size = count_arg(&values[1], "iter.chunk size")?;
    if size == 0 {
        bail!("iter.chunk size must be positive");
    }
    let mut out = Vec::new();
    for start in (0..input.len()).step_by(size) {
        let chunk = typed_list_slice(&input, start, Some(size));
        out.push(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(chunk))));
    }
    runtime_list(out, runtime.heap_mut())
}

fn next32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.next")?;
    first_list_item(&args.as_slice()[0], runtime.heap_mut(), "iter.next")
}

fn collect32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "iter.collect")?;
    let input = typed_list_arg(&args.as_slice()[0], runtime.heap(), "iter.collect")?;
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(input))))
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} expects exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
    }
}

fn one_list(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>, name: &str) -> Result<Vec<RuntimeVal>> {
    expect_arity(args, 1, name)?;
    list_items(&args.as_slice()[0], runtime.heap_mut(), name)
}

fn list_items(value: &RuntimeVal, heap: &mut HeapStore, context: &str) -> Result<Vec<RuntimeVal>> {
    match maybe_list_items(value, heap)? {
        Some(values) => Ok(values),
        None => bail!("{context} expects a list"),
    }
}

fn maybe_list_items(value: &RuntimeVal, heap: &mut HeapStore) -> Result<Option<Vec<RuntimeVal>>> {
    let RuntimeVal::Obj(handle) = value else {
        return Ok(None);
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => {
            let list = list.clone();
            Ok(Some(list.runtime_values_into_heap(heap)?))
        }
        _ => Ok(None),
    }
}

fn maybe_typed_list_arg(value: &RuntimeVal, heap: &HeapStore) -> Result<Option<TypedList>> {
    let RuntimeVal::Obj(handle) = value else {
        return Ok(None);
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(Some(list.clone())),
        _ => Ok(None),
    }
}

fn typed_list_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<TypedList> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a list");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::List(list) => Ok(list.clone()),
        _ => bail!("{context} expects a list"),
    }
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

fn typed_list_item(list: &TypedList, index: usize, heap: &mut HeapStore) -> Option<RuntimeVal> {
    match list {
        TypedList::Mixed(values) => values.get(index).cloned(),
        TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int),
        TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float),
        TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool),
        TypedList::String(values) => {
            let value = values.get(index)?;
            if let Some(short) = ShortStr::new(value) {
                Some(RuntimeVal::ShortStr(short))
            } else {
                Some(RuntimeVal::Obj(heap.alloc(HeapValue::String(value.clone()))))
            }
        }
    }
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
        TypedList::Mixed(values) => TypedList::Mixed(values[start..end].to_vec()),
        TypedList::Int(values) => TypedList::Int(values[start..end].to_vec()),
        TypedList::Float(values) => TypedList::Float(values[start..end].to_vec()),
        TypedList::Bool(values) => TypedList::Bool(values[start..end].to_vec()),
        TypedList::String(values) => TypedList::String(values[start..end].to_vec()),
    }
}

fn typed_list_concat_preserving_backing(left: TypedList, right: TypedList, heap: &mut HeapStore) -> TypedList {
    match (left, right) {
        (TypedList::Int(mut left), TypedList::Int(right)) => {
            left.extend(right);
            TypedList::Int(left)
        }
        (TypedList::Float(mut left), TypedList::Float(right)) => {
            left.extend(right);
            TypedList::Float(left)
        }
        (TypedList::Bool(mut left), TypedList::Bool(right)) => {
            left.extend(right);
            TypedList::Bool(left)
        }
        (TypedList::String(mut left), TypedList::String(right)) => {
            left.extend(right);
            TypedList::String(left)
        }
        (left, right) => {
            let mut values = typed_list_to_runtime_values(left, heap);
            values.extend(typed_list_to_runtime_values(right, heap));
            TypedList::Mixed(values)
        }
    }
}

fn flatten_typed_list(input: TypedList, heap: &mut HeapStore) -> Result<TypedList> {
    let TypedList::Mixed(values) = input else {
        return Ok(input);
    };
    let mut typed_out: Option<TypedList> = None;
    let mut mixed_out: Option<Vec<RuntimeVal>> = None;
    for value in values {
        if let Some(list) = maybe_typed_list_arg(&value, heap)? {
            if let Some(out) = mixed_out.as_mut() {
                out.extend(typed_list_to_runtime_values(list, heap));
            } else {
                typed_out = Some(match typed_out.take() {
                    Some(current) => typed_list_concat_preserving_backing(current, list, heap),
                    None => list,
                });
            }
        } else {
            let out = mixed_out.get_or_insert_with(|| {
                typed_out
                    .take()
                    .map(|list| typed_list_to_runtime_values(list, heap))
                    .unwrap_or_default()
            });
            out.push(value);
        }
    }
    Ok(match mixed_out {
        Some(values) => TypedList::from_runtime_values(values, heap),
        None => typed_out.unwrap_or_else(|| TypedList::Mixed(Vec::new())),
    })
}

fn unique_typed_list(input: TypedList, heap: &HeapStore) -> TypedList {
    match input {
        TypedList::Mixed(values) => unique_mixed_values(values, heap),
        TypedList::Int(values) => TypedList::Int(unique_copy_values(values)),
        TypedList::Float(values) => TypedList::Float(unique_copy_values(values)),
        TypedList::Bool(values) => TypedList::Bool(unique_copy_values(values)),
        TypedList::String(values) => TypedList::String(unique_arc_values(values)),
    }
}

fn unique_mixed_values(values: Vec<RuntimeVal>, heap: &HeapStore) -> TypedList {
    let mut out: Vec<RuntimeVal> = Vec::with_capacity(values.len());
    for value in values {
        if !out.iter().any(|existing| runtime_values_equal(existing, &value, heap)) {
            out.push(value);
        }
    }
    TypedList::from_runtime_values(out, heap)
}

fn unique_copy_values<T>(values: Vec<T>) -> Vec<T>
where
    T: Copy + PartialEq,
{
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

fn unique_arc_values(values: Vec<std::sync::Arc<str>>) -> Vec<std::sync::Arc<str>> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        if !out
            .iter()
            .any(|existing: &std::sync::Arc<str>| existing.as_ref() == value.as_ref())
        {
            out.push(value);
        }
    }
    out
}

fn typed_list_to_runtime_values(list: TypedList, heap: &mut HeapStore) -> Vec<RuntimeVal> {
    match list {
        TypedList::Mixed(values) => values,
        TypedList::Int(values) => values.into_iter().map(RuntimeVal::Int).collect(),
        TypedList::Float(values) => values.into_iter().map(RuntimeVal::Float).collect(),
        TypedList::Bool(values) => values.into_iter().map(RuntimeVal::Bool).collect(),
        TypedList::String(values) => values
            .into_iter()
            .map(|value| {
                if let Some(short) = ShortStr::new(&value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(heap.alloc(HeapValue::String(value)))
                }
            })
            .collect(),
    }
}

fn runtime_list(values: Vec<RuntimeVal>, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let list = TypedList::from_runtime_values(values, heap);
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
    runtime: &mut NativeRuntime32<'_>,
    context: &str,
) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = callable_value else {
        bail!("{context} must be callable");
    };
    let value = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        .clone();
    let HeapValue::Callable(callable) = value else {
        bail!("{context} must be callable");
    };

    match callable {
        CallableValue::Runtime32(function) => {
            let (heap, ctx) = runtime.heap_ctx_mut();
            call_runtime_callable32_runtime(function.as_ref(), NativeArgs32::new(args), heap, ctx)
        }
        CallableValue::Closure { .. } => {
            if let Some((state, ctx, module)) = runtime.state_ctx_module_mut() {
                return call_runtime_value32_runtime(callable_value.clone(), args, state, module, ctx);
            }
            bail!("{context} closure requires active RuntimeModuleState32")
        }
        CallableValue::RuntimeNative32 { arity, function } => {
            let entry = NativeEntry32 {
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
    entry: &NativeEntry32,
    args: &[RuntimeVal],
    runtime: &mut NativeRuntime32<'_>,
) -> Result<RuntimeVal> {
    match &entry.function {
        NativeFunction32::Plain(function)
        | NativeFunction32::Context(function)
        | NativeFunction32::FullState(function) => function(NativeArgs32::new(args), runtime),
        NativeFunction32::RuntimeCallable(function) => {
            let (heap, ctx) = runtime.heap_ctx_mut();
            call_runtime_callable32_runtime(function.as_ref(), NativeArgs32::new(args), heap, ctx)
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
        (HeapValue::Map(left), HeapValue::Map(right)) => {
            let left = left.entries();
            let right = right.entries();
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .iter()
                        .find(|(candidate, _)| candidate == key)
                        .is_some_and(|(_, right)| runtime_values_equal(left, right, heap))
                })
        }
        _ => false,
    }
}

fn runtime_lists_equal(left: &TypedList, right: &TypedList, heap: &HeapStore) -> bool {
    if let (TypedList::String(left), TypedList::String(right)) = (left, right) {
        return left == right;
    }
    let (Some(left), Some(right)) = (runtime_list_items(left), runtime_list_items(right)) else {
        return false;
    };
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| runtime_values_equal(left, right, heap))
}

fn runtime_list_items(list: &TypedList) -> Option<Vec<RuntimeVal>> {
    match list {
        TypedList::Mixed(values) => Some(values.clone()),
        TypedList::Int(values) => Some(values.iter().copied().map(RuntimeVal::Int).collect()),
        TypedList::Float(values) => Some(values.iter().copied().map(RuntimeVal::Float).collect()),
        TypedList::Bool(values) => Some(values.iter().copied().map(RuntimeVal::Bool).collect()),
        TypedList::String(values) => values
            .iter()
            .map(|value| lk_core::val::ShortStr::new(value).map(RuntimeVal::ShortStr))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register_stdlib_modules;
    use lk_core::{
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        vm::{NativeFunction32, Program32Result, RuntimeModuleState32, VmContext},
    };
    use std::sync::Arc;

    fn run32(source: &str) -> Result<Program32Result> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = lk_core::module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn run32_value(source: &str) -> Result<RuntimeVal> {
        Ok(run32(source)?.first_return().clone())
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

    fn expect_return_list(result: &Program32Result) -> Vec<RuntimeVal> {
        expect_list(result.first_return(), &result.state.heap)
    }

    fn iter_native(name: &str) -> Result<(u16, NativeFunction32)> {
        crate::runtime_native::runtime_native_export(&IterModule::new(), name)
    }

    #[test]
    fn iter_exports_use_runtime_native32_abi() -> Result<()> {
        for name in ["map", "filter", "reduce"] {
            let (_, function) = iter_native(name)?;
            assert!(matches!(function, NativeFunction32::FullState(_)));
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
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
        Ok(())
    }

    #[test]
    fn iter_sequence_ops_run_on_exec32() -> Result<()> {
        assert_eq!(
            expect_return_list(&run32("import iter; return iter.range(0, 6, 2);")?),
            vec![RuntimeVal::Int(0), RuntimeVal::Int(2), RuntimeVal::Int(4)]
        );
        let result = run32("import iter; return iter.zip([1,2], [\"a\",\"b\",\"c\"]);")?;
        let zipped = expect_return_list(&result);
        assert_eq!(zipped.len(), 2);
        assert_eq!(
            expect_list(&zipped[0], &result.state.heap),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::ShortStr(lk_core::val::ShortStr::new("a").expect("short"))
            ]
        );
        assert_eq!(
            expect_list(&zipped[1], &result.state.heap),
            vec![
                RuntimeVal::Int(2),
                RuntimeVal::ShortStr(lk_core::val::ShortStr::new("b").expect("short"))
            ]
        );
        assert_eq!(
            expect_return_list(&run32(
                "import iter; return iter.chain(iter.take([1,2,3], 2), iter.skip([4,5,6], 1));"
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
    fn iter_list_shape_ops_run_on_exec32() -> Result<()> {
        assert_eq!(
            expect_return_list(&run32(
                "import iter; let a = [1,2]; let b = [3]; let c = [4]; return iter.flatten([a,b,c]);"
            )?),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4)
            ]
        );
        assert_eq!(
            expect_return_list(&run32("import iter; return iter.unique([1,1,2,2,3]);")?),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2), RuntimeVal::Int(3)]
        );
        let result = run32("import iter; return iter.chunk([1,2,3,4,5], 2);")?;
        let chunks = expect_return_list(&result);
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            expect_list(&chunks[0], &result.state.heap),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2)]
        );
        assert_eq!(
            expect_list(&chunks[1], &result.state.heap),
            vec![RuntimeVal::Int(3), RuntimeVal::Int(4)]
        );
        assert_eq!(expect_list(&chunks[2], &result.state.heap), vec![RuntimeVal::Int(5)]);
        Ok(())
    }

    #[test]
    fn iter_higher_order_ops_call_runtime_closures() -> Result<()> {
        assert_eq!(
            expect_return_list(&run32("import iter; return iter.map([1,2,3], fn(x) => x * 2);")?),
            vec![RuntimeVal::Int(2), RuntimeVal::Int(4), RuntimeVal::Int(6)]
        );
        assert_eq!(
            expect_return_list(&run32(
                "import iter; return iter.filter([1,2,3,4], fn(x) => x % 2 == 0);"
            )?),
            vec![RuntimeVal::Int(2), RuntimeVal::Int(4)]
        );
        assert_eq!(
            run32_value("import iter; return iter.reduce([1,2,3], 0, fn(acc, x) => acc + x);")?,
            RuntimeVal::Int(6)
        );
        Ok(())
    }

    #[test]
    fn iter_direct_runtime_call_preserves_typed_lists() -> Result<()> {
        let (_, function) = iter_native("range")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("range must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let args = [RuntimeVal::Int(1), RuntimeVal::Int(4)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
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
            let NativeFunction32::Plain(function) = function else {
                panic!("{name} must use plain RuntimeNative32");
            };
            let mut state = RuntimeModuleState32::default();
            let list = state.heap.alloc(HeapValue::List(TypedList::String(vec![
                Arc::clone(&long),
                Arc::<str>::from("tail"),
            ])));
            let mut args = args;
            args[0] = RuntimeVal::Obj(list);
            let mut runtime = NativeRuntime32::new(&mut state, None, None);

            let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        let NativeFunction32::Plain(function) = function else {
            panic!("chain must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let left = state
            .heap
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-left-value",
            )])));
        let right = state
            .heap
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-right-value",
            )])));
        let args = [RuntimeVal::Obj(left), RuntimeVal::Obj(right)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        let NativeFunction32::Plain(function) = function else {
            panic!("chunk must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let input = state.heap.alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-one-value"),
            Arc::<str>::from("long-two-value"),
            Arc::<str>::from("long-three-value"),
        ])));
        let args = [RuntimeVal::Obj(input), RuntimeVal::Int(2)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        let NativeFunction32::Plain(function) = function else {
            panic!("zip must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let left = state.heap.alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-left-used"),
            Arc::<str>::from("long-left-unused"),
        ])));
        let right = state
            .heap
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-right-used",
            )])));
        let args = [RuntimeVal::Obj(left), RuntimeVal::Obj(right)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        let NativeFunction32::Plain(function) = function else {
            panic!("collect must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let input = state.heap.alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-collect-one"),
            Arc::<str>::from("long-collect-two"),
        ])));
        let args = [RuntimeVal::Obj(input)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        let NativeFunction32::Plain(function) = function else {
            panic!("flatten must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let first = state
            .heap
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-flatten-one",
            )])));
        let second = state
            .heap
            .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from(
                "long-flatten-two",
            )])));
        let outer = state.heap.alloc(HeapValue::List(TypedList::Mixed(vec![
            RuntimeVal::Obj(first),
            RuntimeVal::Obj(second),
        ])));
        let args = [RuntimeVal::Obj(outer)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        let NativeFunction32::Plain(function) = function else {
            panic!("unique must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let input = state.heap.alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-unique-one"),
            Arc::<str>::from("long-unique-one"),
            Arc::<str>::from("long-unique-two"),
        ])));
        let args = [RuntimeVal::Obj(input)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

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
        assert_eq!(
            run32_value("import iter; return iter.next([7,8]);")?,
            RuntimeVal::Int(7)
        );
        assert_eq!(run32_value("import iter; return iter.next([]);")?, RuntimeVal::Nil);
        assert_eq!(
            expect_return_list(&run32("import iter; return iter.collect([1,2]);")?),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2)]
        );
        Ok(())
    }
}
