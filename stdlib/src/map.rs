use arcstr::ArcStr;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use lk_core::module::{Module, ModuleRegistry};
use lk_core::val::methods::register_fast_method;
use lk_core::val::{NativeArgs, Val};
use lk_core::vm::VmContext;

use crate::collections::{MapMutation, MutableMap};

use lk_core::util::fast_map::FastHashMap;
use lk_core::val::{IteratorState, IteratorValue, MutationGuardState, MutationGuardValue};

const MAP_MUT_TYPE: &str = "MapMut";

struct MapIteratorState {
    entries: Vec<(ArcStr, Val)>,
    index: usize,
}

impl MapIteratorState {
    fn new(entries: Vec<(ArcStr, Val)>) -> Self {
        Self { entries, index: 0 }
    }
}

impl IteratorState for MapIteratorState {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        if self.index >= self.entries.len() {
            return Ok(None);
        }
        let (key, value) = &self.entries[self.index];
        self.index += 1;
        let pair = Val::List(Arc::from(vec![Val::from_str(key.as_str()), value.clone()]));
        Ok(Some(pair))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.entries.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }

    fn debug_name(&self) -> &'static str {
        "map_iter"
    }
}

struct MapMutationGuardState {
    inner: MapMutation,
    mutated: bool,
}

impl MapMutationGuardState {
    fn new(inner: MapMutation) -> Self {
        Self { inner, mutated: false }
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn mark_mutated(&mut self) {
        self.mutated = true;
    }

    fn contains(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }

    fn insert(&mut self, key: ArcStr, value: Val) -> Val {
        self.mark_mutated();
        self.inner.insert(key, value).unwrap_or(Val::Nil)
    }

    fn remove(&mut self, key: &str) -> Val {
        let removed = self.inner.remove(key).unwrap_or(Val::Nil);
        if removed != Val::Nil {
            self.mark_mutated();
        }
        removed
    }
}

impl MutationGuardState for MapMutationGuardState {
    fn guard_type(&self) -> &'static str {
        MAP_MUT_TYPE
    }

    fn commit(&mut self) -> Result<Val> {
        let empty: Arc<FastHashMap<ArcStr, Val>> = Arc::new(FastHashMap::default());
        let current = mem::replace(&mut self.inner, MapMutation::new(empty));
        let updated = current.finish();
        self.inner = MapMutation::from_val(&updated)?;
        self.mutated = false;
        Ok(updated)
    }

    fn snapshot(&mut self) -> Result<Val> {
        let empty: Arc<FastHashMap<ArcStr, Val>> = Arc::new(FastHashMap::default());
        let current = mem::replace(&mut self.inner, MapMutation::new(empty));
        let snapshot = current.finish();
        self.inner = MapMutation::from_val(&snapshot)?;
        Ok(snapshot)
    }

    fn has_mutated(&self) -> bool {
        self.mutated
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn expect_map_guard(val: &Val) -> Result<Arc<MutationGuardValue>> {
    match val {
        Val::MutationGuard(handle) if handle.guard_type() == MAP_MUT_TYPE => Ok(handle.clone()),
        Val::MutationGuard(handle) => Err(anyhow!(
            "expected {} mutation guard, got {}",
            MAP_MUT_TYPE,
            handle.guard_type()
        )),
        other => Err(anyhow!(
            "expected {} mutation guard, got {}",
            MAP_MUT_TYPE,
            other.type_name()
        )),
    }
}

fn with_map_guard_mut<F, R>(guard: &Arc<MutationGuardValue>, f: F) -> Result<R>
where
    F: FnOnce(&mut MapMutationGuardState) -> Result<R>,
{
    guard.with_state_mut(|state| {
        let state = state
            .as_any_mut()
            .downcast_mut::<MapMutationGuardState>()
            .ok_or_else(|| anyhow!("invalid MapMut guard handle"))?;
        f(state)
    })
}

fn with_map_guard<F, R>(guard: &Arc<MutationGuardValue>, f: F) -> Result<R>
where
    F: FnOnce(&MapMutationGuardState) -> Result<R>,
{
    guard.with_state(|state| {
        let state = state
            .as_any()
            .downcast_ref::<MapMutationGuardState>()
            .ok_or_else(|| anyhow!("invalid MapMut guard handle"))?;
        f(state)
    })
}

fn map_mut_guard_len(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("len() expects guard argument"));
    }
    let guard = expect_map_guard(&args[0])?;
    let len = with_map_guard(&guard, |state| Ok(state.len()))?;
    Ok(Val::Int(len as i64))
}

