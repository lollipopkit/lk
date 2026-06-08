use anyhow::{Result, anyhow, bail};
use lk_core::util::fast_map::FastHashMap;
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::{HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap},
    vm::{NativeArgs, NativeRuntime, RuntimeExport, call_runtime_value_runtime},
};
use std::sync::Arc;

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

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

    fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let map = one_map(args, runtime, "len()")?;
        Ok(RuntimeVal::Int(map.len() as i64))
    }

    fn keys(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let map = one_map(args, runtime, "keys()")?.clone();
        let keys = map_keys_list(&map, runtime.heap_mut());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(keys))))
    }

    fn values(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let map = one_map(args, runtime, "values()")?;
        let list = map_values_list(map, runtime.heap());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
    }

    fn has(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "has()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "has() first argument")?;
        let key = runtime_map_key_arg(&values[1], runtime.heap(), "has() key")?;
        Ok(RuntimeVal::Bool(map.get(&key).is_some()))
    }

    fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "get()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "get() first argument")?;
        let key = runtime_map_key_arg(&values[1], runtime.heap(), "get() key")?;
        Ok(map.get(&key).unwrap_or(RuntimeVal::Nil))
    }

    fn set(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 3, "set()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "set() first argument")?;
        let key = runtime_map_key_arg(&values[1], runtime.heap(), "set() key")?;
        let map = set_map_entry(map, key, values[2].clone());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(map))))
    }

    fn delete(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "delete()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "delete() first argument")?;
        let key = runtime_map_key_arg(&values[1], runtime.heap(), "delete() key")?;
        let (map, removed) = delete_map_entry(map, &key);
        let updated = RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(map)));
        Ok(RuntimeVal::Obj(
            runtime
                .heap_mut()
                .alloc(HeapValue::List(TypedList::Mixed(vec![updated, removed]))),
        ))
    }

    fn mutate(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "mutate()")?;
        let values = args.as_slice();
        let map = map_arg(&values[0], runtime.heap(), "mutate() first argument")?.clone();
        let map_root = values[0].clone();
        let callback = values[1].clone();
        let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
            bail!("mutate() requires full runtime state");
        };
        let roots = [map_root, callback.clone()];
        state.collect_garbage(roots.iter());
        let guard = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(map)));
        let gc_threshold = state.heap().gc_threshold();
        state.heap_mut().set_gc_threshold(u32::MAX);
        let result = call_runtime_value_runtime(callback, &[guard.clone()], state, module, ctx);
        state.heap_mut().set_gc_threshold(gc_threshold);
        result?;
        Ok(RuntimeVal::Obj(
            state.heap_mut().alloc(HeapValue::List(TypedList::Mixed(vec![guard]))),
        ))
    }
}

fn set_map_entry(map: &TypedMap, key: RuntimeMapKey, value: RuntimeVal) -> TypedMap {
    let mut out = map.clone();
    out.set(key, value);
    out
}

impl ModuleProvider for MapModule {
    fn name(&self) -> &str {
        "map"
    }

    fn description(&self) -> &str {
        "Map utilities"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("len", Self::len, 1),
                RuntimeNativeExport::plain("keys", Self::keys, 1),
                RuntimeNativeExport::plain("values", Self::values, 1),
                RuntimeNativeExport::plain("has", Self::has, 2),
                RuntimeNativeExport::plain("get", Self::get, 2),
                RuntimeNativeExport::plain("set", Self::set, 3),
                RuntimeNativeExport::plain("delete", Self::delete, 2),
                RuntimeNativeExport::full_state("mutate", Self::mutate, 2),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("map", Box::new(MapModule::new()))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} takes exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
    }
}

fn one_map<'a>(args: NativeArgs<'a>, runtime: &'a NativeRuntime<'a>, name: &str) -> Result<&'a TypedMap> {
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

fn runtime_map_key_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<RuntimeMapKey> {
    match value {
        RuntimeVal::Nil => Ok(RuntimeMapKey::Nil),
        RuntimeVal::Bool(value) => Ok(RuntimeMapKey::Bool(*value)),
        RuntimeVal::Int(value) => Ok(RuntimeMapKey::Int(*value)),
        RuntimeVal::ShortStr(value) => Ok(RuntimeMapKey::ShortStr(*value)),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => Ok(runtime_string_map_key(Arc::clone(value))),
            Some(_) => Ok(RuntimeMapKey::Obj(*handle)),
            None => Err(anyhow!("heap object {} out of bounds", handle.index())),
        },
        RuntimeVal::Float(_) => bail!("{context} cannot be Float"),
    }
}

