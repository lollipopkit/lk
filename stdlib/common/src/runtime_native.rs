use anyhow::{Result, anyhow};
use lk_core::{
    val::{
        CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, ShortStr, TypedList, TypedMap, de,
    },
    vm::{NativeArgs, NativeRuntime},
};
use std::{fmt::Write as _, sync::Arc};

pub fn runtime_native_export(
    module: &dyn lk_core::module::ModuleProvider,
    name: &str,
) -> Result<(u16, lk_core::vm::NativeFunction)> {
    let export = module.runtime_exports()?;
    let state = export.state_lock()?;
    let RuntimeVal::Obj(handle) = export.value() else {
        return Err(anyhow!("module export must be a map"));
    };
    let Some(HeapValue::Map(map)) = state.heap().get(*handle) else {
        return Err(anyhow!("module export must be a map"));
    };
    let value = map.get_str(name).ok_or_else(|| anyhow!("{name} export present"))?;
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{name} must be a heap callable"));
    };
    let Some(HeapValue::Callable(lk_core::val::CallableValue::RuntimeNative { arity, function, .. })) =
        state.heap().get(handle)
    else {
        return Err(anyhow!("{name} must be RuntimeNative"));
    };
    Ok((*arity, function.clone()))
}

pub fn parse_format(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
    name: &str,
    format: de::Format,
) -> Result<RuntimeVal> {
    if args.len() != 1 {
        return Err(anyhow!("{name}(data) requires 1 argument"));
    }
    let data = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), name)?;
    parse_runtime_with_format(data.as_ref(), format, runtime.heap_mut())
}

fn parse_runtime_with_format(input: &str, format: de::Format, heap: &mut HeapStore) -> Result<RuntimeVal> {
    de::parse_runtime_with_format_into_heap(input, format, heap)
}

pub fn runtime_string_arg(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<Arc<str>> {
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

pub fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

pub fn runtime_display_value(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    match value {
        RuntimeVal::Nil => Ok("nil".to_string()),
        RuntimeVal::Bool(value) => Ok(value.to_string()),
        RuntimeVal::Int(value) => Ok(value.to_string()),
        RuntimeVal::Float(value) => Ok(value.to_string()),
        RuntimeVal::ShortStr(value) => Ok(value.as_str().to_string()),
        RuntimeVal::Obj(handle) => {
            let value = heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            runtime_display_heap_value(value, heap)
        }
    }
}

fn runtime_display_heap_value(value: &HeapValue, heap: &HeapStore) -> Result<String> {
    match value {
        HeapValue::String(value) => Ok(value.to_string()),
        HeapValue::Bytes(value) => Ok(format!("<Bytes {} bytes>", value.len())),
        HeapValue::List(values) => runtime_display_list(values, heap),
        HeapValue::Map(values) => runtime_display_map(values, heap),
        HeapValue::Set(values) => runtime_display_set(values),
        HeapValue::Callable(value) => Ok(runtime_display_callable(value)),
        HeapValue::Object(value) => {
            let mut out = value.type_name.to_string();
            append_display_entries(
                &mut out,
                value
                    .fields
                    .iter()
                    .map(|(key, value)| Ok((key.to_string(), runtime_display_value(value, heap)?))),
            )?;
            Ok(out)
        }
        other => Ok(format!("<{}>", other.type_name())),
    }
}

fn runtime_display_set(values: &RuntimeSet) -> Result<String> {
    let mut out = String::from("Set(");
    out.push('[');
    let mut first = true;
    for key in values.entries() {
        push_display_sep(&mut out, &mut first);
        out.push_str(&runtime_display_map_key(key));
    }
    out.push(']');
    out.push(')');
    Ok(out)
}

fn runtime_display_callable(value: &CallableValue) -> String {
    match value {
        CallableValue::Closure {
            function_index,
            captures,
        } => format!("<fn #{}({} captures)>", function_index, captures.len()),
        CallableValue::RuntimeNative { name, arity, .. } => {
            if *arity == lk_core::vm::NativeEntry::VARIADIC {
                format!("<native fn {}(...)>", name)
            } else {
                format!("<native fn {}({} args)>", name, arity)
            }
        }
        CallableValue::Runtime(function) => {
            format!(
                "<fn {} ({} captures)>",
                function.display_signature(),
                function.capture_count()
            )
        }
    }
}

fn runtime_display_list(values: &TypedList, heap: &HeapStore) -> Result<String> {
    let mut out = String::from("[");
    let mut first = true;
    match values {
        TypedList::Mixed(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                out.push_str(&runtime_display_value(value, heap)?);
            }
        }
        TypedList::Int(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                write!(&mut out, "{value}").expect("write to String cannot fail");
            }
        }
        TypedList::Float(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                write!(&mut out, "{value}").expect("write to String cannot fail");
            }
        }
        TypedList::Bool(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                write!(&mut out, "{value}").expect("write to String cannot fail");
            }
        }
        TypedList::String(values) => {
            for value in values {
                push_display_sep(&mut out, &mut first);
                out.push_str(&quote_string(value));
            }
        }
    }
    out.push(']');
    Ok(out)
}

fn runtime_display_map(values: &TypedMap, heap: &HeapStore) -> Result<String> {
    let mut out = String::new();
    match values {
        TypedMap::Mixed(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((runtime_display_map_key(key), runtime_display_value(value, heap)?))),
        )?,
        TypedMap::StringMixed(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), runtime_display_value(value, heap)?))),
        )?,
        TypedMap::StringInt(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), value.to_string()))),
        )?,
        TypedMap::StringFloat(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), value.to_string()))),
        )?,
        TypedMap::StringBool(entries) => append_display_entries(
            &mut out,
            entries
                .iter()
                .map(|(key, value)| Ok((quote_string(key), value.to_string()))),
        )?,
    }
    Ok(out)
}

fn runtime_display_map_key(key: &RuntimeMapKey) -> String {
    match key {
        RuntimeMapKey::Nil => "nil".to_string(),
        RuntimeMapKey::Bool(value) => value.to_string(),
        RuntimeMapKey::Int(value) => value.to_string(),
        RuntimeMapKey::ShortStr(value) => quote_string(value.as_str()),
        RuntimeMapKey::String(value) => quote_string(value),
        RuntimeMapKey::Obj(value) => format!("<object:{}>", value.index()),
    }
}

fn append_display_entries(out: &mut String, entries: impl IntoIterator<Item = Result<(String, String)>>) -> Result<()> {
    out.push('{');
    let mut first = true;
    for entry in entries {
        let (key, value) = entry?;
        push_display_sep(out, &mut first);
        out.push_str(&key);
        out.push(':');
        out.push_str(&value);
    }
    out.push('}');
    Ok(())
}

fn push_display_sep(out: &mut String, first: &mut bool) {
    if *first {
        *first = false;
    } else {
        out.push(',');
    }
}

fn quote_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use lk_core::util::fast_map::fast_hash_map_from_iter;
    use std::sync::Arc;

    use super::*;
    use lk_core::val::TypedMap;

    #[test]
    fn runtime_display_formats_typed_containers_without_val_containers() {
        let mut heap = HeapStore::new();
        let nested = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2]))));
        let map = RuntimeVal::Obj(
            heap.alloc(HeapValue::Map(TypedMap::StringMixed(fast_hash_map_from_iter([
                (Arc::<str>::from("items"), nested),
                (Arc::<str>::from("ok"), RuntimeVal::Bool(true)),
            ])))),
        );

        let output = runtime_display_value(&map, &heap).expect("display");

        assert!(output.contains("\"items\":[1,2]"));
        assert!(output.contains("\"ok\":true"));
    }
}