fn map_mut_guard_len_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
    map_mut_guard_len(args.as_slice(), ctx)
}

fn map_mut_guard_contains(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("has() expects (guard, key)"));
    }
    let guard = expect_map_guard(&args[0])?;
    let key = args[1].as_str().ok_or_else(|| anyhow!("has() key must be a string"))?;
    let result = with_map_guard(&guard, |state| Ok(state.contains(key)))?;
    Ok(Val::Bool(result))
}

fn map_mut_guard_contains_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
    map_mut_guard_contains(args.as_slice(), ctx)
}

fn map_mut_guard_insert(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!("insert() expects (guard, key, value)"));
    }
    let guard = expect_map_guard(&args[0])?;
    let key: ArcStr = args[1]
        .as_str()
        .map(Val::intern_str)
        .ok_or_else(|| anyhow!("insert() key must be a string"))?;
    let value = args[2].clone();
    let previous = with_map_guard_mut(&guard, |state| Ok(state.insert(key, value)))?;
    Ok(previous)
}

fn map_mut_guard_insert_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
    map_mut_guard_insert(args.as_slice(), ctx)
}

fn map_mut_guard_remove(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("remove() expects (guard, key)"));
    }
    let guard = expect_map_guard(&args[0])?;
    let key = args[1]
        .as_str()
        .ok_or_else(|| anyhow!("remove() key must be a string"))?
        .to_owned();
    let removed = with_map_guard_mut(&guard, |state| Ok(state.remove(&key)))?;
    Ok(removed)
}

fn map_mut_guard_remove_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
    map_mut_guard_remove(args.as_slice(), ctx)
}

fn map_mut_guard_commit(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("commit() expects guard argument"));
    }
    let guard = expect_map_guard(&args[0])?;
    guard.commit()
}

fn map_mut_guard_commit_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
    map_mut_guard_commit(args.as_slice(), ctx)
}

fn map_mut_guard_as_map(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("as_map() expects guard argument"));
    }
    let guard = expect_map_guard(&args[0])?;
    guard.snapshot()
}

fn map_mut_guard_as_map_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
    map_mut_guard_as_map(args.as_slice(), ctx)
}

#[derive(Debug)]
pub struct MapModule {
    functions: HashMap<String, Val>,
}

impl Default for MapModule {
    fn default() -> Self {
        Self::new()
    }
}

