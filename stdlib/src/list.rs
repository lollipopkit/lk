use std::collections::HashMap;
use std::{mem, sync::Arc};

use crate::collections::{ListMutation, MutableSequence};
use crate::iter::{
    chain as iter_chain, chunk as iter_chunk, enumerate as iter_enumerate, filter as iter_filter,
    flatten as iter_flatten, map as iter_map, reduce as iter_reduce, skip as iter_skip, take as iter_take,
    unique as iter_unique, zip as iter_zip,
};
use anyhow::{Result, anyhow};
use lkr_core::module::{Module, ModuleRegistry};
use lkr_core::val::Val;
use lkr_core::val::methods::register_method;
use lkr_core::val::{IteratorState, IteratorValue, MutationGuardState, MutationGuardValue};
use lkr_core::vm::VmContext;

const LIST_MUT_TYPE: &str = "ListMut";

struct ListIteratorState {
    data: Arc<[Val]>,
    index: usize,
}

impl ListIteratorState {
    fn new(data: Arc<[Val]>) -> Self {
        Self { data, index: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.index)
    }
}

impl IteratorState for ListIteratorState {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        if self.index >= self.data.len() {
            return Ok(None);
        }
        let value = self.data[self.index].clone();
        self.index += 1;
        Ok(Some(value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining();
        (remaining, Some(remaining))
    }

    fn debug_name(&self) -> &'static str {
        "list_iter"
    }
}

struct ListMutationGuardState {
    inner: ListMutation,
    mutated: bool,
}

impl ListMutationGuardState {
    fn new(inner: ListMutation) -> Self {
        Self { inner, mutated: false }
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn mark_mutated(&mut self) {
        self.mutated = true;
    }

    fn push(&mut self, value: Val) {
        self.inner.push(value);
        self.mark_mutated();
    }

    fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    fn replace(&mut self, index: usize, value: Val) -> Result<Val> {
        let replaced = self.inner.replace(index, value)?;
        self.mark_mutated();
        Ok(replaced)
    }

    fn remove(&mut self, index: usize) -> Option<Val> {
        let removed = self.inner.remove(index);
        if removed.is_some() {
            self.mark_mutated();
        }
        removed
    }

    fn pop(&mut self) -> Option<Val> {
        if self.len() == 0 {
            None
        } else {
            self.remove(self.len() - 1)
        }
    }

    fn as_list_val(&self) -> Val {
        Val::List(Arc::from(self.inner.as_slice().to_vec()))
    }
}

impl MutationGuardState for ListMutationGuardState {
    fn guard_type(&self) -> &'static str {
        LIST_MUT_TYPE
    }

    fn commit(&mut self) -> Result<Val> {
        let scratch: Arc<[Val]> = Arc::from(Vec::<Val>::new());
        let current = mem::replace(&mut self.inner, ListMutation::new(scratch));
        let updated = current.finish();
        self.inner = ListMutation::from_val(&updated)?;
        self.mutated = false;
        Ok(updated)
    }

    fn snapshot(&mut self) -> Result<Val> {
        Ok(self.as_list_val())
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

fn expect_list_guard(val: &Val) -> Result<Arc<MutationGuardValue>> {
    match val {
        Val::MutationGuard(handle) if handle.guard_type() == LIST_MUT_TYPE => Ok(handle.clone()),
        Val::MutationGuard(handle) => Err(anyhow!(
            "expected {} mutation guard, got {}",
            LIST_MUT_TYPE,
            handle.guard_type()
        )),
        other => Err(anyhow!(
            "expected {} mutation guard, got {}",
            LIST_MUT_TYPE,
            other.type_name()
        )),
    }
}

fn with_list_guard_mut<F, R>(guard: &Arc<MutationGuardValue>, f: F) -> Result<R>
where
    F: FnOnce(&mut ListMutationGuardState) -> Result<R>,
{
    guard.with_state_mut(|state| {
        let state = state
            .as_any_mut()
            .downcast_mut::<ListMutationGuardState>()
            .ok_or_else(|| anyhow!("invalid ListMut guard handle"))?;
        f(state)
    })
}

fn with_list_guard<F, R>(guard: &Arc<MutationGuardValue>, f: F) -> Result<R>
where
    F: FnOnce(&ListMutationGuardState) -> Result<R>,
{
    guard.with_state(|state| {
        let state = state
            .as_any()
            .downcast_ref::<ListMutationGuardState>()
            .ok_or_else(|| anyhow!("invalid ListMut guard handle"))?;
        f(state)
    })
}

fn list_mut_guard_len(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("len() expects guard argument"));
    }
    let guard = expect_list_guard(&args[0])?;
    let len = with_list_guard(&guard, |state| Ok(state.len()))?;
    Ok(Val::Int(len as i64))
}

