use std::sync::Arc;

use crate::val::{ClosureCapture, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::RegionAllocator;

use super::super::bytecode::{CaptureSpec, Function};
use super::Vm;

#[derive(Debug, Clone, Copy)]
pub(super) struct CallFrameMeta {
    pub(super) resume_pc: usize,
    pub(super) ret_base: u16,
    pub(super) retc: u8,
    pub(super) caller_window: RegisterWindowRef,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RegisterWindowRef {
    Current,
    StackIndex(usize),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RegisterSpan {
    pub(super) base: usize,
    pub(super) len: usize,
    pub(super) window: RegisterWindowRef,
}

impl RegisterSpan {
    pub(super) const fn new(base: usize, len: usize, window: RegisterWindowRef) -> Self {
        Self { base, len, window }
    }

    pub(super) const fn current(base: usize, len: usize) -> Self {
        Self::new(base, len, RegisterWindowRef::Current)
    }

    pub(super) fn relocate(self, window: RegisterWindowRef) -> Self {
        let relocated = match self.window {
            RegisterWindowRef::Current => window,
            RegisterWindowRef::StackIndex(idx) => RegisterWindowRef::StackIndex(idx),
        };
        Self {
            window: relocated,
            ..self
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CallArgs {
    span: RegisterSpan,
}

impl CallArgs {
    pub(super) fn registers(span: RegisterSpan) -> Self {
        Self { span }
    }

    pub(super) fn len(&self) -> usize {
        self.span.len
    }

    pub(super) fn span(&self) -> RegisterSpan {
        self.span
    }
}

#[derive(Debug)]
pub(super) struct CallFrame<'func> {
    pub(super) func: &'func Function,
    pub(super) pc: usize,
    pub(super) reg_base: usize,
    pub(super) reg_count: usize,
    pub(super) captures: Option<Arc<ClosureCapture>>,
    pub(super) capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    #[allow(dead_code)]
    pub(super) region_plan: Option<Arc<RegionPlan>>,
}

impl<'func> CallFrame<'func> {
    pub(super) fn new(
        func: &'func Function,
        reg_base: usize,
        reg_count: usize,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        region_plan: Option<Arc<RegionPlan>>,
    ) -> Self {
        Self {
            func,
            pc: 0,
            reg_base,
            reg_count,
            captures,
            capture_specs,
            region_plan,
        }
    }
}

#[derive(Clone)]
pub(crate) struct FrameInfo {
    pub(crate) name: Arc<str>,
    pub(crate) location: Option<Arc<str>>,
}

impl FrameInfo {
    pub(crate) fn new<N, L>(name: N, location: Option<L>) -> Self
    where
        N: Into<Arc<str>>,
        L: Into<Arc<str>>,
    {
        Self {
            name: name.into(),
            location: location.map(Into::into),
        }
    }
}

pub(super) struct CallFrameStackGuard {
    vm: *mut Vm,
}

impl CallFrameStackGuard {
    pub(super) fn push(vm: *mut Vm, meta: CallFrameMeta) -> Self {
        unsafe {
            (*vm).frames.push(meta);
        }
        Self { vm }
    }
}

impl Drop for CallFrameStackGuard {
    fn drop(&mut self) {
        unsafe {
            let vm = &mut *self.vm;
            let _ = vm.frames.pop();
        }
    }
}

pub(super) struct FrameState<'func> {
    pub(super) func: &'func Function,
    pub(super) pc: usize,
    pub(super) regs: &'func mut Vec<Val>,
    pub(super) reg_base: usize,
    pub(super) reg_count: usize,
    pub(super) frame_ptr: *mut CallFrame<'func>,
    pub(super) captures: Option<Arc<ClosureCapture>>,
    pub(super) capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    pub(super) region_plan: Option<Arc<RegionPlan>>,
    region_allocator: *const RegionAllocator,
    inline_return_meta: Option<CallFrameMeta>,
    reg_write_high_water: usize,
}

impl<'func> FrameState<'func> {
    pub(super) fn new(
        frame: &mut CallFrame<'func>,
        regs: &'func mut Vec<Val>,
        region_allocator: *const RegionAllocator,
    ) -> Self {
        Self {
            func: frame.func,
            pc: frame.pc,
            regs,
            reg_base: frame.reg_base,
            reg_count: frame.reg_count,
            frame_ptr: frame as *mut _,
            captures: frame.captures.clone(),
            capture_specs: frame.capture_specs.clone(),
            region_plan: frame.region_plan.clone(),
            region_allocator,
            inline_return_meta: None,
            reg_write_high_water: 0,
        }
    }

    #[inline]
    pub(super) fn func(&self) -> &'func Function {
        self.func
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn pc(&self) -> usize {
        self.pc
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn set_pc(&mut self, pc: usize) {
        self.pc = pc;
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn reg_base(&self) -> usize {
        self.reg_base
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn reg_count(&self) -> usize {
        self.reg_count
    }

    #[inline]
    pub(super) fn regs(&mut self) -> &mut Vec<Val> {
        &mut *self.regs
    }

    #[inline]
    pub(super) fn capture_arc(&self) -> Option<Arc<ClosureCapture>> {
        self.captures.as_ref().map(Arc::clone)
    }

    #[inline]
    pub(super) fn capture_specs_arc(&self) -> Option<Arc<Vec<CaptureSpec>>> {
        self.capture_specs.as_ref().map(Arc::clone)
    }

    #[inline]
    pub(super) fn set_inline_return_meta(&mut self, meta: CallFrameMeta) {
        self.inline_return_meta = Some(meta);
    }

    #[inline]
    pub(super) fn take_inline_return_meta(&mut self) -> Option<CallFrameMeta> {
        self.inline_return_meta.take()
    }

    #[inline]
    pub(super) fn record_reg_write(&mut self, idx: usize) {
        let next = idx + 1;
        if next > self.reg_write_high_water {
            self.reg_write_high_water = next;
        }
    }

    #[inline]
    pub(super) fn take_reg_write_high_water(&mut self) -> usize {
        let high = self.reg_write_high_water.min(self.regs.len());
        self.reg_write_high_water = 0;
        high
    }

    #[inline]
    pub(super) fn clear_written_regs(&mut self) -> usize {
        let high = self.take_reg_write_high_water();
        let regs = &mut *self.regs;
        for slot in regs.iter_mut().take(high) {
            *slot = Val::Nil;
        }
        high
    }

    #[inline]
    pub(super) fn write_reg(&mut self, idx: usize, value: Val) {
        self.record_reg_write(idx);
        if idx < self.regs.len() {
            self.regs[idx] = value;
        } else {
            self.regs.resize(idx + 1, Val::Nil);
            self.regs[idx] = value;
        }
    }

    #[inline]
    pub(super) fn region_plan(&self) -> Option<&Arc<RegionPlan>> {
        self.region_plan.as_ref()
    }

    #[inline]
    pub(super) fn region_allocator(&self) -> &RegionAllocator {
        // SAFETY: lifetime由 VM 保证，FrameState 生命周期内 VM 不会被移动
        unsafe { &*self.region_allocator }
    }
}

impl<'func> Drop for FrameState<'func> {
    fn drop(&mut self) {
        unsafe {
            if let Some(frame) = self.frame_ptr.as_mut() {
                frame.pc = self.pc;
                frame.reg_base = self.reg_base;
                frame.reg_count = self.reg_count;
                frame.captures = self.captures.clone();
                frame.capture_specs = self.capture_specs.clone();
                frame.region_plan = self.region_plan.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::alloc::AllocationRegion;
    use crate::vm::analysis::{EscapeClass, EscapeSummary, FunctionAnalysis};
    use crate::vm::bytecode::Function;

    #[test]
    fn frame_state_propagates_region_plan_and_allocator() {
        let mut plan = RegionPlan::default();
        plan.values = vec![AllocationRegion::Heap, AllocationRegion::ThreadLocal];
        let analysis = FunctionAnalysis {
            ssa: None,
            escape: EscapeSummary {
                return_class: EscapeClass::Trivial,
                escaping_values: vec![0],
            },
            region_plan: Arc::new(plan.clone()),
        };

        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 2,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: Some(analysis),
        };

        let mut regs = Vec::<Val>::new();
        let plan_arc = Arc::new(plan);
        let mut frame = CallFrame::new(
            &function,
            0,
            function.n_regs as usize,
            None,
            None,
            Some(Arc::clone(&plan_arc)),
        );
        let allocator = RegionAllocator::new();
        let alloc_ptr: *const RegionAllocator = &allocator;
        let state = FrameState::new(&mut frame, &mut regs, alloc_ptr);

        let plan = state.region_plan.as_ref().expect("region plan available");
        assert_eq!(plan.region_for(0), AllocationRegion::Heap);
        assert_eq!(plan.region_for(1), AllocationRegion::ThreadLocal);
        let ptr = state.region_allocator() as *const RegionAllocator;
        assert_eq!(ptr, alloc_ptr);
    }
}
