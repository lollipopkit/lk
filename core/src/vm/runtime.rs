use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Result, anyhow};

use crate::{
    val::{HeapRef, HeapStore, HeapValue, RuntimeVal, TypedMap},
    vm::VmContext,
};

use super::{cache::InlineCaches, ir::Module};

pub type PlainNativeFunction = fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>;
pub type ContextNativeFunction = fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>;

#[derive(Debug)]
pub struct RuntimeCallable {
    pub(crate) module: Arc<Module>,
    pub(crate) function_index: u32,
    pub(crate) captures: Arc<Vec<RuntimeVal>>,
    pub(crate) state: Arc<Mutex<RuntimeModuleState>>,
}

#[derive(Debug)]
pub struct RuntimeModuleState {
    pub(crate) heap: HeapStore,
    pub(crate) globals: Vec<RuntimeVal>,
    pub(crate) stack: Vec<RuntimeVal>,
    pub(crate) stack_top: usize,
    pub(crate) inline_caches: InlineCaches,
}

impl RuntimeModuleState {
    pub const INITIAL_STACK_CAPACITY: usize = 256;

    pub fn new(heap: HeapStore, globals: Vec<RuntimeVal>) -> Self {
        Self {
            heap,
            globals,
            stack: Vec::with_capacity(Self::INITIAL_STACK_CAPACITY),
            stack_top: 0,
            inline_caches: InlineCaches::default(),
        }
    }

    pub fn root_refs<'a>(&self, extra_roots: impl IntoIterator<Item = &'a RuntimeVal>) -> Vec<crate::val::HeapRef> {
        self.gc_roots(extra_roots).into_refs()
    }

    pub fn collect_garbage<'a>(&mut self, extra_roots: impl IntoIterator<Item = &'a RuntimeVal>) {
        self.heap.collect(self.root_refs(extra_roots));
    }

    pub fn heap(&self) -> &HeapStore {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut HeapStore {
        &mut self.heap
    }

    pub fn into_heap(self) -> HeapStore {
        self.heap
    }

    pub fn globals(&self) -> &[RuntimeVal] {
        &self.globals
    }

    pub fn globals_mut(&mut self) -> &mut Vec<RuntimeVal> {
        &mut self.globals
    }

    pub fn stack(&self) -> &[RuntimeVal] {
        &self.stack
    }

    pub fn stack_top(&self) -> usize {
        self.stack_top
    }
}

impl Default for RuntimeModuleState {
    fn default() -> Self {
        Self::new(HeapStore::new(), Vec::new())
    }
}

impl RuntimeCallable {
    pub fn with_state(
        module: Arc<Module>,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        state: Arc<Mutex<RuntimeModuleState>>,
    ) -> Self {
        Self::with_shared_captures(module, function_index, captures, state)
    }

    pub fn with_shared_captures(
        module: Arc<Module>,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        state: Arc<Mutex<RuntimeModuleState>>,
    ) -> Self {
        Self {
            module,
            function_index,
            captures,
            state,
        }
    }

    pub fn shallow_clone_shared(&self) -> Self {
        Self {
            module: Arc::clone(&self.module),
            function_index: self.function_index,
            captures: Arc::clone(&self.captures),
            state: Arc::clone(&self.state),
        }
    }

    pub fn function_index(&self) -> u32 {
        self.function_index
    }

    pub fn capture_count(&self) -> usize {
        self.captures.len()
    }

    pub fn display_signature(&self) -> String {
        let Some(function) = self.module.functions.get(self.function_index as usize) else {
            return format!("#{}", self.function_index);
        };
        let mut params = String::new();
        let param_count = function.param_count as usize;
        for index in 0..param_count {
            if index > 0 {
                params.push_str(", ");
            }
            if let Some(name) = function.param_names.get(index) {
                params.push_str(name.as_ref());
            } else {
                params.push_str(&format!("arg{index}"));
            }
        }
        format!("({params})")
    }
}

#[derive(Debug)]
pub struct RuntimeExport {
    pub(crate) value: RuntimeVal,
    pub(crate) state: Arc<Mutex<RuntimeModuleState>>,
    pub(crate) module: Arc<Module>,
}

