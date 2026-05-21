use std::sync::Arc;

use anyhow::Result;

use crate::val::{ClosureCapture, Val};
use crate::vm::alloc::RegionAllocator;
use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{ClosureFastCache, FunctionRuntimePlan};
use crate::vm::vm::frame::{CallFrameMeta, FrameInfo, FrameState, RegisterSpan};

#[inline]
pub(super) fn region_allocator<'a>(ptr: *const RegionAllocator) -> &'a RegionAllocator {
    debug_assert!(!ptr.is_null(), "region allocator pointer must be non-null");
    // SAFETY: FrameState::execution_parts produces this pointer from the owning VM
    // region allocator for the duration of the active frame execution.
    unsafe { &*ptr }
}

#[inline]
pub(super) fn function_from_ptr<'a>(ptr: *const Function) -> &'a Function {
    debug_assert!(!ptr.is_null(), "cached function pointer must be non-null");
    // SAFETY: CallIc stores Function pointers obtained from Arc-backed closures.
    // The matching closure Arc is checked at the call site before this helper is used.
    unsafe { &*ptr }
}

#[inline]
pub(super) fn pop_vm_frame(vm: *mut Vm) -> Option<CallFrameMeta> {
    debug_assert!(!vm.is_null(), "vm pointer must be non-null");
    // SAFETY: run_frame receives a unique &mut Vm converted to self_ptr for nested
    // frame helpers. Call sites use this only while executing on that VM thread.
    unsafe { &mut *vm }.frames.pop()
}

#[inline]
pub(super) fn push_vm_frame(vm: *mut Vm, meta: CallFrameMeta) {
    debug_assert!(!vm.is_null(), "vm pointer must be non-null");
    // SAFETY: see pop_vm_frame; pushing restores metadata previously popped from
    // the same VM frame stack during default-argument evaluation.
    unsafe { &mut *vm }.frames.push(meta);
}

#[inline]
pub(super) fn with_vm_mut<R>(vm: *mut Vm, f: impl FnOnce(&mut Vm) -> R) -> R {
    debug_assert!(!vm.is_null(), "vm pointer must be non-null");
    // SAFETY: run_frame creates self_ptr from the active unique &mut Vm. Helpers
    // call this synchronously during that frame execution.
    f(unsafe { &mut *vm })
}

#[inline]
pub(super) fn set_frame_pc(frame: *mut FrameState<'_, '_>, pc: usize) {
    debug_assert!(!frame.is_null(), "frame pointer must be non-null");
    // SAFETY: frame_raw is derived from the active &mut FrameState in run_frame
    // and remains valid until that frame returns.
    unsafe {
        (*frame).pc = pc;
        if let Some(call_frame) = (*frame).frame_ptr.as_mut() {
            call_frame.pc = pc;
        }
    }
}

#[inline]
pub(super) fn take_inline_return_meta(frame: *mut FrameState<'_, '_>) -> Option<CallFrameMeta> {
    debug_assert!(!frame.is_null(), "frame pointer must be non-null");
    // SAFETY: see set_frame_pc; this consumes metadata from the same active frame.
    unsafe { (&mut *frame).take_inline_return_meta() }
}

#[allow(clippy::too_many_arguments)]
#[inline]
pub(super) fn exec_positional_fast_span_unchecked(
    vm: *mut Vm,
    fun: &Function,
    runtime: Option<&FunctionRuntimePlan>,
    args: RegisterSpan,
    ctx: &mut VmContext,
    frame_info: Option<&FrameInfo>,
    captures: Option<Arc<ClosureCapture>>,
    capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    cache: &mut ClosureFastCache,
    return_meta: CallFrameMeta,
    collect_metrics: bool,
) -> Result<Val> {
    with_vm_mut(vm, |vm| {
        vm.exec_function_positional_fast_span_unchecked(
            fun,
            runtime,
            args,
            ctx,
            frame_info,
            captures,
            capture_specs,
            cache,
            return_meta,
            collect_metrics,
        )
    })
}
