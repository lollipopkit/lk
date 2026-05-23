use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap},
    vm::{NativeArgs32, NativeRuntime32, RuntimeExport32},
};

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
        let key = string_key_arg(&values[1], runtime.heap(), "has() key")?;
        Ok(RuntimeVal::Bool(map.get(&key).is_some()))
    }

    fn get32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "get()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "get() first argument")?;
        let key = string_key_arg(&values[1], runtime.heap(), "get() key")?;
        Ok(map.get_into_heap(&key, runtime.heap_mut())?.unwrap_or(RuntimeVal::Nil))
    }

    fn set32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 3, "set()")?;
        let values = args.as_slice();
        let mut map = map_arg(&values[0], runtime.heap(), "set() first argument")?;
        let key = string_key_arg(&values[1], runtime.heap(), "set() key")?;
        map.set(key, values[2].clone());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(map))))
    }

    fn delete32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "delete()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "delete() first argument")?;
        let key = string_key_arg(&values[1], runtime.heap(), "delete() key")?;
        let mut entries = BTreeMap::new();
        let mut removed = RuntimeVal::Nil;
        for (entry_key, value) in map.entries_into_heap(runtime.heap_mut())? {
            if entry_key == key {
                removed = value;
            } else {
                entries.insert(entry_key, value);
            }
        }
        let updated = RuntimeVal::Obj(
            runtime
                .heap_mut()
                .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
        );
        Ok(RuntimeVal::Obj(
            runtime
                .heap_mut()
                .alloc(HeapValue::List(TypedList::Mixed(vec![updated, removed]))),
        ))
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

fn one_map(args: NativeArgs32<'_>, runtime: &NativeRuntime32<'_>, name: &str) -> Result<TypedMap> {
    expect_arity(args, 1, name)?;
    map_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn map_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<TypedMap> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} argument must be a map");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match value {
        HeapValue::Map(map) => Ok(map.clone()),
        other => Err(anyhow!("{context} argument must be a map, got {}", other.type_name())),
    }
}

fn string_key_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<RuntimeMapKey> {
    let key = runtime_string_arg(value, heap, context)?;
    Ok(RuntimeMapKey::String(key))
}

fn map_keys_list(map: TypedMap) -> Vec<std::sync::Arc<str>> {
    match map {
        TypedMap::Mixed(entries) => entries.into_keys().filter_map(|key| key.as_arc_str()).collect(),
        TypedMap::StringMixed(entries) => entries.into_keys().collect(),
        TypedMap::StringInt(entries) => entries.into_keys().collect(),
        TypedMap::StringFloat(entries) => entries.into_keys().collect(),
        TypedMap::StringBool(entries) => entries.into_keys().collect(),
    }
}

fn map_values_list(map: TypedMap, heap: &HeapStore) -> TypedList {
    match map {
        TypedMap::Mixed(entries) => TypedList::from_runtime_values(entries.into_values().collect(), heap),
        TypedMap::StringMixed(entries) => TypedList::from_runtime_values(entries.into_values().collect(), heap),
        TypedMap::StringInt(entries) => TypedList::Int(entries.into_values().collect()),
        TypedMap::StringFloat(entries) => TypedList::Float(entries.into_values().collect()),
        TypedMap::StringBool(entries) => TypedList::Bool(entries.into_values().collect()),
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
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
        );
        let key = runtime_string_value("b", &mut state.heap);
        let args = [map, key, RuntimeVal::Int(2)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        let RuntimeVal::Obj(handle) = result else {
            panic!("set should return map object");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("set should preserve map in runtime heap");
        };
        assert_eq!(map.get_str("a"), Some(RuntimeVal::Int(1)));
        assert_eq!(map.get_str("b"), Some(RuntimeVal::Int(2)));
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
        let map = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(entries))));
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
    fn test_map_keys_reads_string_key_backing_directly() -> Result<()> {
        let (_, function) = map_native("keys")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("keys should use plain RuntimeNative32");
        };
        let mut entries = BTreeMap::new();
        entries.insert(Arc::<str>::from("long-key-one"), true);
        entries.insert(Arc::<str>::from("long-key-two"), false);
        let mut state = RuntimeModuleState32::default();
        let map = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringBool(entries))));
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
}
