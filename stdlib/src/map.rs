use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap},
    vm::{NativeArgs32, NativeRuntime32, RuntimeExport32},
};
use std::{collections::BTreeMap, sync::Arc};

use crate::runtime_native::runtime_string_arg;

#[derive(Debug)]
pub struct MapModule;

impl Default for MapModule {
    fn default() -> Self {
        Self::new()
    }
}

impl MapModule {
    pub fn new() -> Self {
        Self
    }

    fn len32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let map = one_map(args, runtime, "len()")?;
        Ok(RuntimeVal::Int(map.len() as i64))
    }

    fn keys32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let map = one_map(args, runtime, "keys()")?;
        let keys = map_keys_list(map);
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(keys))),
        ))
    }

    fn values32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let map = one_map(args, runtime, "values()")?;
        let list = map_values_list(map, runtime.heap());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
    }

    fn has32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "has()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "has() first argument")?;
        let key = runtime_string_arg(&values[1], runtime.heap(), "has() key")?;
        Ok(RuntimeVal::Bool(map.get(&RuntimeMapKey::String(key)).is_some()))
    }

    fn get32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "get()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "get() first argument")?;
        let key = runtime_string_arg(&values[1], runtime.heap(), "get() key")?;
        Ok(map.get(&RuntimeMapKey::String(key)).unwrap_or(RuntimeVal::Nil))
    }

    fn set32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 3, "set()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "set() first argument")?;
        let key = runtime_string_arg(&values[1], runtime.heap(), "set() key")?;
        let map = set_map_entry(map, key, values[2].clone());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(map))))
    }

    fn delete32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "delete()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "delete() first argument")?;
        let key = runtime_string_arg(&values[1], runtime.heap(), "delete() key")?;
        let (map, removed) = delete_map_entry(map, key.as_ref());
        let updated = RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(map)));
        Ok(RuntimeVal::Obj(
            runtime
                .heap_mut()
                .alloc(HeapValue::List(TypedList::Mixed(vec![updated, removed]))),
        ))
    }
}

fn set_map_entry(map: &TypedMap, key: Arc<str>, value: RuntimeVal) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = BTreeMap::new();
            let inserted_key = RuntimeMapKey::String(Arc::clone(&key));
            for (entry_key, entry_value) in entries {
                if *entry_key != inserted_key {
                    out.insert(entry_key.clone(), entry_value.clone());
                }
            }
            let mut entries = out;
            entries.insert(RuntimeMapKey::String(key), value);
            TypedMap::Mixed(entries)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = BTreeMap::new();
            for (entry_key, entry_value) in entries.iter() {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), entry_value.clone());
                }
            }
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(entries) => match value {
            RuntimeVal::Int(value) => {
                let mut out = BTreeMap::new();
                for (entry_key, entry_value) in entries.iter() {
                    if entry_key.as_ref() != key.as_ref() {
                        out.insert(Arc::clone(entry_key), *entry_value);
                    }
                }
                out.insert(key, value);
                TypedMap::StringInt(out)
            }
            value => {
                let mut out = BTreeMap::new();
                for (entry_key, entry_value) in entries.iter() {
                    if entry_key.as_ref() != key.as_ref() {
                        out.insert(Arc::clone(entry_key), RuntimeVal::Int(*entry_value));
                    }
                }
                out.insert(key, value);
                TypedMap::StringMixed(out)
            }
        },
        TypedMap::StringFloat(entries) => match value {
            RuntimeVal::Float(value) => {
                let mut out = BTreeMap::new();
                for (entry_key, entry_value) in entries.iter() {
                    if entry_key.as_ref() != key.as_ref() {
                        out.insert(Arc::clone(entry_key), *entry_value);
                    }
                }
                out.insert(key, value);
                TypedMap::StringFloat(out)
            }
            value => {
                let mut out = BTreeMap::new();
                for (entry_key, entry_value) in entries.iter() {
                    if entry_key.as_ref() != key.as_ref() {
                        out.insert(Arc::clone(entry_key), RuntimeVal::Float(*entry_value));
                    }
                }
                out.insert(key, value);
                TypedMap::StringMixed(out)
            }
        },
        TypedMap::StringBool(entries) => match value {
            RuntimeVal::Bool(value) => {
                let mut out = BTreeMap::new();
                for (entry_key, entry_value) in entries.iter() {
                    if entry_key.as_ref() != key.as_ref() {
                        out.insert(Arc::clone(entry_key), *entry_value);
                    }
                }
                out.insert(key, value);
                TypedMap::StringBool(out)
            }
            value => {
                let mut out = BTreeMap::new();
                for (entry_key, entry_value) in entries.iter() {
                    if entry_key.as_ref() != key.as_ref() {
                        out.insert(Arc::clone(entry_key), RuntimeVal::Bool(*entry_value));
                    }
                }
                out.insert(key, value);
                TypedMap::StringMixed(out)
            }
        },
    }
}