impl RuntimeExport {
    pub fn new(value: RuntimeVal, state: Arc<Mutex<RuntimeModuleState>>, module: Arc<Module>) -> Self {
        Self { value, state, module }
    }

    pub fn from_value(value: RuntimeVal, heap: HeapStore) -> Self {
        Self::new(
            value,
            Arc::new(Mutex::new(RuntimeModuleState::new(heap, Vec::new()))),
            Arc::new(Module::default()),
        )
    }

    pub fn value(&self) -> &RuntimeVal {
        &self.value
    }

    pub fn shallow_clone_shared(&self) -> Self {
        Self {
            value: self.value,
            state: Arc::clone(&self.state),
            module: Arc::clone(&self.module),
        }
    }

    pub fn shared_state(&self) -> Arc<Mutex<RuntimeModuleState>> {
        Arc::clone(&self.state)
    }

    pub fn shared_module(&self) -> Arc<Module> {
        Arc::clone(&self.module)
    }

    pub fn state_lock(&self) -> Result<MutexGuard<'_, RuntimeModuleState>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("RuntimeExport state lock poisoned"))
    }
}

enum NativeRuntimeStorage<'a> {
    State(&'a mut RuntimeModuleState),
    Parts {
        heap: &'a mut HeapStore,
        globals: &'a [RuntimeVal],
    },
}

pub struct NativeRuntime<'a> {
    storage: NativeRuntimeStorage<'a>,
    ctx: Option<&'a mut VmContext>,
    module: Option<&'a Module>,
    shared_module: Option<Arc<Module>>,
}

impl<'a> NativeRuntime<'a> {
    #[inline]
    pub fn new(state: &'a mut RuntimeModuleState, ctx: Option<&'a mut VmContext>, module: Option<&'a Module>) -> Self {
        Self {
            storage: NativeRuntimeStorage::State(state),
            ctx,
            module,
            shared_module: None,
        }
    }

    #[inline]
    pub fn new_with_shared_module(
        state: &'a mut RuntimeModuleState,
        ctx: Option<&'a mut VmContext>,
        module: Arc<Module>,
    ) -> Self {
        Self {
            storage: NativeRuntimeStorage::State(state),
            ctx,
            module: None,
            shared_module: Some(module),
        }
    }

    #[inline]
    pub fn from_parts(
        heap: &'a mut HeapStore,
        globals: &'a [RuntimeVal],
        ctx: Option<&'a mut VmContext>,
        module: Option<&'a Module>,
    ) -> Self {
        Self {
            storage: NativeRuntimeStorage::Parts { heap, globals },
            ctx,
            module,
            shared_module: None,
        }
    }

    #[inline]
    pub fn from_parts_with_shared_module(
        heap: &'a mut HeapStore,
        globals: &'a [RuntimeVal],
        ctx: Option<&'a mut VmContext>,
        module: Arc<Module>,
    ) -> Self {
        Self {
            storage: NativeRuntimeStorage::Parts { heap, globals },
            ctx,
            module: None,
            shared_module: Some(module),
        }
    }

    #[inline]
    pub fn state_ctx_module_mut(
        &mut self,
    ) -> Option<(&mut RuntimeModuleState, Option<&mut VmContext>, Option<&Module>)> {
        let module = self.shared_module.as_deref().or(self.module);
        match &mut self.storage {
            NativeRuntimeStorage::State(state) => Some((*state, self.ctx.as_deref_mut(), module)),
            NativeRuntimeStorage::Parts { .. } => None,
        }
    }

    #[inline]
    pub(crate) fn parts_mut(&mut self) -> Option<(&mut RuntimeModuleState, Option<&mut VmContext>, Option<&Module>)> {
        self.state_ctx_module_mut()
    }

    #[inline]
    pub fn heap_ctx_mut(&mut self) -> (&mut HeapStore, Option<&mut VmContext>) {
        let heap = match &mut self.storage {
            NativeRuntimeStorage::State(state) => &mut state.heap,
            NativeRuntimeStorage::Parts { heap, .. } => *heap,
        };
        (heap, self.ctx.as_deref_mut())
    }

