use std::sync::Arc;

use crate::val::{ClosureCapture, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::RegionAllocator;
use crate::vm::{
    copy_call_arg_value_for_register_with_metrics, copy_value_for_register_with_metrics, take_register_value,
    write_register_copy_with_metrics, write_register_value_with_metrics,
};

use super::super::bytecode::{CaptureSpec, Function};
use super::Vm;
use super::caches::{FrameDispatchPlan, FunctionRuntimePlan, RuntimeDispatchSites};

#[derive(Debug, Clone, Copy)]
pub(super) struct CallFrameMeta {
    pub(super) resume_pc: usize,
    pub(super) ret_base: u16,
    pub(super) retc: u8,
    pub(super) caller_window: RegisterWindowRef,
}

impl CallFrameMeta {
    #[inline]
    pub(super) const fn inline_return(resume_pc: usize, ret_base: u16, retc: u8, frame_base: usize) -> Self {
        Self {
            resume_pc,
            ret_base,
            retc,
            caller_window: RegisterWindowRef::Base(frame_base),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RegisterWindowRef {
    Current,
    Base(usize),
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
            RegisterWindowRef::Base(base) => RegisterWindowRef::Base(base),
        };
        Self {
            window: relocated,
            ..self
        }
    }

    #[inline]
    pub(super) const fn stack_base(self) -> usize {
        match self.window {
            RegisterWindowRef::Current => self.base,
            RegisterWindowRef::Base(base) => base + self.base,
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

pub(super) struct FrameExecutionParts<'frame, 'func> {
    pub(super) frame: *mut (),
    pub(super) pc: usize,
    pub(super) dispatch_plan: FrameDispatchPlan<'func>,
    pub(super) base: usize,
    pub(super) regs: &'frame mut [Val],
    pub(super) captures: &'frame Option<Arc<ClosureCapture>>,
    pub(super) capture_specs: &'frame Option<Arc<Vec<CaptureSpec>>>,
    pub(super) region_plan: Option<&'frame RegionPlan>,
    pub(super) region_allocator: *const RegionAllocator,
    pub(super) collect_metrics: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StackWindow {
    pub(super) base: usize,
    pub(super) reg_count: usize,
}

impl StackWindow {
    #[inline]
    pub(super) const fn new(base: usize, reg_count: usize) -> Self {
        Self { base, reg_count }
    }

    #[inline]
    pub(super) const fn base(self) -> usize {
        self.base
    }

    #[inline]
    pub(super) fn from_runtime(base: usize, runtime: &FunctionRuntimePlan) -> Self {
        Self::new(base, runtime.reg_count)
    }
}

#[derive(Debug)]
pub(super) struct CallFrame<'func> {
    pub(super) func: &'func Function,
    pub(super) pc: usize,
    pub(super) reg_base: usize,
    pub(super) reg_count: usize,
    pub(super) dispatch_sites: RuntimeDispatchSites,
    pub(super) captures: Option<Arc<ClosureCapture>>,
    pub(super) capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    #[allow(dead_code)]
    pub(super) region_plan: Option<Arc<RegionPlan>>,
}

pub(super) struct FrameActivation {
    window: StackWindow,
    runtime: FunctionRuntimePlan,
    setup: FrameStateSetup,
}

pub(super) struct FrameActivationParts<'func> {
    pub(super) window: StackWindow,
    pub(super) call_frame: CallFrame<'func>,
    pub(super) setup: FrameStateSetup,
}

impl<'func> FrameActivationParts<'func> {
    #[inline]
    pub(super) const fn stack_base(&self) -> usize {
        self.window.base()
    }

    #[inline]
    pub(super) fn frame_state<'state>(
        &'state mut self,
        stack: &'state mut [Val],
        region_allocator: *const RegionAllocator,
    ) -> FrameState<'state, 'func>
    where
        'func: 'state,
    {
        let reg_base = self.stack_base();
        let reg_count = self.call_frame.reg_count;
        let regs = &mut stack[reg_base..reg_base + reg_count];
        FrameState::from_frame(&mut self.call_frame, regs, region_allocator, self.setup)
    }
}

impl FrameActivation {
    #[inline]
    pub(super) fn new(window: StackWindow, runtime: FunctionRuntimePlan, setup: FrameStateSetup) -> Self {
        Self { window, runtime, setup }
    }

    #[inline]
    pub(super) fn into_parts<'func>(
        self,
        func: &'func Function,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    ) -> FrameActivationParts<'func> {
        let window = self.window;
        let setup = self.setup;
        let call_frame = CallFrame::from_runtime(func, window, self.runtime, captures, capture_specs);
        FrameActivationParts {
            window,
            call_frame,
            setup,
        }
    }

    #[inline]
    pub(super) fn seed_positional_from_stack(&self, stack: &mut [Val], args: RegisterSpan, param_regs: &[u16]) {
        let src_base = args.stack_base();
        let count = args.len.min(param_regs.len());
        let collect_metrics = self.setup.collect_metrics;
        for (idx, param_reg) in param_regs.iter().take(count).enumerate() {
            let value = stack
                .get(src_base + idx)
                .map(|value| copy_call_arg_value_for_register_with_metrics(value, collect_metrics))
                .unwrap_or(Val::Nil);
            write_register_value_with_metrics(stack, self.window.base() + *param_reg as usize, value, collect_metrics);
        }
    }
}

impl<'func> CallFrame<'func> {
    pub(super) fn new(
        func: &'func Function,
        reg_base: usize,
        reg_count: usize,
        dispatch_sites: RuntimeDispatchSites,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        region_plan: Option<Arc<RegionPlan>>,
    ) -> Self {
        Self {
            func,
            pc: 0,
            reg_base,
            reg_count,
            dispatch_sites,
            captures,
            capture_specs,
            region_plan,
        }
    }

    #[inline]
    pub(super) fn from_runtime(
        func: &'func Function,
        window: StackWindow,
        runtime: FunctionRuntimePlan,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    ) -> Self {
        Self::new(
            func,
            window.base,
            window.reg_count,
            runtime.dispatch_sites,
            captures,
            capture_specs,
            runtime.region_plan,
        )
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

pub(super) struct FrameState<'frame, 'func> {
    pub(super) func: &'func Function,
    pub(super) pc: usize,
    pub(super) regs: &'frame mut [Val],
    pub(super) reg_base: usize,
    pub(super) reg_count: usize,
    pub(super) dispatch_sites: RuntimeDispatchSites,
    pub(super) frame_ptr: *mut CallFrame<'func>,
    pub(super) captures: Option<Arc<ClosureCapture>>,
    pub(super) capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    pub(super) region_plan: Option<Arc<RegionPlan>>,
    region_allocator: *const RegionAllocator,
    inline_return_meta: Option<CallFrameMeta>,
    sync_on_drop: bool,
    collect_metrics: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameStateSetup {
    sync_on_drop: bool,
    inline_return_meta: Option<CallFrameMeta>,
    collect_metrics: bool,
}

impl FrameStateSetup {
    #[inline]
    pub(super) const fn synchronized(collect_metrics: bool) -> Self {
        Self {
            sync_on_drop: true,
            inline_return_meta: None,
            collect_metrics,
        }
    }

    #[inline]
    pub(super) const fn inline_ephemeral(meta: CallFrameMeta, collect_metrics: bool) -> Self {
        Self {
            sync_on_drop: false,
            inline_return_meta: Some(meta),
            collect_metrics,
        }
    }
}

impl<'frame, 'func> FrameState<'frame, 'func> {
    pub(super) fn from_frame(
        frame: &'frame mut CallFrame<'func>,
        regs: &'frame mut [Val],
        region_allocator: *const RegionAllocator,
        setup: FrameStateSetup,
    ) -> Self {
        Self {
            func: frame.func,
            pc: frame.pc,
            regs,
            reg_base: frame.reg_base,
            reg_count: frame.reg_count,
            dispatch_sites: frame.dispatch_sites,
            frame_ptr: frame as *mut _,
            captures: frame.captures.take(),
            capture_specs: frame.capture_specs.take(),
            region_plan: frame.region_plan.take(),
            region_allocator,
            inline_return_meta: setup.inline_return_meta,
            sync_on_drop: setup.sync_on_drop,
            collect_metrics: setup.collect_metrics,
        }
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
    pub(super) fn take_inline_return_meta(&mut self) -> Option<CallFrameMeta> {
        self.inline_return_meta.take()
    }

    #[inline]
    pub(super) fn record_reg_write(&mut self, _idx: usize) {
        // Register windows are eagerly cleared before each execution. Avoid a
        // branch/write on every VM register assignment in hot loops.
    }

    #[inline]
    pub(super) fn write_reg(&mut self, idx: usize, value: Val) {
        self.record_reg_write(idx);
        write_register_value_with_metrics(&mut self.regs, idx, value, self.collect_metrics);
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn write_reg_copy(&mut self, idx: usize, value: &Val) {
        self.record_reg_write(idx);
        write_register_copy_with_metrics(&mut self.regs, idx, value, self.collect_metrics);
    }

    #[inline]
    pub(super) fn write_reg_call_arg_copy(&mut self, idx: usize, value: &Val) {
        self.record_reg_write(idx);
        write_register_value_with_metrics(
            &mut self.regs,
            idx,
            copy_call_arg_value_for_register_with_metrics(value, self.collect_metrics),
            self.collect_metrics,
        );
    }

    #[inline]
    pub(super) fn seed_positional_from_values(&mut self, param_regs: &[u16], args: &[Val]) {
        for (idx, value) in args.iter().take(param_regs.len()).enumerate() {
            self.write_reg_call_arg_copy(param_regs[idx] as usize, value);
        }
    }

    #[inline]
    pub(super) fn seed_named_call_arg_values(&mut self, named: &[(u16, Val)]) {
        for (reg_idx, value) in named {
            let slot = *reg_idx as usize;
            if slot < self.reg_count {
                self.write_reg_call_arg_copy(slot, value);
            }
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn borrow_reg(&self, idx: usize) -> Option<&Val> {
        self.regs.get(idx)
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn take_or_clone_reg(&mut self, idx: usize, may_take: bool) -> Val {
        debug_assert!(idx < self.regs.len(), "register read out of frame window");
        if may_take {
            take_register_value(&mut self.regs, idx)
        } else {
            copy_value_for_register_with_metrics(&self.regs[idx], self.collect_metrics)
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn move_reg(&mut self, src: usize, dst: usize) {
        debug_assert!(src < self.regs.len(), "register move source out of frame window");
        debug_assert!(dst < self.regs.len(), "register move destination out of frame window");
        if src == dst {
            return;
        }
        let value = take_register_value(&mut self.regs, src);
        self.write_reg(dst, value);
    }

    #[inline]
    pub(super) fn execution_parts(&mut self) -> FrameExecutionParts<'_, 'func> {
        let frame = self as *mut _ as *mut ();
        FrameExecutionParts {
            frame,
            pc: self.pc,
            dispatch_plan: self.dispatch_sites.frame_dispatch_plan(self.func),
            base: self.reg_base,
            regs: self.regs,
            captures: &self.captures,
            capture_specs: &self.capture_specs,
            region_plan: self.region_plan.as_deref(),
            region_allocator: self.region_allocator,
            collect_metrics: self.collect_metrics,
        }
    }
}

impl<'frame, 'func> Drop for FrameState<'frame, 'func> {
    fn drop(&mut self) {
        if !self.sync_on_drop {
            return;
        }
        unsafe {
            if let Some(frame) = self.frame_ptr.as_mut() {
                frame.pc = self.pc;
                frame.reg_base = self.reg_base;
                frame.reg_count = self.reg_count;
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
        let plan = RegionPlan {
            values: vec![AllocationRegion::Heap, AllocationRegion::ThreadLocal],
            ..Default::default()
        };
        let analysis = FunctionAnalysis {
            ssa: None,
            escape: EscapeSummary {
                return_class: EscapeClass::Trivial,
                escaping_values: vec![0],
            },
            region_plan: Arc::new(plan.clone()),
            ..FunctionAnalysis::default()
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
            RuntimeDispatchSites::new(function.code.len(), function.code32.as_ref().map(Vec::len)),
            None,
            None,
            Some(Arc::clone(&plan_arc)),
        );
        let allocator = RegionAllocator::new();
        let alloc_ptr: *const RegionAllocator = &allocator;
        let state = FrameState::from_frame(&mut frame, &mut regs, alloc_ptr, FrameStateSetup::synchronized(false));

        let plan = state.region_plan.as_ref().expect("region plan available");
        assert_eq!(plan.region_for(0), AllocationRegion::Heap);
        assert_eq!(plan.region_for(1), AllocationRegion::ThreadLocal);
        let ptr = state.region_allocator;
        assert_eq!(ptr, alloc_ptr);
    }

    #[test]
    fn call_frame_from_runtime_uses_plan_layout_and_region_plan() {
        let plan = RegionPlan {
            values: vec![AllocationRegion::ThreadLocal, AllocationRegion::Heap],
            ..Default::default()
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
            analysis: Some(FunctionAnalysis {
                region_plan: Arc::new(plan),
                ..FunctionAnalysis::default()
            }),
        };
        let mut vm = Vm::new();
        let runtime = vm.prepare_top_level_runtime(&function);
        let window = StackWindow::from_runtime(4, &runtime);

        let frame = CallFrame::from_runtime(&function, window, runtime, None, None);

        assert_eq!(frame.reg_base, 4);
        assert_eq!(frame.reg_count, 2);
        assert_eq!(
            frame.region_plan.expect("region plan").region_for(1),
            AllocationRegion::Heap
        );
    }

    #[test]
    fn frame_activation_carries_window_setup_and_runtime_into_parts() {
        let plan = RegionPlan {
            values: vec![AllocationRegion::Heap],
            ..Default::default()
        };
        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 1,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: Some(FunctionAnalysis {
                region_plan: Arc::new(plan),
                ..FunctionAnalysis::default()
            }),
        };
        let mut vm = Vm::new();
        let runtime = vm.prepare_top_level_runtime(&function);
        let window = StackWindow::from_runtime(8, &runtime);
        let setup = FrameStateSetup::synchronized(false);
        let activation = FrameActivation::new(window, runtime, setup);

        let parts = activation.into_parts(&function, None, None);

        assert_eq!(parts.window, window);
        assert_eq!(parts.stack_base(), 8);
        assert_eq!(parts.setup.sync_on_drop, setup.sync_on_drop);
        assert_eq!(parts.call_frame.reg_base, 8);
        assert_eq!(parts.call_frame.reg_count, 1);
        assert_eq!(
            parts.call_frame.region_plan.expect("region plan").region_for(0),
            AllocationRegion::Heap
        );
    }

    #[test]
    fn frame_activation_parts_preserve_window_frame_and_setup() {
        let plan = RegionPlan {
            values: vec![AllocationRegion::ThreadLocal],
            ..Default::default()
        };
        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 1,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: Some(FunctionAnalysis {
                region_plan: Arc::new(plan),
                ..FunctionAnalysis::default()
            }),
        };
        let mut vm = Vm::new();
        let runtime = vm.prepare_top_level_runtime(&function);
        let window = StackWindow::from_runtime(12, &runtime);
        let setup = FrameStateSetup::synchronized(false);
        let activation = FrameActivation::new(window, runtime, setup);

        let parts = activation.into_parts(&function, None, None);

        assert_eq!(parts.window, window);
        assert_eq!(parts.setup.sync_on_drop, setup.sync_on_drop);
        assert_eq!(parts.call_frame.reg_base, 12);
        assert_eq!(parts.call_frame.reg_count, 1);
        assert_eq!(
            parts.call_frame.region_plan.expect("region plan").region_for(0),
            AllocationRegion::ThreadLocal
        );
    }

    #[test]
    fn frame_activation_parts_create_frame_state_from_stack_window() {
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
            analysis: None,
        };
        let mut vm = Vm::new();
        let runtime = vm.prepare_top_level_runtime(&function);
        let window = StackWindow::from_runtime(2, &runtime);
        let activation = FrameActivation::new(window, runtime, FrameStateSetup::synchronized(false));
        let mut parts = activation.into_parts(&function, None, None);
        let mut stack = vec![Val::Int(99), Val::Int(88), Val::Nil, Val::Nil];
        let allocator = RegionAllocator::new();
        let alloc_ptr: *const RegionAllocator = &allocator;

        let mut state = parts.frame_state(&mut stack, alloc_ptr);
        state.write_reg(1, Val::Int(42));

        assert_eq!(state.reg_base(), 2);
        assert_eq!(state.reg_count(), 2);
        assert_eq!(state.borrow_reg(1), Some(&Val::Int(42)));
    }

    #[test]
    fn frame_activation_seeds_positional_args_from_stack_window() {
        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 3,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let mut vm = Vm::new();
        let runtime = vm.prepare_top_level_runtime(&function);
        let window = StackWindow::from_runtime(3, &runtime);
        let activation = FrameActivation::new(window, runtime, FrameStateSetup::synchronized(false));
        let mut stack = vec![Val::Int(11), Val::Int(22), Val::Nil, Val::Nil, Val::Nil, Val::Nil];

        activation.seed_positional_from_stack(&mut stack, RegisterSpan::new(0, 2, RegisterWindowRef::Base(0)), &[1, 0]);

        assert_eq!(stack[3], Val::Int(22));
        assert_eq!(stack[4], Val::Int(11));
        assert_eq!(stack[5], Val::Nil);
    }

    #[test]
    fn frame_state_register_protocol_borrows_writes_and_takes() {
        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 3,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let mut regs = vec![Val::Nil; 3];
        let mut frame = CallFrame::new(&function, 0, 3, RuntimeDispatchSites::new(0, None), None, None, None);
        let allocator = RegionAllocator::new();
        let alloc_ptr: *const RegionAllocator = &allocator;
        let mut state = FrameState::from_frame(&mut frame, &mut regs, alloc_ptr, FrameStateSetup::synchronized(false));

        state.write_reg(1, Val::Int(42));
        assert_eq!(state.borrow_reg(1), Some(&Val::Int(42)));
        assert_eq!(state.take_or_clone_reg(1, false), Val::Int(42));
        assert_eq!(state.borrow_reg(1), Some(&Val::Int(42)));
        assert_eq!(state.take_or_clone_reg(1, true), Val::Int(42));
        assert_eq!(state.borrow_reg(1), Some(&Val::Nil));

        state.write_reg(0, Val::Int(7));
        state.move_reg(0, 2);
        assert_eq!(state.borrow_reg(0), Some(&Val::Nil));
        assert_eq!(state.borrow_reg(2), Some(&Val::Int(7)));
    }

    #[test]
    fn frame_state_seeds_positional_and_named_args_with_bounds_check() {
        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 3,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let mut regs = vec![Val::Nil; 3];
        let mut frame = CallFrame::new(&function, 0, 3, RuntimeDispatchSites::new(0, None), None, None, None);
        let allocator = RegionAllocator::new();
        let alloc_ptr: *const RegionAllocator = &allocator;
        let mut state = FrameState::from_frame(&mut frame, &mut regs, alloc_ptr, FrameStateSetup::synchronized(false));

        state.seed_positional_from_values(&[2, 0], &[Val::Int(5), Val::Int(9), Val::Int(99)]);
        state.seed_named_call_arg_values(&[(1, Val::Int(7)), (9, Val::Int(100))]);

        assert_eq!(state.borrow_reg(0), Some(&Val::Int(9)));
        assert_eq!(state.borrow_reg(1), Some(&Val::Int(7)));
        assert_eq!(state.borrow_reg(2), Some(&Val::Int(5)));
    }

    #[test]
    fn frame_state_setup_inline_ephemeral_sets_return_meta_without_syncing_drop() {
        let function = Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 1,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let mut regs = vec![Val::Nil; 1];
        let mut frame = CallFrame::new(&function, 0, 1, RuntimeDispatchSites::new(0, None), None, None, None);
        let allocator = RegionAllocator::new();
        let alloc_ptr: *const RegionAllocator = &allocator;
        let meta = CallFrameMeta::inline_return(9, 2, 1, 4);

        {
            let mut state = FrameState::from_frame(
                &mut frame,
                &mut regs,
                alloc_ptr,
                FrameStateSetup::inline_ephemeral(meta, false),
            );
            assert_eq!(state.take_inline_return_meta().map(|meta| meta.resume_pc), Some(9));
            state.set_pc(7);
        }

        assert_eq!(frame.pc, 0);
    }
}
