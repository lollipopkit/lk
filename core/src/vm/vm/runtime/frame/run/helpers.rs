use anyhow::{Result, anyhow};
use std::sync::Arc;

use crate::op::BinOp;
use crate::util::fast_map::FastHashMap;
use crate::val::Val;
use crate::vm::bytecode::{Function, PatternBinding};
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{ForRangeState, GlobalEntry};
use crate::vm::vm::frame::{CallFrameMeta, FrameState, RegisterWindowRef};
use crate::vm::{
    copy_local_load_register_value_with_metrics, copy_local_store_register_value_with_metrics,
    copy_register_value_with_metrics, move_register_value, take_register_value, write_register_const_copy_with_metrics,
    write_register_copy_with_metrics, write_register_value_with_metrics,
};
use arcstr::ArcStr;

use super::raw_boundary::{set_frame_pc, take_inline_return_meta, with_vm_mut};

#[inline]
pub(super) fn load_global_for_register(
    ctx: &mut VmContext,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    name_val: &Val,
    collect_metrics: bool,
) -> Val {
    let Some(name) = name_val.as_str() else {
        let fallback_name = format!("{name_val}");
        return ctx
            .get(fallback_name.as_str())
            .map(|value| crate::vm::copy_value_for_register_with_metrics(value, collect_metrics))
            .unwrap_or(Val::Nil);
    };

    let key_ptr = name.as_ptr() as usize;
    let current_generation = ctx.generation();
    let local_shadowed = ctx.is_local_name(name);
    if !local_shadowed
        && let Some(GlobalEntry(ptr, value, generation)) = &global_ic[pc]
        && *ptr == key_ptr
        && *generation == current_generation
    {
        return crate::vm::copy_value_for_register_with_metrics(value, collect_metrics);
    }

    let mut out = Val::Nil;
    if let Some(value) = ctx.get(name) {
        out = crate::vm::copy_value_for_register_with_metrics(value, collect_metrics);
    }
    if matches!(out, Val::Nil)
        && let Some(value) = ctx.resolver().get_builtin(name)
    {
        out = crate::vm::copy_value_for_register_with_metrics(value, collect_metrics);
    }
    if !local_shadowed {
        let cached = crate::vm::copy_value_for_register_with_metrics(&out, collect_metrics);
        global_ic[pc] = Some(GlobalEntry(key_ptr, cached, current_generation));
    }
    out
}

#[inline]
pub(super) fn copy_capture_spec_value(
    ctx: &VmContext,
    regs: &[Val],
    consts: &[Val],
    spec: &crate::vm::bytecode::CaptureSpec,
    collect_metrics: bool,
) -> Val {
    match spec {
        crate::vm::bytecode::CaptureSpec::Register { src, .. } => regs
            .get(*src as usize)
            .map(|value| crate::vm::copy_value_for_register_with_metrics(value, collect_metrics))
            .unwrap_or(Val::Nil),
        crate::vm::bytecode::CaptureSpec::Const { kidx, .. } => consts
            .get(*kidx as usize)
            .map(|value| crate::vm::copy_const_value_for_register_with_metrics(value, collect_metrics))
            .unwrap_or(Val::Nil),
        crate::vm::bytecode::CaptureSpec::Global { name } => ctx
            .get(name.as_str())
            .map(|value| crate::vm::copy_value_for_register_with_metrics(value, collect_metrics))
            .unwrap_or(Val::Nil),
    }
}

#[inline]
pub(super) fn assign_pattern_bindings_with_metrics(
    regs: &mut [Val],
    bindings: &[PatternBinding],
    bound: &[(String, Val)],
    collect_metrics: bool,
) {
    for binding in bindings {
        let value = bound
            .iter()
            .find(|(name, _)| name == &binding.name)
            .map(|(_, value)| crate::vm::copy_value_for_register_with_metrics(value, collect_metrics))
            .unwrap_or(Val::Nil);
        write_register_value_with_metrics(regs, binding.reg as usize, value, collect_metrics);
    }
}

#[inline]
pub(super) fn assign_pattern_bindings_for_context_with_metrics(
    regs: &mut [Val],
    bindings: &[PatternBinding],
    bound: &[(String, Val)],
    assigned: &mut Vec<(String, Val)>,
    collect_metrics: bool,
) {
    for binding in bindings {
        if let Some((_, value)) = bound.iter().find(|(name, _)| name == &binding.name) {
            let binding_value = crate::vm::copy_value_for_register_with_metrics(value, collect_metrics);
            let context_value = crate::vm::copy_value_for_register_with_metrics(&binding_value, collect_metrics);
            write_register_value_with_metrics(regs, binding.reg as usize, binding_value, collect_metrics);
            assigned.push((binding.name.clone(), context_value));
        } else {
            write_register_value_with_metrics(regs, binding.reg as usize, Val::Nil, collect_metrics);
        }
    }
}