impl Module for MapModule {
    fn name(&self) -> &str {
        "map"
    }

    fn description(&self) -> &str {
        "Map utilities"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("len", Self::len32, 1),
                RuntimeNativeExport32::plain("keys", Self::keys32, 1),
                RuntimeNativeExport32::plain("values", Self::values32, 1),
                RuntimeNativeExport32::plain("has", Self::has32, 2),
                RuntimeNativeExport32::plain("get", Self::get32, 2),
                RuntimeNativeExport32::plain("set", Self::set32, 3),
                RuntimeNativeExport32::plain("delete", Self::delete32, 2),
            ],
            &[],
        ))
    }
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} takes exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
    }
}

fn one_map<'a>(args: NativeArgs32<'a>, runtime: &'a NativeRuntime32<'a>, name: &str) -> Result<&'a TypedMap> {
    expect_arity(args, 1, name)?;
    map_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn map_arg<'a>(value: &RuntimeVal, heap: &'a HeapStore, context: &str) -> Result<&'a TypedMap> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} argument must be a map");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::Map(map) => Ok(map),
        other => Err(anyhow!("{context} argument must be a map, got {}", other.type_name())),
    }
}

fn map_keys_list(map: &TypedMap) -> Vec<std::sync::Arc<str>> {
    match map {
        TypedMap::Mixed(entries) => {
            let mut keys = Vec::new();
            for key in entries.keys() {
                if let Some(key) = key.as_arc_str() {
                    keys.push(key);
                }
            }
            keys
        }
        TypedMap::StringMixed(entries) => copy_string_map_keys(entries),
        TypedMap::StringInt(entries) => copy_string_map_keys(entries),
        TypedMap::StringFloat(entries) => copy_string_map_keys(entries),
        TypedMap::StringBool(entries) => copy_string_map_keys(entries),
    }
}

fn map_values_list(map: &TypedMap, heap: &HeapStore) -> TypedList {
    match map {
        TypedMap::Mixed(entries) => map_runtime_values_to_list(entries.values(), heap),
        TypedMap::StringMixed(entries) => map_runtime_values_to_list(entries.values(), heap),
        TypedMap::StringInt(entries) => {
            let mut values = Vec::with_capacity(entries.len());
            for value in entries.values() {
                values.push(*value);
            }
            TypedList::Int(values)
        }
        TypedMap::StringFloat(entries) => {
            let mut values = Vec::with_capacity(entries.len());
            for value in entries.values() {
                values.push(*value);
            }
            TypedList::Float(values)
        }
        TypedMap::StringBool(entries) => {
            let mut values = Vec::with_capacity(entries.len());
            for value in entries.values() {
                values.push(*value);
            }
            TypedList::Bool(values)
        }
    }
}

fn copy_string_map_keys<T>(entries: &BTreeMap<Arc<str>, T>) -> Vec<Arc<str>> {
    let mut keys = Vec::with_capacity(entries.len());
    for key in entries.keys() {
        keys.push(Arc::clone(key));
    }
    keys
}

