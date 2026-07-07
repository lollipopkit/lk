#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow};

use crate::val::{HeapRef, RuntimeVal};

use super::{RuntimeCallable, RuntimeExport, RuntimeModuleState};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcRoots {
    refs: Vec<HeapRef>,
}

impl GcRoots {
    #[inline]
    pub const fn new() -> Self {
        Self { refs: Vec::new() }
    }

    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            refs: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    pub fn push_ref(&mut self, reference: HeapRef) {
        self.refs.push(reference);
    }

    #[inline]
    pub fn push_value(&mut self, value: &RuntimeVal) {
        if let RuntimeVal::Obj(reference) = value {
            self.push_ref(*reference);
        }
    }

    #[inline]
    pub fn extend_values<'a>(&mut self, values: impl IntoIterator<Item = &'a RuntimeVal>) {
        for value in values {
            self.push_value(value);
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[HeapRef] {
        &self.refs
    }

    #[inline]
    pub fn into_refs(self) -> Vec<HeapRef> {
        self.refs
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.refs.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }
}

pub fn collect_runtime_export(export: &RuntimeExport) -> Result<()> {
    let mut state = export.state_lock()?;
    state.collect_garbage([export.value()]);
    Ok(())
}

impl RuntimeModuleState {
    pub fn gc_roots<'a>(&self, extra_roots: impl IntoIterator<Item = &'a RuntimeVal>) -> GcRoots {
        let active_stack_end = self.stack_top.min(self.stack.len());
        let extra_roots = extra_roots.into_iter();
        let mut roots = GcRoots::with_capacity(self.globals.len() + active_stack_end + extra_roots.size_hint().0);
        roots.extend_values(&self.globals);
        roots.extend_values(&self.stack[..active_stack_end]);
        roots.extend_values(extra_roots);
        // A first-class error value unwinding toward its `pcall` must survive GC
        // even though it is no longer on the VM stack (plan M2.2).
        roots.extend_values(self.pending_raise_root.iter());
        roots
    }
}

impl RuntimeCallable {
    pub fn collect_garbage(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("RuntimeCallable state lock poisoned"))?;
        state.collect_garbage(self.captures.iter());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use crate::{
        val::{HeapStore, HeapValue},
        vm::{Module, RuntimeExport},
    };

    use super::*;

    #[test]
    fn gc_roots_collect_globals_active_stack_and_extra_values() {
        let mut state = RuntimeModuleState::default();
        state.globals = vec![RuntimeVal::Obj(HeapRef::new(1)), RuntimeVal::Int(9)];
        state.stack = vec![
            RuntimeVal::Obj(HeapRef::new(2)),
            RuntimeVal::Nil,
            RuntimeVal::Obj(HeapRef::new(3)),
        ];
        state.stack_top = 2;
        let extra = vec![RuntimeVal::Obj(HeapRef::new(4))];

        assert_eq!(
            state.gc_roots(&extra).into_refs(),
            vec![HeapRef::new(1), HeapRef::new(2), HeapRef::new(4)]
        );
    }

    #[test]
    fn collect_runtime_export_keeps_export_value_and_state_roots() {
        let mut heap = HeapStore::new();
        let exported = heap.alloc(HeapValue::String(Arc::<str>::from("exported")));
        let global = heap.alloc(HeapValue::String(Arc::<str>::from("global")));
        let dead = heap.alloc(HeapValue::String(Arc::<str>::from("dead")));
        let export = RuntimeExport::new(
            RuntimeVal::Obj(exported),
            Arc::new(crate::compat::sync::Mutex::new(RuntimeModuleState::new(
                heap,
                vec![RuntimeVal::Obj(global)],
            ))),
            Arc::new(Module::default()),
        );

        collect_runtime_export(&export).expect("collect export");
        let state = export.state_lock().expect("state");

        assert!(state.heap.get(exported).is_some());
        assert!(state.heap.get(global).is_some());
        assert!(state.heap.get(dead).is_none());
    }
}
