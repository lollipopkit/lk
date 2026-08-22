use super::{Executor, handler::ErrorHandler};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

impl Executor {
    pub(super) fn alloc_heap_value(&mut self, value: crate::val::HeapValue) -> crate::val::HeapRef {
        let handle = self.state.heap.alloc(value);
        if self.state.heap.should_collect() {
            self.gc_pending = true;
        }
        handle
    }

    pub(crate) fn root_refs(&self) -> Vec<crate::val::HeapRef> {
        let handler_roots = self.handler_stack.iter().flat_map(ErrorHandler::roots);
        // Ancestor frames' captures (plan M2.5 sub-step ①: flattened LK→LK
        // calls no longer keep them alive implicitly on the Rust stack) must
        // be rooted explicitly here, same as the current frame's `captures`.
        let frame_roots = self
            .frames
            .iter()
            .flat_map(|frame| frame.captures.iter().flat_map(|captures| captures.iter()));
        self.state
            .gc_roots(
                self.captures
                    .iter()
                    .flat_map(|captures| captures.iter())
                    .chain(frame_roots)
                    .chain(handler_roots),
            )
            .into_refs()
    }

    /// The collection itself, reached only once a safepoint has decided there
    /// is something to do.
    ///
    /// The test used to live *inside* this function while the function was
    /// `#[cold]`, which put the outline boundary in the wrong place: every
    /// safepoint made a real call into the cold section just to load two bools
    /// and come back. A call/return pair passes two safepoints, so the empty
    /// call benchmark spent ~3% of its time there. [`Executor::safepoint`] now
    /// holds the test and this stays cold, which is what `#[cold]` is for.
    #[cold]
    fn collect_now(&mut self) {
        let roots = self.root_refs();
        self.state.heap.collect(roots);
        self.gc_pending = false;
    }

    pub(super) fn sync_heap_gc_threshold(&mut self) {
        if self.state.heap.should_collect() {
            self.gc_pending = true;
        }
    }

    /// Collect unconditionally (regardless of the allocation threshold), using
    /// the full live-root set. Used by the heap-object sandbox limit: the live
    /// counter includes garbage not yet swept, so a limit check collects first
    /// and only fails if the *reachable* set is still over budget.
    #[cold]
    pub(super) fn force_collect(&mut self) {
        self.collect_now();
    }

    /// GC safepoint run after allocation-heavy opcodes and at call boundaries:
    /// reclaim pending garbage, then enforce the process **byte** budget (plan
    /// M2.6, `LK_MAX_HEAP_BYTES`).
    /// A non-allocating, non-calling hot loop never reaches a safepoint, so it
    /// pays nothing;
    /// [`mem::over_limit`](crate::mem::over_limit) short-circuits to a single
    /// atomic load when the limit is unset. When over budget, collect once more
    /// (returning freed VM memory to the allocator) and abort execution (a hard
    /// sandbox stop, like fuel) only if the *reachable* footprint is still over.
    #[inline]
    pub(super) fn safepoint(&mut self) -> anyhow::Result<()> {
        // Stress mode collects at every safepoint (not at allocation sites —
        // fresh handles are only rooted once the handler writes them to a
        // register), so a missed root fails deterministically in any test run.
        if self.gc_pending || self.gc_stress {
            self.collect_now();
        }
        if crate::mem::over_limit() {
            self.enforce_memory_limit()?;
        }
        Ok(())
    }

    #[cold]
    fn enforce_memory_limit(&mut self) -> anyhow::Result<()> {
        self.force_collect();
        if crate::mem::over_limit() {
            anyhow::bail!(
                "memory limit exceeded ({} bytes used, limit {} bytes)",
                crate::mem::used(),
                crate::mem::limit()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use crate::val::{HeapValue, RuntimeVal};

    use super::Executor;

    #[test]
    fn executor_root_refs_include_runtime_state_and_captures() {
        let mut executor = Executor::new(2);
        let global = executor.state.heap.alloc(HeapValue::String(Arc::<str>::from("global")));
        let stack = executor.state.heap.alloc(HeapValue::String(Arc::<str>::from("stack")));
        let inactive_stack = executor
            .state
            .heap
            .alloc(HeapValue::String(Arc::<str>::from("inactive")));
        let capture = executor
            .state
            .heap
            .alloc(HeapValue::String(Arc::<str>::from("capture")));
        executor.state.globals = vec![RuntimeVal::Obj(global)];
        executor.state.stack = vec![
            RuntimeVal::Obj(stack),
            RuntimeVal::Int(1),
            RuntimeVal::Obj(inactive_stack),
        ];
        executor.state.stack_top = 2;
        executor.captures = Some(Arc::new(vec![RuntimeVal::Obj(capture)]));

        assert_eq!(executor.root_refs(), vec![global, stack, capture]);
    }
}