enum RuntimeValueListShape {
    Empty,
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<MapStringValue>),
    Mixed(Vec<RuntimeVal>),
}

enum MapStringValue {
    Short(ShortStr),
    Heap { handle: HeapRef, value: Arc<str> },
}

impl MapStringValue {
    fn into_arc(self) -> Arc<str> {
        match self {
            Self::Short(value) => Arc::<str>::from(value.as_str()),
            Self::Heap { value, .. } => value,
        }
    }

    fn into_runtime(self) -> RuntimeVal {
        match self {
            Self::Short(value) => RuntimeVal::ShortStr(value),
            Self::Heap { handle, .. } => RuntimeVal::Obj(handle),
        }
    }
}

fn map_runtime_values_to_list<'a>(values: impl IntoIterator<Item = &'a RuntimeVal>, heap: &HeapStore) -> TypedList {
    let mut shape = RuntimeValueListShape::Empty;
    for value in values {
        shape = append_map_value_list_shape(shape, value, heap);
    }
    match shape {
        RuntimeValueListShape::Empty => TypedList::Mixed(Vec::new()),
        RuntimeValueListShape::Int(values) => TypedList::Int(values),
        RuntimeValueListShape::Float(values) => TypedList::Float(values),
        RuntimeValueListShape::Bool(values) => TypedList::Bool(values),
        RuntimeValueListShape::String(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(value.into_arc());
            }
            TypedList::String(out)
        }
        RuntimeValueListShape::Mixed(values) => TypedList::Mixed(values),
    }
}

fn append_map_value_list_shape(
    shape: RuntimeValueListShape,
    value: &RuntimeVal,
    heap: &HeapStore,
) -> RuntimeValueListShape {
    match (shape, value) {
        (RuntimeValueListShape::Empty, RuntimeVal::Int(value)) => RuntimeValueListShape::Int(vec![*value]),
        (RuntimeValueListShape::Empty, RuntimeVal::Float(value)) => RuntimeValueListShape::Float(vec![*value]),
        (RuntimeValueListShape::Empty, RuntimeVal::Bool(value)) => RuntimeValueListShape::Bool(vec![*value]),
        (RuntimeValueListShape::Empty, value) => match runtime_string_from_map_value(value, heap) {
            Some(string) => RuntimeValueListShape::String(vec![string]),
            None => RuntimeValueListShape::Mixed(vec![value.clone()]),
        },
        (RuntimeValueListShape::Int(mut values), RuntimeVal::Int(value)) => {
            values.push(*value);
            RuntimeValueListShape::Int(values)
        }
        (RuntimeValueListShape::Float(mut values), RuntimeVal::Float(value)) => {
            values.push(*value);
            RuntimeValueListShape::Float(values)
        }
        (RuntimeValueListShape::Bool(mut values), RuntimeVal::Bool(value)) => {
            values.push(*value);
            RuntimeValueListShape::Bool(values)
        }
        (RuntimeValueListShape::String(mut values), value) => match runtime_string_from_map_value(value, heap) {
            Some(string) => {
                values.push(string);
                RuntimeValueListShape::String(values)
            }
            None => {
                let mut mixed = string_values_to_mixed(values, 1);
                mixed.push(value.clone());
                RuntimeValueListShape::Mixed(mixed)
            }
        },
        (RuntimeValueListShape::Mixed(mut values), value) => {
            values.push(value.clone());
            RuntimeValueListShape::Mixed(values)
        }
        (RuntimeValueListShape::Int(values), value) => {
            let mut mixed = int_values_to_mixed(values, 1);
            mixed.push(value.clone());
            RuntimeValueListShape::Mixed(mixed)
        }
        (RuntimeValueListShape::Float(values), value) => {
            let mut mixed = float_values_to_mixed(values, 1);
            mixed.push(value.clone());
            RuntimeValueListShape::Mixed(mixed)
        }
        (RuntimeValueListShape::Bool(values), value) => {
            let mut mixed = bool_values_to_mixed(values, 1);
            mixed.push(value.clone());
            RuntimeValueListShape::Mixed(mixed)
        }
    }
}