fn list_mut_guard_push(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("push() expects (guard, value)"));
    }
    let guard = expect_list_guard(&args[0])?;
    let value = args[1].clone();
    with_list_guard_mut(&guard, |state| {
        state.push(value);
        Ok(())
    })?;
    Ok(args[0].clone())
}

fn list_mut_guard_pop(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("pop() expects guard argument"));
    }
    let guard = expect_list_guard(&args[0])?;
    let result = with_list_guard_mut(&guard, |state| Ok(state.pop().unwrap_or(Val::Nil)))?;
    Ok(result)
}

fn list_mut_guard_replace(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!("replace() expects (guard, index, value)"));
    }
    let guard = expect_list_guard(&args[0])?;
    let index = match &args[1] {
        Val::Int(i) if *i >= 0 => *i as usize,
        _ => return Err(anyhow!("replace() index must be non-negative integer")),
    };
    let value = args[2].clone();
    with_list_guard_mut(&guard, |state| state.replace(index, value))
}

fn list_mut_guard_remove(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("remove() expects (guard, index)"));
    }
    let guard = expect_list_guard(&args[0])?;
    let index = match &args[1] {
        Val::Int(i) if *i >= 0 => *i as usize,
        _ => return Err(anyhow!("remove() index must be non-negative integer")),
    };
    let removed = with_list_guard_mut(&guard, |state| Ok(state.remove(index).unwrap_or(Val::Nil)))?;
    Ok(removed)
}

fn list_mut_guard_reserve(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("reserve() expects (guard, additional)"));
    }
    let guard = expect_list_guard(&args[0])?;
    let additional = match &args[1] {
        Val::Int(i) if *i >= 0 => *i as usize,
        _ => return Err(anyhow!("reserve() additional must be non-negative integer")),
    };
    with_list_guard_mut(&guard, |state| {
        state.reserve(additional);
        Ok(())
    })?;
    Ok(args[0].clone())
}

fn list_mut_guard_commit(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("commit() expects guard argument"));
    }
    let guard = expect_list_guard(&args[0])?;
    guard.commit()
}

fn list_mut_guard_as_list(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("as_list() expects guard argument"));
    }
    let guard = expect_list_guard(&args[0])?;
    guard.snapshot()
}

#[derive(Debug)]
pub struct ListModule {
    functions: HashMap<String, Val>,
}

