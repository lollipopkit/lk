use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::{
    val::{HeapStore, RuntimeVal},
    vm::VmContext,
};

use super::ir32::Module32;

pub type PlainNativeFunction32 = fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>;
pub type ContextNativeFunction32 = fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>;

#[derive(Debug)]
pub struct RuntimeCallable32 {
    pub module: Arc<Module32>,
    pub function_index: u32,
    pub captures: Arc<Vec<RuntimeVal>>,
    pub state: Arc<Mutex<RuntimeModuleState32>>,
}

#[derive(Clone, Debug)]
pub struct RuntimeModuleState32 {
    pub heap: HeapStore,
    pub globals: Vec<RuntimeVal>,
    pub stack: Vec<RuntimeVal>,
    pub stack_top: usize,
}

impl RuntimeModuleState32 {
    pub const INITIAL_STACK_CAPACITY: usize = 256;

    pub fn new(heap: HeapStore, globals: Vec<RuntimeVal>) -> Self {
        Self {
            heap,
            globals,
            stack: Vec::with_capacity(Self::INITIAL_STACK_CAPACITY),
            stack_top: 0,
        }
    }

    pub fn root_refs<'a>(&self, extra_roots: impl IntoIterator<Item = &'a RuntimeVal>) -> Vec<crate::val::HeapRef> {
        self.gc_roots(extra_roots).into_refs()
    }

    pub fn collect_garbage<'a>(&mut self, extra_roots: impl IntoIterator<Item = &'a RuntimeVal>) {
        self.heap.collect(self.root_refs(extra_roots));
    }
}

impl Default for RuntimeModuleState32 {
    fn default() -> Self {
        Self::new(HeapStore::new(), Vec::new())
    }
}

impl RuntimeCallable32 {
    pub fn new(
        module: Arc<Module32>,
        function_index: u32,
        captures: Vec<RuntimeVal>,
        heap: HeapStore,
        globals: Vec<RuntimeVal>,
    ) -> Self {
        Self::with_state(
            module,
            function_index,
            captures,
            Arc::new(Mutex::new(RuntimeModuleState32::new(heap, globals))),
        )
    }

    pub fn with_state(
        module: Arc<Module32>,
        function_index: u32,
        captures: Vec<RuntimeVal>,
        state: Arc<Mutex<RuntimeModuleState32>>,
    ) -> Self {
        Self {
            module,
            function_index,
            captures: Arc::new(captures),
            state,
        }
    }
}