fn string_values_to_mixed(values: Vec<MapStringValue>, extra: usize) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + extra);
    for value in values {
        mixed.push(value.into_runtime());
    }
    mixed
}

fn int_values_to_mixed(values: Vec<i64>, extra: usize) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + extra);
    for value in values {
        mixed.push(RuntimeVal::Int(value));
    }
    mixed
}

fn float_values_to_mixed(values: Vec<f64>, extra: usize) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + extra);
    for value in values {
        mixed.push(RuntimeVal::Float(value));
    }
    mixed
}

fn bool_values_to_mixed(values: Vec<bool>, extra: usize) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + extra);
    for value in values {
        mixed.push(RuntimeVal::Bool(value));
    }
    mixed
}

fn runtime_string_from_map_value(value: &RuntimeVal, heap: &HeapStore) -> Option<MapStringValue> {
    match value {
        RuntimeVal::ShortStr(value) => Some(MapStringValue::Short(*value)),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => Some(MapStringValue::Heap {
                handle: *handle,
                value: Arc::clone(value),
            }),
            _ => None,
        },
        _ => None,
    }
}

fn delete_map_entry(map: &TypedMap, key: &str) -> (TypedMap, RuntimeVal) {
    match map {
        TypedMap::Mixed(entries) => {
            let removed_key = RuntimeMapKey::String(Arc::<str>::from(key));
            let mut removed = RuntimeVal::Nil;
            let mut out = BTreeMap::new();
            for (entry_key, value) in entries {
                if *entry_key == removed_key {
                    removed = value.clone();
                } else {
                    out.insert(entry_key.clone(), value.clone());
                }
            }
            (TypedMap::Mixed(out), removed)
        }
        TypedMap::StringMixed(entries) => {
            let mut removed = RuntimeVal::Nil;
            let mut out = BTreeMap::new();
            for (entry_key, value) in entries {
                if entry_key.as_ref() == key {
                    removed = value.clone();
                } else {
                    out.insert(Arc::clone(entry_key), value.clone());
                }
            }
            (TypedMap::StringMixed(out), removed)
        }
        TypedMap::StringInt(entries) => {
            let mut removed = RuntimeVal::Nil;
            let mut out = BTreeMap::new();
            for (entry_key, value) in entries {
                if entry_key.as_ref() == key {
                    removed = RuntimeVal::Int(*value);
                } else {
                    out.insert(Arc::clone(entry_key), *value);
                }
            }
            (TypedMap::StringInt(out), removed)
        }
        TypedMap::StringFloat(entries) => {
            let mut removed = RuntimeVal::Nil;
            let mut out = BTreeMap::new();
            for (entry_key, value) in entries {
                if entry_key.as_ref() == key {
                    removed = RuntimeVal::Float(*value);
                } else {
                    out.insert(Arc::clone(entry_key), *value);
                }
            }
            (TypedMap::StringFloat(out), removed)
        }
        TypedMap::StringBool(entries) => {
            let mut removed = RuntimeVal::Nil;
            let mut out = BTreeMap::new();
            for (entry_key, value) in entries {
                if entry_key.as_ref() == key {
                    removed = RuntimeVal::Bool(*value);
                } else {
                    out.insert(Arc::clone(entry_key), *value);
                }
            }
            (TypedMap::StringBool(out), removed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register_stdlib_modules;
    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, Program32Result, RuntimeModuleState32, VmContext},
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::runtime_native::runtime_string_value;

    fn run32(source: &str) -> Result<Program32Result> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn run32_return(source: &str) -> Result<RuntimeVal> {
        Ok(run32(source)?.first_return().clone())
    }

    fn runtime_short_string(value: &str) -> RuntimeVal {
        RuntimeVal::ShortStr(ShortStr::new(value).expect("short test string"))
    }

    fn expect_list(result: &Program32Result) -> Vec<RuntimeVal> {
        match result.first_return_list().expect("expected list") {
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

    fn map_native(name: &str) -> Result<(u16, NativeFunction32)> {
        crate::runtime_native::runtime_native_export(&MapModule::new(), name)
    }

    #[test]
    fn test_map_len_keys_values_has_get() -> Result<()> {
        assert_eq!(
            run32_return("import map; return map.len({\"a\":1, \"b\":2});")?,
            RuntimeVal::Int(2)
        );

        let keys = run32_return(
            "import map; import string; let m={\"a\":1, \"b\":2}; return string.join(map.keys(m), \",\");",
        )?;
        match keys {
            RuntimeVal::ShortStr(v) if v.as_str() == "a,b" || v.as_str() == "b,a" => {}
            _ => panic!("unexpected keys output: {:?}", keys),
        }

        assert_eq!(
            run32_return("import map; return map.has({\"a\":1}, \"a\");")?,
            RuntimeVal::Bool(true)
        );
        assert_eq!(
            run32_return("import map; return map.has({\"a\":1}, \"b\");")?,
            RuntimeVal::Bool(false)
        );
        assert_eq!(
            run32_return("import map; return map.get({\"a\":1}, \"a\");")?,
            RuntimeVal::Int(1)
        );
        assert_eq!(
            run32_return("import map; return map.get({\"a\":1}, \"b\");")?,
            RuntimeVal::Nil
        );

        let values = run32("import map; return map.values({\"a\":1, \"b\":2});")?;
        let values = expect_list(&values);
        assert_eq!(values.len(), 2);
        assert!(values.contains(&RuntimeVal::Int(1)));
        assert!(values.contains(&RuntimeVal::Int(2)));
        Ok(())
    }

    #[test]
    fn test_map_set_and_delete() -> Result<()> {
        let result = run32(
            r#"
            import map;
            let updated = map.set({"a": 1}, "a", 7);
            let removed_pair = map.delete(updated, "a");
            let without = removed_pair[0];
            let removed = removed_pair[1];
            return [removed, map.has(without, "a")];
        "#,
        )?;
        let values = expect_list(&result);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], RuntimeVal::Int(7));
        assert_eq!(values[1], RuntimeVal::Bool(false));
        Ok(())
    }

    #[test]
    fn test_map_public_functions_use_runtime_native32_abi() -> Result<()> {
        for name in ["len", "keys", "values", "has", "get", "set", "delete"] {
            let (arity, function) = map_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry32::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn test_map_direct_runtime_call_preserves_typed_map() -> Result<()> {
        let (_, function) = map_native("set")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("set should use plain RuntimeNative32");
        };
        let mut entries = BTreeMap::new();
        entries.insert(RuntimeMapKey::String(Arc::<str>::from("a")), RuntimeVal::Int(1));
        let mut state = RuntimeModuleState32::default();
        let map = RuntimeVal::Obj(
            state.heap_mut().alloc(HeapValue::Map(TypedMap::StringInt(
                entries
                    .into_iter()
                    .map(|(key, value)| {
                        let RuntimeMapKey::String(key) = key else {
                            panic!("test map key must be a string");
                        };
                        let RuntimeVal::Int(value) = value else {
                            panic!("test map value must be an int");
                        };
                        (key, value)
                    })
                    .collect(),
            ))),
        );
        let key = runtime_string_value("a", state.heap_mut());
        let args = [map, key, RuntimeVal::Int(7)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        let RuntimeVal::Obj(handle) = result else {
            panic!("set should return map object");
        };
        let Some(HeapValue::Map(TypedMap::StringInt(map))) = runtime.heap().get(handle) else {
            panic!("set should preserve typed int map backing");
        };
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a"), Some(&7));
        Ok(())
    }

    #[test]
    fn test_map_values_reads_typed_map_backing_directly() -> Result<()> {
        let (_, function) = map_native("values")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("values should use plain RuntimeNative32");
        };
        let mut entries = BTreeMap::new();
        entries.insert(Arc::<str>::from("a"), 1);
        entries.insert(Arc::<str>::from("b"), 2);
        let mut state = RuntimeModuleState32::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringInt(entries))));
        let args = [map];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("values should return list object");
        };
        let Some(HeapValue::List(TypedList::Int(values))) = runtime.heap().get(handle) else {
            panic!("values should preserve typed int list backing");
        };
        assert_eq!(values, &vec![1, 2]);
        assert_eq!(runtime.heap().len(), 2);
        Ok(())
    }

    #[test]
    fn test_map_values_classifies_string_mixed_backing_without_runtime_value_helper() -> Result<()> {
        let (_, function) = map_native("values")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("values should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let heap_string = runtime_string_value("long-map-value", state.heap_mut());
        let mut entries = BTreeMap::new();
        entries.insert(Arc::<str>::from("a"), runtime_string_value("short", state.heap_mut()));
        entries.insert(Arc::<str>::from("b"), heap_string);
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
        let args = [map];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("values should return list object");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("values should classify all string values into typed string backing");
        };
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|value| value.as_ref() == "short"));
        assert!(values.iter().any(|value| value.as_ref() == "long-map-value"));
        assert_eq!(runtime.heap().len(), 3);
        Ok(())
    }

    #[test]
    fn test_map_values_pollutes_numeric_shape_to_mixed_without_reclassifying() -> Result<()> {
        let (_, function) = map_native("values")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("values should use plain RuntimeNative32");
        };
        let mut entries = BTreeMap::new();
        entries.insert(Arc::<str>::from("a"), RuntimeVal::Int(1));
        entries.insert(Arc::<str>::from("b"), runtime_short_string("two"));
        let mut state = RuntimeModuleState32::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
        let args = [map];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("values should return list object");
        };
        let Some(HeapValue::List(TypedList::Mixed(values))) = runtime.heap().get(handle) else {
            panic!("mixed map values should pollute numeric shape to mixed list backing");
        };
        assert_eq!(values, &vec![RuntimeVal::Int(1), runtime_short_string("two")]);
        assert_eq!(runtime.heap().len(), 2);
        Ok(())
    }

    #[test]
    fn test_map_keys_reads_string_key_backing_directly() -> Result<()> {
        let (_, function) = map_native("keys")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("keys should use plain RuntimeNative32");
        };
        let mut entries = BTreeMap::new();
        entries.insert(Arc::<str>::from("long-key-one"), true);
        entries.insert(Arc::<str>::from("long-key-two"), false);
        let mut state = RuntimeModuleState32::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringBool(entries))));
        let args = [map];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("keys should return list object");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("keys should preserve typed string list backing");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(runtime.heap().len(), 2);
        Ok(())
    }

    #[test]
    fn test_map_delete_preserves_typed_map_backing_directly() -> Result<()> {
        let (_, function) = map_native("delete")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("delete should use plain RuntimeNative32");
        };
        let mut entries = BTreeMap::new();
        entries.insert(Arc::<str>::from("a"), 1);
        entries.insert(Arc::<str>::from("b"), 2);
        let mut state = RuntimeModuleState32::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringInt(entries))));
        let key = runtime_string_value("b", state.heap_mut());
        let args = [map, key];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = function(NativeArgs32::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(pair_handle) = result else {
            panic!("delete should return pair list");
        };
        let Some(HeapValue::List(TypedList::Mixed(pair))) = runtime.heap().get(pair_handle) else {
            panic!("delete should return mixed pair list");
        };
        let [RuntimeVal::Obj(updated_handle), RuntimeVal::Int(removed)] = pair.as_slice() else {
            panic!("delete should return updated map and removed int");
        };
        let Some(HeapValue::Map(TypedMap::StringInt(updated))) = runtime.heap().get(*updated_handle) else {
            panic!("delete should preserve typed int map backing");
        };
        assert_eq!(*removed, 2);
        assert_eq!(updated.get("a"), Some(&1));
        assert_eq!(updated.get("b"), None);
        assert_eq!(runtime.heap().len(), 3);
        Ok(())
    }
}