    #[inline]
    pub fn heap(&self) -> &HeapStore {
        match &self.storage {
            NativeRuntimeStorage::State(state) => &state.heap,
            NativeRuntimeStorage::Parts { heap, .. } => heap,
        }
    }

    #[inline]
    pub fn heap_mut(&mut self) -> &mut HeapStore {
        match &mut self.storage {
            NativeRuntimeStorage::State(state) => &mut state.heap,
            NativeRuntimeStorage::Parts { heap, .. } => heap,
        }
    }

    #[inline]
    pub fn globals(&self) -> &[RuntimeVal] {
        match &self.storage {
            NativeRuntimeStorage::State(state) => &state.globals,
            NativeRuntimeStorage::Parts { globals, .. } => globals,
        }
    }

    #[inline]
    pub fn module(&self) -> Option<&Module> {
        self.shared_module.as_deref().or(self.module)
    }

    #[inline]
    pub fn shared_module(&self) -> Option<Arc<Module>> {
        self.shared_module.as_ref().map(Arc::clone)
    }

    #[inline]
    pub fn ctx(&self) -> Option<&VmContext> {
        self.ctx.as_deref()
    }

    /// The async (tokio) runtime handle for this call, taken from the VM
    /// context. Returns an independent handle when there is no context (e.g.
    /// native-compilation shims), so callers never reach for a global.
    #[inline]
    pub fn async_runtime(&self) -> crate::rt::AsyncRuntimeHandle {
        self.ctx
            .as_deref()
            .map(|ctx| ctx.async_runtime().clone())
            .unwrap_or_default()
    }

    #[inline]
    pub fn ctx_mut(&mut self) -> Option<&mut VmContext> {
        self.ctx.as_deref_mut()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NativeArgs<'a> {
    values: &'a [RuntimeVal],
    named: NativeNamedArgs<'a>,
}

#[derive(Clone, Copy, Debug)]
enum NativeNamedArgs<'a> {
    Empty,
    Stack {
        stack: &'a [RuntimeVal],
        start: usize,
        count: u16,
    },
    MapHandle {
        handle: HeapRef,
        count: usize,
    },
}

impl<'a> NativeArgs<'a> {
    #[inline]
    pub const fn new(values: &'a [RuntimeVal]) -> Self {
        Self {
            values,
            named: NativeNamedArgs::Empty,
        }
    }

    #[inline]
    pub const fn new_with_named_stack(
        values: &'a [RuntimeVal],
        stack: &'a [RuntimeVal],
        start: usize,
        count: u16,
    ) -> Self {
        Self {
            values,
            named: NativeNamedArgs::Stack { stack, start, count },
        }
    }

    #[inline]
    pub const fn new_with_named_map_handle(values: &'a [RuntimeVal], handle: HeapRef, count: usize) -> Self {
        Self {
            values,
            named: NativeNamedArgs::MapHandle { handle, count },
        }
    }

    #[inline]
    pub const fn len(self) -> usize {
        self.values.len()
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.values.is_empty()
    }

    #[inline]
    pub const fn as_slice(self) -> &'a [RuntimeVal] {
        self.values
    }

    #[inline]
    pub fn named_len(self) -> usize {
        match self.named {
            NativeNamedArgs::Empty => 0,
            NativeNamedArgs::Stack { count, .. } => count as usize,
            NativeNamedArgs::MapHandle { count, .. } => count,
        }
    }

    pub fn has_named(self) -> bool {
        self.named_len() != 0
    }

    pub fn try_for_each_named(
        self,
        heap: &HeapStore,
        mut f: impl FnMut(&str, &RuntimeVal) -> Result<()>,
    ) -> Result<()> {
        match self.named {
            NativeNamedArgs::Empty => Ok(()),
            NativeNamedArgs::Stack { stack, start, count } => {
                let end = start + count as usize * 2;
                let Some(named_slots) = stack.get(start..end) else {
                    anyhow::bail!("native named argument window {start}..{end} out of bounds");
                };
                for pair in named_slots.chunks_exact(2) {
                    let name = runtime_named_arg_name(&pair[0], heap)?;
                    f(name, &pair[1])?;
                }
                Ok(())
            }
            NativeNamedArgs::MapHandle { handle, .. } => {
                let Some(HeapValue::Map(map)) = heap.get(handle) else {
                    anyhow::bail!("native named argument map {} is not a live map", handle.index());
                };
                for_each_typed_map_named_arg(map, &mut f)?;
                Ok(())
            }
        }
    }

    #[inline]
    pub fn get(self, index: usize) -> Option<&'a RuntimeVal> {
        self.values.get(index)
    }
}

