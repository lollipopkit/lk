#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::Result;

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
        // Values host (native) functions hold across re-entrant VM calls —
        // e.g. an HOF's accumulated callback results (see `host_roots`).
        roots.extend_values(&self.host_roots);
        roots
    }
}

impl RuntimeCallable {
    /// Collect the heap this callable's *own* module owns.
    ///
    /// Reached from [`HeapStore::collect`](crate::val::HeapStore::collect) when
    /// marking a heap that holds an imported function: the function's captures
    /// live in the exporting module's heap, not in the one being marked, so
    /// that heap has to be collected against its own roots.
    ///
    /// `try_lock`, not `lock`. This walk can arrive back at a state that is
    /// already being collected further up the stack, and neither backing mutex
    /// is re-entrant — `lock` would hang the process with no error and no
    /// output. Today the import graph is a DAG (`ModuleResolver` rejects
    /// circular imports by path), so that cannot happen; but nothing here
    /// depends on that check, or would notice if it were relaxed. Skipping is
    /// the conservative answer either way: the heap keeps its objects until the
    /// collection already in progress, or the next one, reaches them.
    pub fn collect_garbage(&self) -> Result<()> {
        let Some(mut state) = self.state.try_lock() else {
            return Ok(());
        };
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
        let mut state = RuntimeModuleState {
            globals: vec![RuntimeVal::Obj(HeapRef::new(1)), RuntimeVal::Int(9)],
            ..Default::default()
        };
        state.stack = vec![
            RuntimeVal::Obj(HeapRef::new(2)),
            RuntimeVal::Nil,
            RuntimeVal::Obj(HeapRef::new(3)),
        ];
        state.stack_top = 2;
        state.host_root_push(RuntimeVal::Obj(HeapRef::new(5)));
        let extra = vec![RuntimeVal::Obj(HeapRef::new(4))];

        assert_eq!(
            state.gc_roots(&extra).into_refs(),
            vec![HeapRef::new(1), HeapRef::new(2), HeapRef::new(4), HeapRef::new(5)]
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
