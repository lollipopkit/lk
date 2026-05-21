use super::super::call_common::{
    call_hot_path_to_option, run_closure_exact_call_entry_common, run_exact_call_entry_common, run_named_call_common,
    run_native_fast_or_positional_call_entry_common, run_positional_call_entry_common,
};
use super::super::raw_boundary::region_allocator;
use super::*;
use crate::vm::vm::frame::FrameState;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let allocator = region_allocator(region_allocator_ptr);
    Ok(call_hot_path_to_option(
        run_positional_call_entry_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc_ref,
            *pc_ref + 1,
            frame_base,
            self_ptr,
            *rf,
            *base,
            *argc,
            *retc,
            allocator,
            collect_metrics,
        )?,
        "positional call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_native_fast_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let allocator = region_allocator(region_allocator_ptr);
    Ok(call_hot_path_to_option(
        run_native_fast_or_positional_call_entry_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc_ref,
            *pc_ref + 1,
            frame_base,
            self_ptr,
            *rf,
            *base,
            *argc,
            *retc,
            allocator,
            collect_metrics,
        )?,
        "native fast or positional call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_closure_exact_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    _region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    Ok(call_hot_path_to_option(
        run_closure_exact_call_entry_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc_ref,
            *pc_ref + 1,
            frame_base,
            self_ptr,
            *rf,
            *base,
            *argc,
            *retc,
            collect_metrics,
        )?,
        "closure exact call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_exact_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    _region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    Ok(call_hot_path_to_option(
        run_exact_call_entry_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc_ref,
            *pc_ref + 1,
            frame_base,
            self_ptr,
            *rf,
            *base,
            *argc,
            *retc,
            collect_metrics,
        )?,
        "exact call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_named_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base_pos: &u16,
    posc: &u8,
    base_named: &u16,
    namedc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let allocator = region_allocator(region_allocator_ptr);
    Ok(call_hot_path_to_option(
        run_named_call_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc_ref,
            *pc_ref + 1,
            frame_base,
            self_ptr,
            *rf,
            *base_pos,
            *posc,
            *base_named,
            *namedc,
            *retc,
            allocator,
            collect_metrics,
        )?,
        "named call common cannot miss",
    ))
}