fn map_keys_list(map: &TypedMap, heap: &mut HeapStore) -> TypedList {
    match map {
        TypedMap::Mixed(entries) => {
            let mut keys = Vec::new();
            for key in entries.keys() {
                keys.push(runtime_map_key_to_value(key, heap));
            }
            TypedList::Mixed(keys)
        }
        TypedMap::StringMixed(entries) => TypedList::String(copy_string_map_keys(entries)),
        TypedMap::StringInt(entries) => TypedList::String(copy_string_map_keys(entries)),
        TypedMap::StringFloat(entries) => TypedList::String(copy_string_map_keys(entries)),
        TypedMap::StringBool(entries) => TypedList::String(copy_string_map_keys(entries)),
    }
}

fn runtime_map_key_to_value(key: &RuntimeMapKey, heap: &mut HeapStore) -> RuntimeVal {
    match key {
        RuntimeMapKey::Nil => RuntimeVal::Nil,
        RuntimeMapKey::Bool(value) => RuntimeVal::Bool(*value),
        RuntimeMapKey::Int(value) => RuntimeVal::Int(*value),
        RuntimeMapKey::ShortStr(value) => RuntimeVal::ShortStr(*value),
        RuntimeMapKey::String(value) => {
            if let Some(short) = ShortStr::new(value.as_ref()) {
                RuntimeVal::ShortStr(short)
            } else {
                RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::clone(value))))
            }
        }
        RuntimeMapKey::Obj(handle) => RuntimeVal::Obj(*handle),
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

