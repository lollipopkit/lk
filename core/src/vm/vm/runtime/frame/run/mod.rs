use anyhow::Result;
use std::sync::Arc;

use crate::val::{ClosureCapture, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::RegionAllocator;
use crate::vm::bytecode::CaptureSpec;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{FrameDispatchPlan, RuntimeDispatchMode, VmCaches};
use crate::vm::vm::frame::{FrameExecutionParts, FrameState};
use crate::vm::vm_runtime_metrics_enabled;

mod call_common;
mod helpers;
mod ic;
mod invoke;
mod math;
mod method_ops;
mod opcode;
mod packed;
mod plan;
mod raw_boundary;

use helpers::handle_return_common;
use opcode::run_opcode_code;
use packed::run_packed_code;

pub(super) struct FrameRuntimeView<'a, 'func> {
    pub(super) frame_raw: *mut FrameState<'a, 'func>,
    pub(super) pc: usize,
    pub(super) regs: &'a mut [Val],
    pub(super) ctx: &'a mut VmContext,
    pub(super) caches: VmCaches<'a>,
    pub(super) dispatch_plan: FrameDispatchPlan<'func>,
    pub(super) collect_metrics: bool,
    pub(super) base: usize,
    pub(super) captures: &'a Option<Arc<ClosureCapture>>,
    pub(super) capture_specs: &'a Option<Arc<Vec<CaptureSpec>>>,
    pub(super) region_plan: Option<&'a RegionPlan>,
    pub(super) region_allocator: *const RegionAllocator,
    pub(super) self_ptr: *mut Vm,
}

impl FrameRuntimeView<'_, '_> {
    #[inline(always)]
    fn prepare_packed_dispatch(&mut self) -> bool {
        match self.dispatch_plan.mode() {
            RuntimeDispatchMode::Packed(_) => {
                self.dispatch_plan.prepare_packed_sites(&mut self.caches);
                true
            }
            RuntimeDispatchMode::Opcode => false,
        }
    }

    #[inline(always)]
    fn prepare_opcode_dispatch(&mut self) {
        self.dispatch_plan.prepare_opcode_sites(&mut self.caches);
    }

    #[inline(always)]
    fn finish_fallthrough_return(self) -> Result<Val> {
        handle_return_common(
            self.frame_raw,
            self.regs,
            self.pc,
            self.base,
            0,
            Val::Nil,
            self.self_ptr,
        )
    }
}

pub(crate) fn run_frame(
    frame: &mut FrameState<'_, '_>,
    ctx: &mut VmContext,
    caches: VmCaches<'_>,
    self_ptr: *mut Vm,
) -> Result<Val> {
    let FrameExecutionParts {
        frame: frame_raw,
        pc,
        dispatch_plan,
        base: frame_base,
        regs,
        captures: frame_captures,
        capture_specs: frame_capture_specs,
        region_plan,
        region_allocator: region_allocator_ptr,
    } = frame.execution_parts();
    let frame_raw = frame_raw.cast::<FrameState<'_, '_>>();
    let mut runtime = FrameRuntimeView {
        frame_raw,
        pc,
        regs,
        ctx,
        caches,
        dispatch_plan,
        collect_metrics: vm_runtime_metrics_enabled(),
        base: frame_base,
        captures: frame_captures,
        capture_specs: frame_capture_specs,
        region_plan,
        region_allocator: region_allocator_ptr,
        self_ptr,
    };

    if runtime.prepare_packed_dispatch()
        && let Some(value) = run_packed_code(&mut runtime)?
    {
        return Ok(value);
    }

    runtime.prepare_opcode_dispatch();
    if let Some(value) = run_opcode_code(&mut runtime)? {
        return Ok(value);
    }

    runtime.finish_fallthrough_return()
}