#[inline]
pub(super) fn clear_pattern_bindings_with_metrics(
    regs: &mut [Val],
    bindings: &[PatternBinding],
    collect_metrics: bool,
) {
    for binding in bindings {
        write_register_value_with_metrics(regs, binding.reg as usize, Val::Nil, collect_metrics);
    }
}

#[inline]
pub(super) fn fold_add_values_with_metrics<'a>(
    initial: &Val,
    values: impl Iterator<Item = &'a Val> + Clone,
    collect_metrics: bool,
) -> Result<Val> {
    if let Val::Int(initial_total) = initial {
        let mut total = *initial_total;
        let mut all_int = true;
        for item in values.clone() {
            if let Val::Int(value) = item {
                total = total.wrapping_add(*value);
            } else {
                all_int = false;
                break;
            }
        }
        if all_int {
            return Ok(Val::Int(total));
        }
    }

    let mut out = crate::vm::copy_value_for_register_with_metrics(initial, collect_metrics);
    for item in values {
        out = BinOp::Add.eval_vals_with_metrics(&out, item, collect_metrics)?;
    }
    Ok(out)
}

/// Mark the end of the current frame and return to caller.
/// Records the PC position and updates the frame pointer.
#[inline]
pub(super) fn frame_return_common(frame_raw: *mut FrameState<'_, '_>, pc: usize, value: Result<Val>) -> Result<Val> {
    set_frame_pc(frame_raw, pc);
    value
}

pub(super) fn handle_return_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    pc: usize,
    base_idx: usize,
    retc: usize,
    ret_val: Val,
    self_ptr: *mut Vm,
    collect_metrics: bool,
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
            move_return_values(vm, regs, base_idx, retc, expected, meta, collect_metrics);
            vm.pending_resume_pc = Some(meta.resume_pc);
        }
    });
    frame_return_common(frame_raw, pc, Ok(ret_val))
}

/// Known-enabled `assign_reg`: skips the per-write `runtime_metrics_enabled()`
/// atomic read when `collect_metrics` is false.
#[inline(always)]
pub(super) fn assign_reg_with_metrics(regs: &mut [Val], idx: usize, value: Val, collect_metrics: bool) {
    write_register_value_with_metrics(regs, idx, value, collect_metrics);
}

#[inline(always)]
pub(super) fn assign_reg_copy_with_metrics(regs: &mut [Val], idx: usize, value: &Val, collect_metrics: bool) {
    write_register_copy_with_metrics(regs, idx, value, collect_metrics);
}

#[inline(always)]
pub(super) fn assign_reg_const_copy_with_metrics(regs: &mut [Val], idx: usize, value: &Val, collect_metrics: bool) {
    write_register_const_copy_with_metrics(regs, idx, value, collect_metrics);
}