impl MapModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Core map utilities
        functions.insert("len".to_string(), Val::RustFastFunction(Self::len_fast));
        functions.insert("keys".to_string(), Val::RustFastFunction(Self::keys));
        functions.insert("values".to_string(), Val::RustFastFunction(Self::values));
        functions.insert("has".to_string(), Val::RustFastFunction(Self::has_fast));
        functions.insert("get".to_string(), Val::RustFastFunction(Self::get_fast));
        functions.insert("set".to_string(), Val::RustFastFunction(Self::set));
        functions.insert("delete".to_string(), Val::RustFastFunction(Self::delete));
        {
            functions.insert("into_iter".to_string(), Val::RustFastFunction(Self::into_iter));
            functions.insert("mutate".to_string(), Val::RustFastFunction(Self::mutate));
        }

        // Register meta-methods for Map
        register_fast_method("Map", "len", Self::len_fast);
        register_fast_method("Map", "keys", Self::keys);
        register_fast_method("Map", "values", Self::values);
        register_fast_method("Map", "has", Self::has_fast);
        register_fast_method("Map", "get", Self::get_fast);
        register_fast_method("Map", "set", Self::set);
        register_fast_method("Map", "delete", Self::delete);
        {
            register_fast_method("Map", "into_iter", Self::into_iter);
            register_fast_method("Map", "__iter__", Self::into_iter);
            register_fast_method("Map", "mutate", Self::mutate_method_fast);

            register_fast_method(MAP_MUT_TYPE, "len", map_mut_guard_len_fast);
            register_fast_method(MAP_MUT_TYPE, "has", map_mut_guard_contains_fast);
            register_fast_method(MAP_MUT_TYPE, "contains", map_mut_guard_contains_fast);
            register_fast_method(MAP_MUT_TYPE, "set", map_mut_guard_insert_fast);
            register_fast_method(MAP_MUT_TYPE, "insert", map_mut_guard_insert_fast);
            register_fast_method(MAP_MUT_TYPE, "delete", map_mut_guard_remove_fast);
            register_fast_method(MAP_MUT_TYPE, "remove", map_mut_guard_remove_fast);
            register_fast_method(MAP_MUT_TYPE, "commit", map_mut_guard_commit_fast);
            register_fast_method(MAP_MUT_TYPE, "as_map", map_mut_guard_as_map_fast);
        }

        Self { functions }
    }

    fn len_fast(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("len() takes exactly 1 argument"));
        }
        match args.get(0) {
            Some(Val::Map(map)) => Ok(Val::Int(map.len() as i64)),
            _ => Err(anyhow!("len() argument must be a map")),
        }
    }

    fn keys(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("keys() takes exactly 1 argument"));
        }
        match args.get(0) {
            Some(Val::Map(m)) => {
                let mut out: Vec<Val> = Vec::with_capacity(m.len());
                for k in m.keys() {
                    out.push(Val::from_str(k.as_str()));
                }
                Ok(Val::List(Arc::from(out)))
            }
            _ => Err(anyhow!("keys() argument must be a map")),
        }
    }

    fn values(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("values() takes exactly 1 argument"));
        }
        match args.get(0) {
            Some(Val::Map(m)) => {
                let mut out: Vec<Val> = Vec::with_capacity(m.len());
                for v in m.values() {
                    out.push(v.clone());
                }
                Ok(Val::List(Arc::from(out)))
            }
            _ => Err(anyhow!("values() argument must be a map")),
        }
    }

    fn has_fast(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("has() takes exactly 2 arguments: map, key"));
        }
        let map = match args.get(0) {
            Some(Val::Map(map)) => &**map,
            _ => return Err(anyhow!("has() first argument must be a map")),
        };
        let key = args
            .get(1)
            .and_then(Val::as_str)
            .ok_or_else(|| anyhow!("has() key must be a string"))?;
        Ok(Val::Bool(Val::map_contains_str(map, key)))
    }

    fn get_fast(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("get() takes exactly 2 arguments: map, key"));
        }
        let map = match args.get(0) {
            Some(Val::Map(map)) => &**map,
            _ => return Err(anyhow!("get() first argument must be a map")),
        };
        let key = args
            .get(1)
            .and_then(Val::as_str)
            .ok_or_else(|| anyhow!("get() key must be a string"))?;
        Ok(Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil))
    }

    fn set(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 3 {
            return Err(anyhow!("set() takes exactly 3 arguments: map, key, value"));
        }
        let args = args.as_slice();
        let key_arc: ArcStr = args[1]
            .string_key_arcstr()
            .ok_or_else(|| anyhow!("set() key must be a string"))?;
        let mut map_arc = match &args[0] {
            Val::Map(m) => m.clone(),
            other => return Err(anyhow!("set() first argument must be a map, got {}", other.type_name())),
        };
        // Arc::make_mut: if refcount is 1, reuses allocation in-place.
        // If refcount > 1 (shared), clones the data. This is the stdlib CoW.
        Val::map_insert_arcstr(Arc::make_mut(&mut map_arc), key_arc, args[2].clone());
        Ok(Val::Map(map_arc))
    }

    fn delete(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("delete() takes exactly 2 arguments: map, key"));
        }
        let args = args.as_slice();
        let key = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("delete() key must be a string"))?;
        match &args[0] {
            Val::Map(_) => {
                let mut map = MapMutation::from_val(&args[0])?;
                let removed = map.remove(key).unwrap_or(Val::Nil);
                let updated = map.finish();
                Ok(Val::List(vec![updated, removed].into()))
            }
            _ => Err(anyhow!("delete() first argument must be a map")),
        }
    }

    fn into_iter(args: NativeArgs<'_>, _: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("into_iter expects exactly 1 argument"));
        }
        let map = match args.get(0) {
            Some(Val::Map(map)) => map.clone(),
            Some(other) => return Err(anyhow!("into_iter expects a map, got {}", other.type_name())),
            None => return Err(anyhow!("into_iter expects exactly 1 argument")),
        };
        let mut entries: Vec<(ArcStr, Val)> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        entries.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        let iter_state = MapIteratorState::new(entries);
        let handle = IteratorValue::with_origin(iter_state, ArcStr::from("map.into_iter"));
        Ok(Val::Iterator(handle))
    }

    fn mutate(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
        let (updated, closure_result) = Self::mutate_impl(args.as_slice(), ctx)?;
        let out = Vec::from([updated, closure_result]);
        Ok(Val::List(Arc::from(out)))
    }

    fn mutate_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        let (updated, _) = Self::mutate_impl(args, ctx)?;
        Ok(updated)
    }

    fn mutate_method_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> Result<Val> {
        Self::mutate_method(args.as_slice(), ctx)
    }

    fn mutate_impl(args: &[Val], ctx: &mut VmContext) -> Result<(Val, Val)> {
        if args.len() != 2 {
            return Err(anyhow!("mutate() expects (map, function)"));
        }
        let map_val = match &args[0] {
            Val::Map(_) => args[0].clone(),
            other => {
                return Err(anyhow!(
                    "mutate() first argument must be a map, got {}",
                    other.type_name()
                ));
            }
        };
        let mutator = match &args[1] {
            f @ Val::Closure(_)
            | f @ Val::RustFunction(_)
            | f @ Val::RustFastFunction(_)
            | f @ Val::RustFastFunctionNamed(_)
            | f @ Val::RustFunctionNamed(_) => f.clone(),
            other => {
                return Err(anyhow!(
                    "mutate() second argument must be a function, got {}",
                    other.type_name()
                ));
            }
        };

        let guard_state = MapMutationGuardState::new(MapMutation::from_val(&map_val)?);
        let guard_handle = MutationGuardValue::new(guard_state);
        let guard_val = Val::MutationGuard(guard_handle.clone());

        let closure_result = mutator.call(std::slice::from_ref(&guard_val), ctx)?;
        let updated = guard_handle.commit()?;
        Ok((updated, closure_result))
    }
}

