use anyhow::{Result, anyhow};
use std::sync::Arc;

use crate::util::fast_map::FastHashMap;
use crate::val::Val;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::ForRangeState;
use crate::vm::vm::frame::{CallFrameMeta, FrameState, RegisterWindowRef};
use crate::vm::{record_register_write, record_return_value_move};
use arcstr::ArcStr;

use super::raw_boundary::{set_frame_pc, take_inline_return_meta, with_vm_mut};

/// Mark the end of the current frame and return to caller.
/// Records the PC position and updates the frame pointer.
#[inline]
pub(super) fn frame_return_common(frame_raw: *mut FrameState<'_>, pc: usize, value: Result<Val>) -> Result<Val> {
    set_frame_pc(frame_raw, pc);
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
    let inline_meta = take_inline_return_meta(frame_raw);
    with_vm_mut(self_ptr, |vm| {
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
    });
    frame_return_common(frame_raw, pc, Ok(ret_val))
}

#[inline(always)]
pub(super) fn assign_reg(_frame_raw: *mut FrameState<'_>, regs: &mut [Val], idx: usize, value: Val) {
    record_register_write();
    regs[idx] = value;
}

#[inline(always)]
pub(super) fn assign_reg_slice(frame_raw: *mut FrameState<'_>, regs: &mut [Val], idx: usize, value: Val) {
    assign_reg(frame_raw, regs, idx, value);
}

#[inline(always)]
pub(super) fn mark_reg_written(_frame_raw: *mut FrameState<'_>, _idx: usize) {}

#[inline(always)]
pub(super) fn move_reg_value(regs: &mut [Val], src_idx: usize) -> Val {
    let value = std::mem::replace(&mut regs[src_idx], Val::Nil);
    record_return_value_move();
    value
}

#[inline(always)]
pub(super) fn move_reg_to_reg(frame_raw: *mut FrameState<'_>, regs: &mut [Val], src_idx: usize, dst_idx: usize) {
    if src_idx == dst_idx {
        return;
    }
    mark_reg_written(frame_raw, src_idx);
    mark_reg_written(frame_raw, dst_idx);
    let value = move_reg_value(regs, src_idx);
    regs[dst_idx] = value;
}

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
pub(super) fn insert_map_entry(arc: &mut Arc<FastHashMap<ArcStr, Val>>, key: ArcStr, value: Val) {
    let map = Arc::make_mut(arc);
    if map.is_empty() && map.capacity() < DYNAMIC_MAP_FIRST_MUTATION_RESERVE {
        map.reserve(DYNAMIC_MAP_FIRST_MUTATION_RESERVE);
    }
    Val::map_insert_arcstr(map, key, value);
}

#[inline]
pub(super) fn fetch_for_range_state(slots: &mut [Option<ForRangeState>], pc: usize) -> Result<&mut ForRangeState> {
    slots
        .get_mut(pc)
        .and_then(|slot| slot.as_mut())
        .ok_or_else(|| anyhow!("For-range state missing at pc {}", pc))
}

#[inline(always)]
pub(super) fn advance_for_range_tail(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    slots: &mut [Option<ForRangeState>],
    guard_pc: usize,
    body_pc: usize,
    exit_pc: usize,
    idx: u16,
    write_idx: bool,
) -> Result<usize> {
    let idx_reg = idx as usize;
    let next_pc = {
        let state = fetch_for_range_state(slots, guard_pc)?;
        if state.should_continue() {
            if write_idx {
                assign_reg(frame_raw, regs, idx_reg, Val::Int(state.current));
            }
            state.current += state.step;
            body_pc
        } else {
            if write_idx {
                assign_reg(frame_raw, regs, idx_reg, Val::Int(state.current));
            }
            exit_pc
        }
    };
    if next_pc == exit_pc {
        slots[guard_pc] = None;
    }
    Ok(next_pc)
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
                move_reg_to_reg(frame_raw, callee_regs, src_idx, dst_idx);
            }
            if expected > retc {
                for i in retc..expected {
                    let dst_idx = dest_base + i;
                    mark_reg_written(frame_raw, dst_idx);
                    callee_regs[dst_idx] = Val::Nil;
                }
            }
        }
        RegisterWindowRef::Base(base) => {
            let dest_base = base + meta.ret_base as usize;
            let limit = expected.min(retc);
            for i in 0..limit {
                let src_idx = base_idx + i;
                mark_reg_written(frame_raw, src_idx);
                vm.stack[dest_base + i] = move_reg_value(callee_regs, src_idx);
            }
            if expected > retc {
                for i in retc..expected {
                    vm.stack[dest_base + i] = Val::Nil;
                }
            }
        }
    }
}