#[inline(always)]
#[cfg(test)]
pub(super) fn assign_reg_from_reg(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
) {
    let _ = frame_raw;
    assign_reg_from_reg_with_metrics(regs, dst_idx, src_idx, crate::vm::vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(super) fn assign_reg_from_reg_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    collect_metrics: bool,
) {
    copy_register_value_with_metrics(regs, dst_idx, src_idx, collect_metrics);
}

#[inline(always)]
#[cfg(test)]
#[allow(dead_code)]
pub(super) fn assign_reg_from_local_load(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
) {
    let _ = frame_raw;
    assign_reg_from_local_load_with_metrics(regs, dst_idx, src_idx, crate::vm::vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(super) fn assign_reg_from_local_load_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    collect_metrics: bool,
) {
    copy_local_load_register_value_with_metrics(regs, dst_idx, src_idx, collect_metrics);
}

#[inline(always)]
pub(super) fn assign_reg_from_local_load_or_take_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    may_take: bool,
    collect_metrics: bool,
) {
    if may_take {
        assign_reg_from_reg_or_take_with_metrics(regs, dst_idx, src_idx, true, collect_metrics);
    } else {
        assign_reg_from_local_load_with_metrics(regs, dst_idx, src_idx, collect_metrics);
    }
}

#[inline(always)]
#[cfg(test)]
#[allow(dead_code)]
pub(super) fn assign_reg_from_local_store(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
) {
    let _ = frame_raw;
    assign_reg_from_local_store_with_metrics(regs, dst_idx, src_idx, crate::vm::vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(super) fn assign_reg_from_local_store_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    collect_metrics: bool,
) {
    copy_local_store_register_value_with_metrics(regs, dst_idx, src_idx, collect_metrics);
}

#[inline(always)]
#[cfg(test)]
#[allow(dead_code)]
pub(super) fn assign_reg_from_reg_or_take(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    may_take: bool,
) {
    let _ = frame_raw;
    assign_reg_from_reg_or_take_with_metrics(
        regs,
        dst_idx,
        src_idx,
        may_take,
        crate::vm::vm_runtime_metrics_enabled(),
    );
}

#[inline(always)]
pub(super) fn assign_reg_from_reg_or_take_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    may_take: bool,
    collect_metrics: bool,
) {
    if !may_take || dst_idx == src_idx {
        assign_reg_from_reg_with_metrics(regs, dst_idx, src_idx, collect_metrics);
        return;
    }
    let value = take_register_value(regs, src_idx);
    write_register_value_with_metrics(regs, dst_idx, value, collect_metrics);
}

#[inline(always)]
#[cfg(test)]
#[allow(dead_code)]
pub(super) fn assign_local_from_reg_or_take(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    may_take: bool,
) {
    let _ = frame_raw;
    assign_local_from_reg_or_take_with_metrics(
        regs,
        dst_idx,
        src_idx,
        may_take,
        crate::vm::vm_runtime_metrics_enabled(),
    );
}

#[inline(always)]
pub(super) fn assign_local_from_reg_or_take_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    may_take: bool,
    collect_metrics: bool,
) {
    if may_take {
        assign_reg_from_reg_or_take_with_metrics(regs, dst_idx, src_idx, true, collect_metrics);
    } else {
        assign_reg_from_local_store_with_metrics(regs, dst_idx, src_idx, collect_metrics);
    }
}

#[inline(always)]
pub(super) fn local_store_may_take_source(func: &Function, pc: usize) -> bool {
    func.analysis
        .as_ref()
        .and_then(|analysis| analysis.perf.local_copy(pc))
        .is_some_and(|fact| fact.move_source)
}

#[inline(always)]
pub(super) fn local_load_may_take_source(func: &Function, pc: usize) -> bool {
    func.analysis
        .as_ref()
        .and_then(|analysis| analysis.perf.local_copy(pc))
        .is_some_and(|fact| fact.move_source)
}

#[inline(always)]
pub(super) fn register_move_may_take_source(func: &Function, pc: usize, src: u16) -> bool {
    if let Some(analysis) = func.analysis.as_ref() {
        return analysis.perf.register_copy(pc).is_some_and(|fact| fact.move_source);
    }
    func.code
        .get(pc + 1..)
        .is_some_and(|ops| crate::vm::register_dead_for_move_take(ops.iter(), src))
}

#[inline(always)]
pub(super) fn move_reg_value(regs: &mut [Val], src_idx: usize) -> Val {
    move_register_value(regs, src_idx)
}