impl Clone for RuntimeCallable32 {
    fn clone(&self) -> Self {
        Self {
            module: Arc::clone(&self.module),
            function_index: self.function_index,
            captures: Arc::clone(&self.captures),
            state: Arc::clone(&self.state),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeExport32 {
    pub value: RuntimeVal,
    pub state: Arc<Mutex<RuntimeModuleState32>>,
    pub module: Arc<Module32>,
}

impl RuntimeExport32 {
    pub fn from_value(value: RuntimeVal, heap: HeapStore) -> Self {
        Self {
            value,
            state: Arc::new(Mutex::new(RuntimeModuleState32::new(heap, Vec::new()))),
            module: Arc::new(Module32::default()),
        }
    }
}

enum NativeRuntimeStorage32<'a> {
    State(&'a mut RuntimeModuleState32),
    Parts {
        heap: &'a mut HeapStore,
        globals: &'a [RuntimeVal],
    },
}

pub struct NativeRuntime32<'a> {
    storage: NativeRuntimeStorage32<'a>,
    ctx: Option<&'a mut VmContext>,
    module: Option<&'a Module32>,
}

impl<'a> NativeRuntime32<'a> {
    #[inline]
    pub fn new(
        state: &'a mut RuntimeModuleState32,
        ctx: Option<&'a mut VmContext>,
        module: Option<&'a Module32>,
    ) -> Self {
        Self {
            storage: NativeRuntimeStorage32::State(state),
            ctx,
            module,
        }
    }

    #[inline]
    pub fn from_parts(
        heap: &'a mut HeapStore,
        globals: &'a [RuntimeVal],
        ctx: Option<&'a mut VmContext>,
        module: Option<&'a Module32>,
    ) -> Self {
        Self {
            storage: NativeRuntimeStorage32::Parts { heap, globals },
            ctx,
            module,
        }
    }

    #[inline]
    pub(crate) fn parts_mut(
        &mut self,
    ) -> Option<(&mut RuntimeModuleState32, Option<&mut VmContext>, Option<&Module32>)> {
        match &mut self.storage {
            NativeRuntimeStorage32::State(state) => Some((*state, self.ctx.as_deref_mut(), self.module)),
            NativeRuntimeStorage32::Parts { .. } => None,
        }
    }

    #[inline]
    pub fn heap_ctx_mut(&mut self) -> (&mut HeapStore, Option<&mut VmContext>) {
        let heap = match &mut self.storage {
            NativeRuntimeStorage32::State(state) => &mut state.heap,
            NativeRuntimeStorage32::Parts { heap, .. } => *heap,
        };
        (heap, self.ctx.as_deref_mut())
    }

    #[inline]
    pub fn heap(&self) -> &HeapStore {
        match &self.storage {
            NativeRuntimeStorage32::State(state) => &state.heap,
            NativeRuntimeStorage32::Parts { heap, .. } => heap,
        }
    }

    #[inline]
    pub fn heap_mut(&mut self) -> &mut HeapStore {
        match &mut self.storage {
            NativeRuntimeStorage32::State(state) => &mut state.heap,
            NativeRuntimeStorage32::Parts { heap, .. } => heap,
        }
    }

    #[inline]
    pub fn globals(&self) -> &[RuntimeVal] {
        match &self.storage {
            NativeRuntimeStorage32::State(state) => &state.globals,
            NativeRuntimeStorage32::Parts { globals, .. } => globals,
        }
    }

    #[inline]
    pub fn module(&self) -> Option<&Module32> {
        self.module
    }

    #[inline]
    pub fn ctx(&self) -> Option<&VmContext> {
        self.ctx.as_deref()
    }

    #[inline]
    pub fn ctx_mut(&mut self) -> Option<&mut VmContext> {
        self.ctx.as_deref_mut()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NativeArgs32<'a> {
    values: &'a [RuntimeVal],
    named: &'a [(Arc<str>, RuntimeVal)],
}

impl<'a> NativeArgs32<'a> {
    #[inline]
    pub const fn new(values: &'a [RuntimeVal]) -> Self {
        Self { values, named: &[] }
    }

    #[inline]
    pub const fn new_with_named(values: &'a [RuntimeVal], named: &'a [(Arc<str>, RuntimeVal)]) -> Self {
        Self { values, named }
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
    pub const fn named(self) -> &'a [(Arc<str>, RuntimeVal)] {
        self.named
    }

    #[inline]
    pub fn get(self, index: usize) -> Option<&'a RuntimeVal> {
        self.values.get(index)
    }
}

impl<'a> IntoIterator for NativeArgs32<'a> {
    type Item = &'a RuntimeVal;
    type IntoIter = std::slice::Iter<'a, RuntimeVal>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

#[derive(Clone, Debug)]
pub enum NativeFunction32 {
    Plain(PlainNativeFunction32),
    Context(ContextNativeFunction32),
    FullState(ContextNativeFunction32),
    RuntimeCallable(Arc<RuntimeCallable32>),
}

impl NativeFunction32 {
    #[inline]
    pub const fn requires_full_state(&self) -> bool {
        matches!(self, Self::FullState(_))
    }
}

#[derive(Clone, Debug)]
pub struct NativeEntry32 {
    pub name: String,
    pub arity: u16,
    pub function: NativeFunction32,
}

impl NativeEntry32 {
    pub const VARIADIC: u16 = u16::MAX;

    #[inline]
    pub const fn accepts_arity(&self, arg_count: u16) -> bool {
        self.arity == Self::VARIADIC || self.arity == arg_count
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::val::{HeapStore, HeapValue, RuntimeVal};

    use super::*;

    #[test]
    fn runtime_module_state_starts_with_shared_stack_capacity() {
        let state = RuntimeModuleState32::new(HeapStore::new(), vec![RuntimeVal::Int(7)]);

        assert_eq!(state.globals, vec![RuntimeVal::Int(7)]);
        assert_eq!(state.stack_top, 0);
        assert!(state.stack.is_empty());
        assert!(state.stack.capacity() >= RuntimeModuleState32::INITIAL_STACK_CAPACITY);
    }

    #[test]
    fn runtime_callable_clones_share_module_and_state() {
        let module = Arc::new(Module32::default());
        let callable = RuntimeCallable32::new(
            Arc::clone(&module),
            3,
            vec![RuntimeVal::Int(1)],
            HeapStore::new(),
            Vec::new(),
        );
        let cloned = callable.clone();

        assert!(Arc::ptr_eq(&callable.module, &cloned.module));
        assert!(Arc::ptr_eq(&callable.captures, &cloned.captures));
        assert!(Arc::ptr_eq(&callable.state, &cloned.state));
        assert_eq!(cloned.function_index, 3);
    }

    #[test]
    fn native_args_and_runtime_parts_borrow_heap_and_globals_without_state() {
        let args = [RuntimeVal::Int(1), RuntimeVal::Bool(true)];
        let named = [(Arc::<str>::from("flag"), RuntimeVal::Bool(false))];
        let native_args = NativeArgs32::new_with_named(&args, &named);

        assert_eq!(native_args.len(), 2);
        assert_eq!(native_args.get(0), Some(&RuntimeVal::Int(1)));
        assert_eq!(native_args.named()[0].0.as_ref(), "flag");

        let mut heap = HeapStore::new();
        let globals = [RuntimeVal::Int(9)];
        let mut runtime = NativeRuntime32::from_parts(&mut heap, &globals, None, None);
        let handle = runtime.heap_mut().alloc(HeapValue::String(Arc::<str>::from("runtime")));

        assert!(matches!(
            runtime.heap().get(handle),
            Some(HeapValue::String(value)) if value.as_ref() == "runtime"
        ));
        assert_eq!(runtime.globals(), &globals);
        assert!(runtime.parts_mut().is_none());
    }
}
