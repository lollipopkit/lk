use anyhow::{Result, anyhow};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap, de},
    vm::{NativeArgs32, NativeRuntime32},
};
use std::sync::Arc;

#[cfg(test)]
pub(crate) fn runtime_native_export(
    module: &dyn lk_core::module::Module,
    name: &str,
) -> Result<(u16, lk_core::vm::NativeFunction32)> {
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
    let Some(HeapValue::Callable(lk_core::val::CallableValue::RuntimeNative32 { arity, function })) =
        state.heap().get(handle)
    else {
        return Err(anyhow!("{name} must be RuntimeNative32"));
    };
    Ok((*arity, function.clone()))
}

pub(crate) fn parse_format32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
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

pub(crate) fn runtime_display_value(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
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
        HeapValue::List(values) => runtime_display_list(values, heap),
        HeapValue::Map(values) => runtime_display_map(values, heap),
        HeapValue::Object(value) => {
            let fields = value
                .fields
                .iter()
                .map(|(key, value)| Ok((key.to_string(), runtime_display_value(value, heap)?)))
                .collect::<Result<Vec<_>>>()?;
            Ok(format!("{}{}", value.type_name, display_entries(fields)))
        }
        other => Ok(format!("<{}>", other.type_name())),
    }
}

fn runtime_display_list(values: &TypedList, heap: &HeapStore) -> Result<String> {
    let values = match values {
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| runtime_display_value(value, heap))
            .collect::<Result<Vec<_>>>()?,
        TypedList::Int(values) => values.iter().map(ToString::to_string).collect(),
        TypedList::Float(values) => values.iter().map(ToString::to_string).collect(),
        TypedList::Bool(values) => values.iter().map(ToString::to_string).collect(),
        TypedList::String(values) => values.iter().map(|value| quote_string(value)).collect(),
    };
    Ok(format!("[{}]", values.join(",")))
}

fn runtime_display_map(values: &TypedMap, heap: &HeapStore) -> Result<String> {
    let entries = runtime_display_map_entries(values, heap)?;
    Ok(display_entries(entries))
}

fn runtime_display_map_entries(values: &TypedMap, heap: &HeapStore) -> Result<Vec<(String, String)>> {
    match values {
        TypedMap::Mixed(entries) => entries
            .iter()
            .map(|(key, value)| Ok((runtime_display_map_key(key), runtime_display_value(value, heap)?)))
            .collect(),
        TypedMap::StringMixed(entries) => entries
            .iter()
            .map(|(key, value)| Ok((quote_string(key), runtime_display_value(value, heap)?)))
            .collect(),
        TypedMap::StringInt(entries) => Ok(entries
            .iter()
            .map(|(key, value)| (quote_string(key), value.to_string()))
            .collect()),
        TypedMap::StringFloat(entries) => Ok(entries
            .iter()
            .map(|(key, value)| (quote_string(key), value.to_string()))
            .collect()),
        TypedMap::StringBool(entries) => Ok(entries
            .iter()
            .map(|(key, value)| (quote_string(key), value.to_string()))
            .collect()),
    }
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

fn display_entries(entries: Vec<(String, String)>) -> String {
    let body = entries
        .into_iter()
        .map(|(key, value)| format!("{key}:{value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

fn quote_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use super::*;
    use lk_core::val::TypedMap;

    #[test]
    fn runtime_display_formats_typed_containers_without_val_containers() {
        let mut heap = HeapStore::new();
        let nested = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2]))));
        let map = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(BTreeMap::from([
            (Arc::<str>::from("items"), nested),
            (Arc::<str>::from("ok"), RuntimeVal::Bool(true)),
        ])))));

        let output = runtime_display_value(&map, &heap).expect("display");

        assert!(output.contains("\"items\":[1,2]"));
        assert!(output.contains("\"ok\":true"));
    }
}