#[inline(always)]
pub(super) fn move_reg_to_reg(regs: &mut [Val], src_idx: usize, dst_idx: usize, collect_metrics: bool) {
    if src_idx == dst_idx {
        return;
    }
    let value = move_reg_value(regs, src_idx);
    write_register_value_with_metrics(regs, dst_idx, value, collect_metrics);
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
    regs: &mut [Val],
    slots: &mut [Option<ForRangeState>],
    guard_pc: usize,
    body_pc: usize,
    exit_pc: usize,
    idx: u16,
    write_idx: bool,
    collect_metrics: bool,
) -> Result<usize> {
    let idx_reg = idx as usize;
    let next_pc = {
        let state = fetch_for_range_state(slots, guard_pc)?;
        if state.should_continue() {
            if write_idx {
                assign_reg_with_metrics(regs, idx_reg, Val::Int(state.current), collect_metrics);
            }
            state.current += state.step;
            body_pc
        } else {
            if write_idx {
                assign_reg_with_metrics(regs, idx_reg, Val::Int(state.current), collect_metrics);
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
    vm: &mut Vm,
    callee_regs: &mut [Val],
    base_idx: usize,
    retc: usize,
    expected: usize,
    meta: CallFrameMeta,
    collect_metrics: bool,
) {
    match meta.caller_window {
        RegisterWindowRef::Current => {
            let dest_base = meta.ret_base as usize;
            let limit = expected.min(retc);
            for i in 0..limit {
                let src_idx = base_idx + i;
                let dst_idx = dest_base + i;
                move_reg_to_reg(callee_regs, src_idx, dst_idx, collect_metrics);
            }
            if expected > retc {
                for i in retc..expected {
                    let dst_idx = dest_base + i;
                    write_register_value_with_metrics(callee_regs, dst_idx, Val::Nil, collect_metrics);
                }
            }
        }
        RegisterWindowRef::Base(base) => {
            let dest_base = base + meta.ret_base as usize;
            let limit = expected.min(retc);
            for i in 0..limit {
                let src_idx = base_idx + i;
                write_register_value_with_metrics(
                    &mut vm.stack,
                    dest_base + i,
                    move_reg_value(callee_regs, src_idx),
                    collect_metrics,
                );
            }
            if expected > retc {
                for i in retc..expected {
                    write_register_value_with_metrics(&mut vm.stack, dest_base + i, Val::Nil, collect_metrics);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::analysis::{FunctionAnalysis, vm_runtime_metrics_reset, vm_runtime_metrics_snapshot};
    use crate::vm::bytecode::Op;

    fn function_with_analysis(analysis: Option<FunctionAnalysis>) -> Function {
        Function {
            consts: Vec::new(),
            code: vec![Op::Move(1, 0), Op::Ret { base: 1, retc: 1 }],
            n_regs: 2,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis,
        }
    }

    #[test]
    fn assign_reg_from_reg_skips_self_copy() {
        vm_runtime_metrics_reset();
        let mut regs = vec![Val::from_str("longer-than-short")];

        assign_reg_from_reg(std::ptr::null_mut(), &mut regs, 0, 0);

        let metrics = vm_runtime_metrics_snapshot();
        assert_eq!(metrics.register_writes, 0);
        assert_eq!(metrics.val_clones, 0);
        assert_eq!(metrics.heap_val_clones, 0);
    }

    #[test]
    fn assign_reg_from_reg_counts_cross_reg_copy() {
        vm_runtime_metrics_reset();
        let mut regs = vec![Val::from_str("longer-than-short"), Val::Nil];

        assign_reg_from_reg(std::ptr::null_mut(), &mut regs, 1, 0);

        let metrics = vm_runtime_metrics_snapshot();
        assert_eq!(metrics.register_writes, 1);
        assert_eq!(metrics.val_clones, 1);
        assert_eq!(metrics.heap_val_clones, 1);
    }

    #[test]
    fn assign_reg_from_reg_or_take_moves_dead_source_without_clone() {
        vm_runtime_metrics_reset();
        let mut regs = vec![Val::from_str("longer-than-short"), Val::Nil];

        assign_reg_from_reg_or_take(std::ptr::null_mut(), &mut regs, 1, 0, true);

        let metrics = vm_runtime_metrics_snapshot();
        assert_eq!(regs[0], Val::Nil);
        assert_eq!(regs[1], Val::from_str("longer-than-short"));
        assert_eq!(metrics.register_writes, 1);
        assert_eq!(metrics.val_clones, 0);
        assert_eq!(metrics.heap_val_clones, 0);
    }

    #[test]
    fn assign_reg_from_reg_or_take_copies_live_source() {
        vm_runtime_metrics_reset();
        let mut regs = vec![Val::from_str("longer-than-short"), Val::Nil];

        assign_reg_from_reg_or_take(std::ptr::null_mut(), &mut regs, 1, 0, false);

        let metrics = vm_runtime_metrics_snapshot();
        assert_eq!(regs[0], Val::from_str("longer-than-short"));
        assert_eq!(regs[1], Val::from_str("longer-than-short"));
        assert_eq!(metrics.register_writes, 1);
        assert_eq!(metrics.val_clones, 1);
        assert_eq!(metrics.heap_val_clones, 1);
    }

    #[test]
    fn register_move_take_uses_performance_fact_when_analysis_exists() {
        let func = function_with_analysis(Some(FunctionAnalysis::default()));

        assert!(!register_move_may_take_source(&func, 0, 0));
    }

    #[test]
    fn register_move_take_falls_back_to_scan_without_analysis() {
        let func = function_with_analysis(None);

        assert!(register_move_may_take_source(&func, 0, 0));
    }
}