impl Module for MapModule {
    fn name(&self) -> &str {
        "map"
    }

    fn description(&self) -> &str {
        "Map utilities and meta-methods"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        // Functions are available via module import; meta methods are registered above
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register_stdlib_modules;
    use anyhow::Result;
    use lk_core::module::ModuleRegistry;
    use lk_core::stmt::{ModuleResolver, stmt_parser::StmtParser};
    use lk_core::token::Tokenizer;
    use lk_core::vm::Vm;
    use std::sync::Arc;

    fn run(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut vm = Vm::new();
        program.execute_with_vm(&mut vm, &mut env)
    }

    #[test]
    fn test_map_len_keys_values_has_get() -> Result<()> {
        // len
        assert_eq!(run("return {\"a\":1, \"b\":2}.len();")?, Val::Int(2));
        // keys/values
        let keys = run("let m={\"a\":1, \"b\":2}; let ks = m.keys().join(\",\"); return ks;")?;
        // Order is not guaranteed; check either order
        match keys {
            v if v.as_str() == Some("a,b") || v.as_str() == Some("b,a") => {}
            _ => panic!("unexpected keys output: {}", keys),
        }
        // has/get
        assert_eq!(run("let m={\"a\":1}; return m.has(\"a\");")?, Val::Bool(true));
        assert_eq!(run("let m={\"a\":1}; return m.has(\"b\");")?, Val::Bool(false));
        assert_eq!(run("let m={\"a\":1}; return m.get(\"a\");")?, Val::Int(1));
        assert_eq!(run("let m={\"a\":1}; return m.get(\"b\");")?, Val::Nil);
        Ok(())
    }

    #[test]
    fn test_map_set_and_delete() -> Result<()> {
        let result = run(r#"
            import map;
            let updated = map.set({"a": 1}, "a", 7);
            let removed_pair = map.delete(updated, "a");
            let without = removed_pair.get(0);
            let removed = removed_pair.get(1);
            return [removed, without.has("a")];
        "#)?;
        let Val::List(values) = result else {
            panic!("expected list");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], Val::Int(7)); // removed: 7
        assert_eq!(values[1], Val::Bool(false));
        Ok(())
    }

    #[test]
    fn test_map_public_functions_use_fast_native_abi() {
        let module = MapModule::new();
        let exports = module.exports();
        for name in [
            "len",
            "keys",
            "values",
            "has",
            "get",
            "set",
            "delete",
            "into_iter",
            "mutate",
        ] {
            let value = exports.get(name).expect("map function export present");
            assert!(
                matches!(value, Val::RustFastFunction(_)),
                "{name} should use RustFastFunction"
            );
        }
    }
}