impl Default for ListModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ListModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Core list utilities
        functions.insert("len".to_string(), Val::RustFunction(Self::len));
        functions.insert("push".to_string(), Val::RustFunction(Self::push));
        functions.insert("concat".to_string(), Val::RustFunction(Self::concat));
        functions.insert("join".to_string(), Val::RustFunction(Self::join));
        functions.insert("get".to_string(), Val::RustFunction(Self::get));
        functions.insert("first".to_string(), Val::RustFunction(Self::first));
        functions.insert("last".to_string(), Val::RustFunction(Self::last));
        functions.insert("set".to_string(), Val::RustFunction(Self::set));
        // Functional helpers
        functions.insert("map".to_string(), Val::RustFunction(Self::map));
        functions.insert("filter".to_string(), Val::RustFunction(Self::filter));
        functions.insert("reduce".to_string(), Val::RustFunction(Self::reduce));
        // Iterator-based helpers (method sugar delegating to iter)
        functions.insert("take".to_string(), Val::RustFunction(Self::take));
        functions.insert("skip".to_string(), Val::RustFunction(Self::skip));
        functions.insert("chain".to_string(), Val::RustFunction(Self::chain));
        functions.insert("flatten".to_string(), Val::RustFunction(Self::flatten));
        functions.insert("unique".to_string(), Val::RustFunction(Self::unique));
        functions.insert("chunk".to_string(), Val::RustFunction(Self::chunk));
        functions.insert("enumerate".to_string(), Val::RustFunction(Self::enumerate));
        functions.insert("zip".to_string(), Val::RustFunction(Self::zip));
        {
            functions.insert("into_iter".to_string(), Val::RustFunction(Self::into_iter));
            functions.insert("mutate".to_string(), Val::RustFunction(Self::mutate));
        }

        // Register as meta-methods for List
        register_method("List", "len", Self::len);
        register_method("List", "push", Self::push);
        register_method("List", "concat", Self::concat);
        register_method("List", "join", Self::join);
        register_method("List", "get", Self::get);
        register_method("List", "first", Self::first);
        register_method("List", "last", Self::last);
        register_method("List", "set", Self::set);
        register_method("List", "map", Self::map);
        register_method("List", "filter", Self::filter);
        register_method("List", "reduce", Self::reduce);
        // Iterator-based helpers as List methods
        register_method("List", "take", Self::take);
        register_method("List", "skip", Self::skip);
        register_method("List", "chain", Self::chain);
        register_method("List", "flatten", Self::flatten);
        register_method("List", "unique", Self::unique);
        register_method("List", "chunk", Self::chunk);
        register_method("List", "enumerate", Self::enumerate);
        register_method("List", "zip", Self::zip);
        {
            register_method("List", "into_iter", Self::into_iter);
            register_method("List", "__iter__", Self::into_iter);
            register_method("List", "mutate", Self::mutate_method);

            register_method(LIST_MUT_TYPE, "len", list_mut_guard_len);
            register_method(LIST_MUT_TYPE, "push", list_mut_guard_push);
            register_method(LIST_MUT_TYPE, "pop", list_mut_guard_pop);
            register_method(LIST_MUT_TYPE, "replace", list_mut_guard_replace);
            register_method(LIST_MUT_TYPE, "remove", list_mut_guard_remove);
            register_method(LIST_MUT_TYPE, "reserve", list_mut_guard_reserve);
            register_method(LIST_MUT_TYPE, "commit", list_mut_guard_commit);
            register_method(LIST_MUT_TYPE, "as_list", list_mut_guard_as_list);
        }

        Self { functions }
    }

    fn len(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("len() takes exactly 1 argument"));
        }
        match &args[0] {
            Val::List(l) => Ok(Val::Int(l.len() as i64)),
            _ => Err(anyhow!("len() argument must be a list")),
        }
    }

    // Return a new list with value appended (immutable)
    fn push(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("push() takes exactly 2 arguments: list, value"));
        }
        match &args[0] {
            Val::List(_) => {
                let mut list = ListMutation::from_val(&args[0])?;
                list.reserve(1);
                list.push(args[1].clone());
                Ok(list.finish())
            }
            _ => Err(anyhow!("push() first argument must be a list")),
        }
    }

    // Concatenate two lists
    fn concat(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("concat() takes exactly 2 arguments: list, other_list"));
        }
        let other = match &args[1] {
            Val::List(list) => list.clone(),
            _ => {
                return Err(anyhow!("concat() second argument must be a list"));
            }
        };
        match &args[0] {
            Val::List(_) => {
                let mut list = ListMutation::from_val(&args[0])?;
                list.reserve(other.len());
                list.extend(other.iter().cloned());
                Ok(list.finish())
            }
            _ => Err(anyhow!("concat() first argument must be a list")),
        }
    }

    // Join a list of strings with a delimiter
    fn join(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("join() takes exactly 2 arguments: list<string>, delimiter"));
        }
        let list = match &args[0] {
            Val::List(l) => &**l,
            _ => return Err(anyhow!("join() first argument must be a list")),
        };
        let delimiter = match &args[1] {
            Val::Str(d) => &**d,
            _ => return Err(anyhow!("join() second argument must be a string")),
        };
        let mut strings: Vec<&str> = Vec::with_capacity(list.len());
        for item in list.iter() {
            match item {
                Val::Str(s) => strings.push(&**s),
                _ => return Err(anyhow!("join() list must contain only strings")),
            }
        }
        Ok(Val::Str(strings.join(delimiter).into()))
    }

    // Safe index access: get(index) -> value|nil
    fn get(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("get() takes exactly 2 arguments: list, index"));
        }
        let list = match &args[0] {
            Val::List(l) => &**l,
            _ => return Err(anyhow!("get() first argument must be a list")),
        };
        let idx = match &args[1] {
            Val::Int(i) => *i,
            _ => return Err(anyhow!("get() index must be an integer")),
        };
        if idx < 0 {
            return Ok(Val::Nil);
        }
        let uidx = idx as usize;
        Ok(list.get(uidx).cloned().unwrap_or(Val::Nil))
    }

    fn first(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("first() takes exactly 1 argument"));
        }
        match &args[0] {
            Val::List(l) => Ok(l.first().cloned().unwrap_or(Val::Nil)),
            _ => Err(anyhow!("first() argument must be a list")),
        }
    }

    fn last(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("last() takes exactly 1 argument"));
        }
        match &args[0] {
            Val::List(l) => Ok(l.last().cloned().unwrap_or(Val::Nil)),
            _ => Err(anyhow!("last() argument must be a list")),
        }
    }

    // Replace the element at index with a new value, returning [updated_list, old_value]
    fn set(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 3 {
            return Err(anyhow!("set() takes exactly 3 arguments: list, index, value"));
        }
        let index = match &args[1] {
            Val::Int(i) => *i,
            _ => return Err(anyhow!("set() index must be an integer")),
        };
        if index < 0 {
            return Err(anyhow!("set() index must be non-negative"));
        }
        match &args[0] {
            Val::List(_) => {
                let mut list = ListMutation::from_val(&args[0])?;
                let old = list.replace(index as usize, args[2].clone())?;
                let updated = list.finish();
                Ok(Val::List(vec![updated, old].into()))
            }
            _ => Err(anyhow!("set() first argument must be a list")),
        }
    }

    // Map over list with a function: list.map(|x| ...)
    // Accepts either as module call: map(list, func) or meta-method: list.map(func)
    fn map(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        // Delegate to iter::map for core logic
        iter_map(args, ctx)
    }

    // Filter list with predicate function: list.filter(|x| cond)
    // Truthiness: false and nil are false; everything else treated as true
    fn filter(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        // Delegate to iter::filter for core logic
        iter_filter(args, ctx)
    }

    // Reduce list with accumulator: list.reduce(init, |acc, x| ...)
    fn reduce(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        // Delegate to iter::reduce for core logic
        iter_reduce(args, ctx)
    }

    // Method sugar delegating to iter::* sequence helpers
    fn take(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_take(args, ctx)
    }

    fn skip(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_skip(args, ctx)
    }

    fn chain(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_chain(args, ctx)
    }

    fn flatten(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_flatten(args, ctx)
    }

    fn unique(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_unique(args, ctx)
    }

    fn chunk(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_chunk(args, ctx)
    }

    fn enumerate(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_enumerate(args, ctx)
    }

    fn zip(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        iter_zip(args, ctx)
    }

    fn into_iter(args: &[Val], _: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("into_iter expects exactly 1 argument"));
        }
        let list = match &args[0] {
            Val::List(list) => list.clone(),
            other => return Err(anyhow!("into_iter expects a list, got {}", other.type_name())),
        };
        let iter_state = ListIteratorState::new(list);
        let handle = IteratorValue::with_origin(iter_state, Arc::from("list.into_iter"));
        Ok(Val::Iterator(handle))
    }

    fn mutate(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        let (updated, closure_result) = Self::mutate_impl(args, ctx)?;
        let out = Vec::from([updated, closure_result]);
        Ok(Val::List(Arc::from(out)))
    }

    fn mutate_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        let (updated, _) = Self::mutate_impl(args, ctx)?;
        Ok(updated)
    }

    fn mutate_impl(args: &[Val], ctx: &mut VmContext) -> Result<(Val, Val)> {
        if args.len() != 2 {
            return Err(anyhow!("mutate() expects (list, function)"));
        }
        let list_val = match &args[0] {
            Val::List(_) => args[0].clone(),
            other => {
                return Err(anyhow!(
                    "mutate() first argument must be a list, got {}",
                    other.type_name()
                ));
            }
        };
        let mutator = match &args[1] {
            f @ Val::Closure(_) | f @ Val::RustFunction(_) | f @ Val::RustFunctionNamed(_) => f.clone(),
            other => {
                return Err(anyhow!(
                    "mutate() second argument must be a function, got {}",
                    other.type_name()
                ));
            }
        };

        let guard_state = ListMutationGuardState::new(ListMutation::from_val(&list_val)?);
        let guard_handle = MutationGuardValue::new(guard_state);
        let guard_val = Val::MutationGuard(guard_handle.clone());

        let closure_result = mutator.call(std::slice::from_ref(&guard_val), ctx)?;
        let updated = guard_handle.commit()?;
        Ok((updated, closure_result))
    }
}

impl Module for ListModule {
    fn name(&self) -> &str {
        "list"
    }

    fn description(&self) -> &str {
        "List utilities and meta-methods"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        // Functions are available via module import; meta methods are registered above
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}
