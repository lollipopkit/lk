use super::{Executor, handler::ErrorHandler};

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
        self.state
            .gc_roots(self.captures.iter().chain(handler_roots))
            .into_refs()
    }

    #[cold]
    #[inline]
    pub(super) fn collect_pending_garbage(&mut self) {
        if self.gc_pending {
            let roots = self.root_refs();
            self.state.heap.collect(roots);
            self.gc_pending = false;
        }
    }

    pub(super) fn sync_heap_gc_threshold(&mut self) {
        if self.state.heap.should_collect() {
            self.gc_pending = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
        executor.captures = Arc::new(vec![RuntimeVal::Obj(capture)]);

        assert_eq!(executor.root_refs(), vec![global, stack, capture]);
    }
}