fn for_each_typed_map_named_arg<'a>(
    map: &'a TypedMap,
    f: &mut impl FnMut(&'a str, &RuntimeVal) -> Result<()>,
) -> Result<()> {
    match map {
        TypedMap::StringMixed(values) => {
            for (name, value) in values {
                f(name.as_ref(), value)?;
            }
        }
        TypedMap::StringInt(values) => {
            for (name, value) in values {
                let value = RuntimeVal::Int(*value);
                f(name.as_ref(), &value)?;
            }
        }
        TypedMap::StringFloat(values) => {
            for (name, value) in values {
                let value = RuntimeVal::Float(*value);
                f(name.as_ref(), &value)?;
            }
        }
        TypedMap::StringBool(values) => {
            for (name, value) in values {
                let value = RuntimeVal::Bool(*value);
                f(name.as_ref(), &value)?;
            }
        }
        TypedMap::Mixed(values) => {
            for (key, value) in values {
                let Some(name) = key.as_str() else {
                    anyhow::bail!("native named argument key must be a string");
                };
                f(name, value)?;
            }
        }
    }
    Ok(())
}

fn runtime_named_arg_name<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Result<&'a str> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(value.as_str()),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow::anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(value.as_ref()),
            _ => anyhow::bail!("native named argument name must be a string"),
        },
        _ => anyhow::bail!("native named argument name must be a string"),
    }
}

impl<'a> IntoIterator for NativeArgs<'a> {
    type Item = &'a RuntimeVal;
    type IntoIter = std::slice::Iter<'a, RuntimeVal>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

#[derive(Clone, Debug)]
pub enum NativeFunction {
    Plain(PlainNativeFunction),
    Context(ContextNativeFunction),
    FullState(ContextNativeFunction),
}

impl NativeFunction {
    #[inline]
    pub const fn requires_full_state(&self) -> bool {
        matches!(self, Self::FullState(_))
    }
}

#[derive(Clone, Debug)]
pub struct NativeEntry {
    pub name: String,
    pub arity: u16,
    pub function: NativeFunction,
}

impl NativeEntry {
    pub const VARIADIC: u16 = u16::MAX;

    #[inline]
    pub const fn accepts_arity(&self, arg_count: u16) -> bool {
        self.arity == Self::VARIADIC || self.arity == arg_count
    }
}

#[cfg(test)]
mod tests {
    use crate::util::fast_map::fast_hash_map_from_iter;
    use std::sync::Arc;

    use crate::val::{HeapStore, HeapValue, RuntimeVal, TypedMap};

    use super::*;

    #[test]
    fn runtime_module_state_starts_with_shared_stack_capacity() {
        let state = RuntimeModuleState::new(HeapStore::new(), vec![RuntimeVal::Int(7)]);

        assert_eq!(state.globals, vec![RuntimeVal::Int(7)]);
        assert_eq!(state.stack_top, 0);
        assert!(state.stack.is_empty());
        assert!(state.stack.capacity() >= RuntimeModuleState::INITIAL_STACK_CAPACITY);
    }

