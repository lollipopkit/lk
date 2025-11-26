use anyhow::{Result, anyhow};

use crate::val::Val;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::ForRangeState;
use crate::vm::vm::frame::{CallFrameMeta, FrameState, RegisterWindowRef};

pub(super) fn frame_return_common(frame_raw: *mut FrameState<'_>, pc: usize, value: Result<Val>) -> Result<Val> {
    unsafe {
        (*frame_raw).pc = pc;
        if let Some(call_frame) = (*frame_raw).frame_ptr.as_mut() {
            call_frame.pc = pc;
        }
    }
    value
}

pub(super) fn handle_return_common(
    frame_raw: *mut FrameState<'_>,
    regs: &mut Vec<Val>,
    pc: usize,
    base_idx: usize,
    retc: usize,
    ret_val: Val,
    self_ptr: *mut Vm,
) -> Result<Val> {
    let vm = unsafe { &mut *self_ptr };
    let inline_meta = unsafe { (&mut *frame_raw).take_inline_return_meta() };
    if let Some(meta) = inline_meta {
        let expected = meta.retc as usize;
        debug_assert!(
            expected == retc || expected == 0 || retc == 0,
            "inline meta expected {} but callee returned {} values",
            expected,
            retc
        );
        vm.pending_resume_pc = Some(meta.resume_pc);
    } else if let Some(meta) = vm.frames.last().copied() {
        let expected = meta.retc as usize;
        debug_assert!(
            expected == retc || retc == 0,
            "callee retc {} differs from caller expectation {}",
            retc,
            expected
        );
        move_return_values(frame_raw, vm, regs, base_idx, retc, expected, meta);
        vm.pending_resume_pc = Some(meta.resume_pc);
    }
    frame_return_common(frame_raw, pc, Ok(ret_val))
}

#[inline(always)]
pub(super) fn assign_reg(frame_raw: *mut FrameState<'_>, regs: &mut Vec<Val>, idx: usize, value: Val) {
    unsafe {
        (*frame_raw).record_reg_write(idx);
    }
    regs[idx] = value;
}

#[inline(always)]
pub(super) fn assign_reg_slice(frame_raw: *mut FrameState<'_>, regs: &mut [Val], idx: usize, value: Val) {
    unsafe {
        (*frame_raw).record_reg_write(idx);
    }
    regs[idx] = value;
}

#[inline(always)]
pub(super) fn mark_reg_written(frame_raw: *mut FrameState<'_>, idx: usize) {
    unsafe {
        (*frame_raw).record_reg_write(idx);
    }
}

#[inline]
pub(super) fn fetch_for_range_state<'a>(
    slots: &'a mut Vec<Option<ForRangeState>>,
    pc: usize,
) -> Result<&'a mut ForRangeState> {
    slots
        .get_mut(pc)
        .and_then(|slot| slot.as_mut())
        .ok_or_else(|| anyhow!("For-range state missing at pc {}", pc))
}

fn move_return_values(
    frame_raw: *mut FrameState<'_>,
    vm: &mut Vm,
    callee_regs: &mut Vec<Val>,
    base_idx: usize,
    retc: usize,
    expected: usize,
    meta: CallFrameMeta,
) {
    match meta.caller_window {
        RegisterWindowRef::Current => {
            let dest_base = meta.ret_base as usize;
            let limit = expected.min(retc);
            for i in 0..limit {
                let src_idx = base_idx + i;
                let dst_idx = dest_base + i;
                if src_idx == dst_idx {
                    continue;
                }
                mark_reg_written(frame_raw, src_idx);
                mark_reg_written(frame_raw, dst_idx);
                let value = std::mem::replace(&mut callee_regs[src_idx], Val::Nil);
                callee_regs[dst_idx] = value;
            }
            if expected > retc {
                for i in retc..expected {
                    let dst_idx = dest_base + i;
                    mark_reg_written(frame_raw, dst_idx);
                    callee_regs[dst_idx] = Val::Nil;
                }
            }
        }
        RegisterWindowRef::StackIndex(idx) => {
            let dest_base = meta.ret_base as usize;
            let parent_regs = if let Some(slot) = vm.reg_stack.get_mut(idx) {
                slot
            } else {
                vm.reg_stack.last_mut().expect("missing caller register window")
            };
            let limit = expected.min(retc);
            for i in 0..limit {
                let src_idx = base_idx + i;
                mark_reg_written(frame_raw, src_idx);
                let value = std::mem::replace(&mut callee_regs[src_idx], Val::Nil);
                parent_regs[dest_base + i] = value;
            }
            if expected > retc {
                for i in retc..expected {
                    parent_regs[dest_base + i] = Val::Nil;
                }
            }
        }
    }
}
