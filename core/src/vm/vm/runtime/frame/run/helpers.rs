use anyhow::{Result, anyhow};
use std::sync::Arc;

use crate::util::fast_map::FastHashMap;
use crate::val::Val;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::ForRangeState;
use crate::vm::vm::frame::{CallFrameMeta, FrameState, RegisterWindowRef};

/// Mark the end of the current frame and return to caller.
/// Records the PC position and updates the frame pointer.
#[inline]
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
    regs: &mut [Val],
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
#[allow(clippy::ptr_arg)]
pub(super) fn assign_reg(_frame_raw: *mut FrameState<'_>, regs: &mut Vec<Val>, idx: usize, value: Val) {
    regs[idx] = value;
}

#[inline(always)]
pub(super) fn assign_reg_slice(_frame_raw: *mut FrameState<'_>, regs: &mut [Val], idx: usize, value: Val) {
    regs[idx] = value;
}

#[inline(always)]
pub(super) fn mark_reg_written(_frame_raw: *mut FrameState<'_>, _idx: usize) {}

const DYNAMIC_MAP_FIRST_MUTATION_RESERVE: usize = 64;
const DYNAMIC_LIST_FIRST_MUTATION_RESERVE: usize = 128;

#[inline]
pub(super) fn push_list_entry(arc: &mut Arc<Vec<Val>>, value: Val) {
    let list = Arc::make_mut(arc);
    if list.is_empty() && list.capacity() < DYNAMIC_LIST_FIRST_MUTATION_RESERVE {
        list.reserve(DYNAMIC_LIST_FIRST_MUTATION_RESERVE);
    }
    list.push(value);
}

#[inline]
pub(super) fn insert_map_entry(arc: &mut Arc<FastHashMap<Arc<str>, Val>>, key: Arc<str>, value: Val) {
    let map = Arc::make_mut(arc);
    if map.is_empty() && map.capacity() < DYNAMIC_MAP_FIRST_MUTATION_RESERVE {
        map.reserve(DYNAMIC_MAP_FIRST_MUTATION_RESERVE);
    }
    map.insert(key, value);
}

#[inline]
pub(super) fn fetch_for_range_state(slots: &mut [Option<ForRangeState>], pc: usize) -> Result<&mut ForRangeState> {
    slots
        .get_mut(pc)
        .and_then(|slot| slot.as_mut())
        .ok_or_else(|| anyhow!("For-range state missing at pc {}", pc))
}

fn move_return_values(
    frame_raw: *mut FrameState<'_>,
    vm: &mut Vm,
    callee_regs: &mut [Val],
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