fn copy_string_map_keys<T>(entries: &FastHashMap<Arc<str>, T>) -> Vec<Arc<str>> {
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

fn runtime_string_map_key(value: Arc<str>) -> RuntimeMapKey {
    if let Some(short) = ShortStr::new(&value) {
        RuntimeMapKey::ShortStr(short)
    } else {
        RuntimeMapKey::String(value)
    }
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

fn delete_map_entry(map: &TypedMap, key: &RuntimeMapKey) -> (TypedMap, RuntimeVal) {
    let mut out = map.clone();
    let removed = out.remove(key).unwrap_or(RuntimeVal::Nil);
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        util::fast_map::fast_hash_map_new,
        vm::{NativeArgs, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };
    use std::sync::Arc;

    use crate::runtime_native::runtime_string_value;

    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        registry.register_module("map", Box::new(MapModule::new()))?;
        registry.register_module("string", Box::new(lk_stdlib_string::StringModule::new()))?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn run_return(source: &str) -> Result<RuntimeVal> {
        Ok(run(source)?.first_return().clone())
    }

    fn runtime_short_string(value: &str) -> RuntimeVal {
        RuntimeVal::ShortStr(ShortStr::new(value).expect("short test string"))
    }

    fn expect_list(result: &ProgramResult) -> Vec<RuntimeVal> {
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

    fn map_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&MapModule::new(), name)
    }

    #[test]
    fn test_map_len_keys_values_has_get() -> Result<()> {
        assert_eq!(
            run_return("use map; return map.len({\"a\":1, \"b\":2});")?,
            RuntimeVal::Int(2)
        );

        let keys =
            run_return("use map; use string; let m={\"a\":1, \"b\":2}; return string.join(map.keys(m), \",\");")?;
        match keys {
            RuntimeVal::ShortStr(v) if v.as_str() == "a,b" || v.as_str() == "b,a" => {}
            _ => panic!("unexpected keys output: {:?}", keys),
        }

        assert_eq!(
            run_return("use map; return map.has({\"a\":1}, \"a\");")?,
            RuntimeVal::Bool(true)
        );
        assert_eq!(
            run_return("use map; return map.has({\"a\":1}, \"b\");")?,
            RuntimeVal::Bool(false)
        );
        assert_eq!(
            run_return("use map; return map.get({\"a\":1}, \"a\");")?,
            RuntimeVal::Int(1)
        );
        assert_eq!(
            run_return("use map; return map.get({\"a\":1}, \"b\");")?,
            RuntimeVal::Nil
        );
        assert_eq!(
            run_return("use map; let m=map.set({}, \"a\", \"x\"); return map.has(m, \"a\");")?,
            RuntimeVal::Bool(true)
        );
        assert_eq!(
            run_return("use map; let m=map.set({}, \"a\", \"x\"); return map.get(m, \"a\");")?,
            runtime_short_string("x")
        );

        let values = run("use map; return map.values({\"a\":1, \"b\":2});")?;
        let values = expect_list(&values);
        assert_eq!(values.len(), 2);
        assert!(values.contains(&RuntimeVal::Int(1)));
        assert!(values.contains(&RuntimeVal::Int(2)));
        Ok(())
    }

    #[test]
    fn test_map_int_keys_use_mixed_map_backing() -> Result<()> {
        let result = run(r#"
            use map;
            let counts = {};
            counts = map.set(counts, 1, 10);
            counts = map.set(counts, 2, 20);
            let removed_pair = map.delete(counts, 1);
            let without = removed_pair[0];
            return [map.get(counts, 1), map.get(counts, 2), map.values(counts), map.has(without, 1), removed_pair[1]];
        "#)?;
        let values = expect_list(&result);
        assert_eq!(values.len(), 5);
        assert_eq!(values[0], RuntimeVal::Int(10));
        assert_eq!(values[1], RuntimeVal::Int(20));
        assert_eq!(values[3], RuntimeVal::Bool(false));
        assert_eq!(values[4], RuntimeVal::Int(10));
        let RuntimeVal::Obj(values_handle) = values[2] else {
            panic!("map.values should return a list");
        };
        let Some(HeapValue::List(TypedList::Int(map_values))) = result.state.heap().get(values_handle) else {
            panic!("map.values should keep int list shape");
        };
        let mut sorted = map_values.as_slice().to_vec();
        sorted.sort();
        assert_eq!(sorted, vec![10, 20]);
        Ok(())
    }

    #[test]
    fn test_map_set_counts_dynamic_string_keys() -> Result<()> {
        let result = run_return(
            r#"
            use map;
            let words = "the fox the".split(" ");
            let counts = {};
            for word in words {
              let current = map.get(counts, word);
              if (current == nil) {
                counts = map.set(counts, word, 1);
              } else {
                counts = map.set(counts, word, current + 1);
              }
            }
            return map.get(counts, "the");
            "#,
        )?;

        assert_eq!(result, RuntimeVal::Int(2));
        Ok(())
    }

    #[test]
    fn test_map_set_and_delete() -> Result<()> {
        let result = run(r#"
            use map;
            let updated = map.set({"a": 1}, "a", 7);
            let removed_pair = map.delete(updated, "a");
            let without = removed_pair[0];
            let removed = removed_pair[1];
            return [removed, map.has(without, "a")];
        "#)?;
        let values = expect_list(&result);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], RuntimeVal::Int(7));
        assert_eq!(values[1], RuntimeVal::Bool(false));
        Ok(())
    }

    #[test]
    fn test_map_public_functions_use_runtime_native_abi() -> Result<()> {
        for name in ["len", "keys", "values", "has", "get", "set", "delete"] {
            let (arity, function) = map_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn test_map_direct_runtime_call_preserves_typed_map() -> Result<()> {
        let (_, function) = map_native("set")?;
        let NativeFunction::Plain(function) = function else {
            panic!("set should use plain RuntimeNative");
        };
        let mut entries = fast_hash_map_new();
        entries.insert(RuntimeMapKey::String(Arc::<str>::from("a")), RuntimeVal::Int(1));
        let mut state = RuntimeModuleState::default();
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
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let result = function(NativeArgs::new(&args), &mut runtime)?;
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
        let NativeFunction::Plain(function) = function else {
            panic!("values should use plain RuntimeNative");
        };
        let mut entries = fast_hash_map_new();
        entries.insert(Arc::<str>::from("a"), 1);
        entries.insert(Arc::<str>::from("b"), 2);
        let mut state = RuntimeModuleState::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringInt(entries))));
        let args = [map];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

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
        let NativeFunction::Plain(function) = function else {
            panic!("values should use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let heap_string = runtime_string_value("long-map-value", state.heap_mut());
        let mut entries = fast_hash_map_new();
        entries.insert(Arc::<str>::from("a"), runtime_string_value("short", state.heap_mut()));
        entries.insert(Arc::<str>::from("b"), heap_string);
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
        let args = [map];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

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
        let NativeFunction::Plain(function) = function else {
            panic!("values should use plain RuntimeNative");
        };
        let mut entries = fast_hash_map_new();
        entries.insert(Arc::<str>::from("a"), RuntimeVal::Int(1));
        entries.insert(Arc::<str>::from("b"), runtime_short_string("two"));
        let mut state = RuntimeModuleState::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
        let args = [map];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

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
        let NativeFunction::Plain(function) = function else {
            panic!("keys should use plain RuntimeNative");
        };
        let mut entries = fast_hash_map_new();
        entries.insert(Arc::<str>::from("long-key-one"), true);
        entries.insert(Arc::<str>::from("long-key-two"), false);
        let mut state = RuntimeModuleState::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringBool(entries))));
        let args = [map];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

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
        let NativeFunction::Plain(function) = function else {
            panic!("delete should use plain RuntimeNative");
        };
        let mut entries = fast_hash_map_new();
        entries.insert(Arc::<str>::from("a"), 1);
        entries.insert(Arc::<str>::from("b"), 2);
        let mut state = RuntimeModuleState::default();
        let map = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Map(TypedMap::StringInt(entries))));
        let key = runtime_string_value("b", state.heap_mut());
        let args = [map, key];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

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