    #[test]
    fn runtime_callable_shared_clone_keeps_module_captures_and_state_shared() {
        let module = Arc::new(Module::default());
        let state = Arc::new(Mutex::new(RuntimeModuleState::new(HeapStore::new(), Vec::new())));
        let captures = Arc::new(vec![RuntimeVal::Int(1)]);
        let callable = RuntimeCallable::with_state(Arc::clone(&module), 3, Arc::clone(&captures), Arc::clone(&state));
        let cloned = callable.shallow_clone_shared();

        assert!(Arc::ptr_eq(&callable.module, &cloned.module));
        assert!(Arc::ptr_eq(&callable.captures, &cloned.captures));
        assert!(Arc::ptr_eq(&captures, &callable.captures));
        assert!(Arc::ptr_eq(&callable.state, &cloned.state));
        assert_eq!(cloned.function_index, 3);
    }

    #[test]
    fn runtime_export_shared_clone_keeps_module_and_state_shared() {
        let module = Arc::new(Module::default());
        let state = Arc::new(Mutex::new(RuntimeModuleState::new(
            HeapStore::new(),
            vec![RuntimeVal::Int(1)],
        )));
        let export = RuntimeExport::new(RuntimeVal::Int(7), Arc::clone(&state), Arc::clone(&module));
        let cloned = export.shallow_clone_shared();

        assert_eq!(cloned.value(), &RuntimeVal::Int(7));
        assert!(Arc::ptr_eq(&export.state, &cloned.state));
        assert!(Arc::ptr_eq(&export.module, &cloned.module));
    }

    #[test]
    fn native_args_and_runtime_parts_borrow_heap_and_globals_without_state() {
        let args = [RuntimeVal::Int(1), RuntimeVal::Bool(true)];
        let named = [
            RuntimeVal::ShortStr(crate::val::ShortStr::new("flag").expect("short")),
            RuntimeVal::Bool(false),
        ];
        let native_args = NativeArgs::new_with_named_stack(&args, &named, 0, 1);

        assert_eq!(native_args.len(), 2);
        assert_eq!(native_args.get(0), Some(&RuntimeVal::Int(1)));
        assert_eq!(native_args.named_len(), 1);
        let mut seen = Vec::new();
        let heap = HeapStore::new();
        native_args
            .try_for_each_named(&heap, |name, value| {
                seen.push((name.to_string(), value.clone()));
                Ok(())
            })
            .expect("iterate named");
        assert_eq!(seen, vec![("flag".to_string(), RuntimeVal::Bool(false))]);

        let mut heap = HeapStore::new();
        let named_handle = heap.alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([(
            Arc::<str>::from("limit"),
            7,
        )]))));
        let native_args = NativeArgs::new_with_named_map_handle(&args, named_handle, 1);
        assert_eq!(native_args.named_len(), 1);
        let mut seen = Vec::new();
        native_args
            .try_for_each_named(&heap, |name, value| {
                seen.push((name.to_string(), value.clone()));
                Ok(())
            })
            .expect("iterate map-handle named");
        assert_eq!(seen, vec![("limit".to_string(), RuntimeVal::Int(7))]);

        let mut heap = HeapStore::new();
        let globals = [RuntimeVal::Int(9)];
        let mut runtime = NativeRuntime::from_parts(&mut heap, &globals, None, None);
        let handle = runtime.heap_mut().alloc(HeapValue::String(Arc::<str>::from("runtime")));

        assert!(matches!(
            runtime.heap().get(handle),
            Some(HeapValue::String(value)) if value.as_ref() == "runtime"
        ));
        assert_eq!(runtime.globals(), &globals);
        assert!(runtime.parts_mut().is_none());
    }

    #[test]
    fn shared_module_only_reports_arc_backed_module() {
        let mut state = RuntimeModuleState::default();
        let borrowed_module = Module::default();
        let borrowed_runtime = NativeRuntime::new(&mut state, None, Some(&borrowed_module));
        assert!(borrowed_runtime.module().is_some());
        assert!(borrowed_runtime.shared_module().is_none());

        let shared = Arc::new(Module::default());
        let shared_runtime = NativeRuntime::new_with_shared_module(&mut state, None, Arc::clone(&shared));
        let observed = shared_runtime.shared_module().expect("shared module");
        assert!(Arc::ptr_eq(&observed, &shared));
    }
}
