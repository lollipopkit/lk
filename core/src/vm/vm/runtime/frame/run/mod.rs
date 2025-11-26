use anyhow::Result;

use crate::val::Val;
use crate::vm::alloc::RegionAllocator;
use crate::vm::bc32;
use crate::vm::bytecode::Function;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::VmCaches;
use crate::vm::vm::frame::FrameState;

mod helpers;
mod ic;
mod invoke;
mod math;
mod opcode;
mod packed;
mod plan;

use helpers::handle_return_common;
use opcode::run_opcode_code;
use packed::run_packed_code;

pub(crate) fn run_frame(
    frame: &mut FrameState<'_>,
    ctx: &mut VmContext,
    mut caches: VmCaches<'_>,
    self_ptr: *mut Vm,
) -> Result<Val> {
    let f = frame.func();
    let mut pc: usize = frame.pc();
    let frame_raw: *mut FrameState<'_> = frame;
    let frame_base = frame.reg_base();
    let frame_captures = frame.capture_arc();
    let frame_capture_specs = frame.capture_specs_arc();
    let region_plan = frame.region_plan().cloned();
    let region_allocator_ptr = frame.region_allocator() as *const RegionAllocator;
    let regs = frame.regs();

    if let Some(code32) = f.code32.as_ref()
        && supports_bc32_fast_path(f)
    {
        if let Some(value) = run_packed_code(
            frame_raw,
            regs,
            ctx,
            &mut caches,
            f,
            &mut pc,
            frame_base,
            code32,
            f.bc32_decoded.as_deref(),
            &frame_captures,
            &frame_capture_specs,
            region_plan.as_deref(),
            region_allocator_ptr,
            self_ptr,
        )? {
            return Ok(value);
        }
    }

    if let Some(value) = run_opcode_code(
        frame_raw,
        regs,
        ctx,
        &mut caches,
        f,
        &mut pc,
        frame_base,
        &frame_captures,
        &frame_capture_specs,
        region_plan.as_deref(),
        region_allocator_ptr,
        self_ptr,
    )? {
        return Ok(value);
    }

    handle_return_common(frame_raw, regs, pc, frame_base, 0, Val::Nil, self_ptr)
}

fn supports_bc32_fast_path(f: &Function) -> bool {
    if !f.named_param_layout.is_empty() {
        return false;
    }
    if let Some(code32) = f.code32.as_ref() {
        let has_reg_ext = code32.iter().any(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
        if has_reg_ext { f.bc32_decoded.is_some() } else { true }
    } else {
        false
    }
}
