//! Packed 32-bit bytecode fast-path interpreter.
//!
//! Switch-free BC32 dispatch with packed hot slots, sentinel skips, and range tail fusion.

use arcstr::ArcStr;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::op::BinOp;
use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use crate::val::{ClosureCapture, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::{AllocationRegion, RegionAllocator};
use crate::vm::bc32::{self, Bc32Decoded, Tag};
use crate::vm::bytecode::{CaptureSpec, Function, Op, rk_index, rk_is_const, rk_make_const};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    AccessIc, CallIc, CallReturnLayout, ForRangeState, GlobalEntry, IndexIc, PackedAddOperand, PackedArithOp,
    PackedCmpImmOp, PackedCmpOp, PackedHotCallKind, PackedHotEntry, PackedHotKind, PackedHotSlot, PackedRangeTail,
    PackedValueOperand, RuntimeDispatchMode,
};
use crate::vm::vm::frame::{CallArgs, CallFrameMeta, CallFrameStackGuard, FrameState, RegisterSpan};
use crate::vm::{
    VmBc32FallbackMetric, VmCallMetric, copy_value_for_register, copy_value_for_register_with_metrics,
    record_bc32_fallback_op_known_enabled, record_bc32_fallback_reason_known_enabled, record_call_op_known_enabled,
    record_opcode_step_known_enabled, record_quickening_build_attempt_known_enabled,
    record_quickening_build_success_known_enabled, record_quickening_hit_known_enabled,
    record_quickening_miss_known_enabled, record_quickening_sentinel_skip_known_enabled, restore_register_value,
    take_register_value,
};

use super::FrameRuntimeView;
use super::helpers::{
    advance_for_range_tail, assign_local_from_reg_or_take, assign_local_from_reg_or_take_with_metrics, assign_reg,
    assign_reg_const_copy, assign_reg_const_copy_with_metrics, assign_reg_copy, assign_reg_from_local_load,
    assign_reg_from_local_load_with_metrics, assign_reg_from_reg, assign_reg_from_reg_or_take_with_metrics,
    assign_reg_from_reg_with_metrics, fetch_for_range_state, frame_return_common, handle_return_common,
    insert_map_entry, local_store_may_take_source, push_list_entry, register_move_may_take_source,
};
use super::invoke::{
    ArgWindow, NativeCallable, ReturnSlot, clear_pending_resume_pc, invoke_native_callable_with_ic,
    invoke_rust_fast_function_named, invoke_rust_function_named_fast, take_pending_resume_pc,
};
use super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, float_binop, floor_div_i64, int_binop, int_binop_imm, rk_read};
use super::method_ops;
use super::plan::get_or_build_named_call_site_plan;
use super::raw_boundary::region_allocator;

mod call;
mod closure;
mod cold_basic;
mod cold_math;
mod decode;
mod fetch;
mod hot_exec;
mod hot_values;
mod named_args;

#[inline]
fn packed_move_may_take_source(f: &Function, pc: usize, src: u16) -> bool {
    let instr_pc = packed_instr_pc(f, pc);
    register_move_may_take_source(f, instr_pc, src)
}

#[inline]
fn packed_instr_pc(f: &Function, pc: usize) -> usize {
    f.bc32_decoded
        .as_deref()
        .and_then(|decoded| decoded.word_to_instr.get(pc).copied())
        .map(|idx| idx as usize)
        .unwrap_or(pc)
}

fn assign_move_call_args(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    f: &Function,
    pc: usize,
    moves: &[(u16, u16)],
    collect_metrics: bool,
) {
    let start_instr = f
        .bc32_decoded
        .as_deref()
        .and_then(|decoded| decoded.word_to_instr.get(pc))
        .copied()
        .map(|idx| idx as usize);
    for (offset, (dst, src)) in moves.iter().enumerate() {
        let pc = start_instr.map_or(pc + offset, |idx| idx + offset);
        let may_take = register_move_may_take_source(f, pc, *src);
        assign_reg_from_reg_or_take_with_metrics(
            frame_raw,
            regs,
            *dst as usize,
            *src as usize,
            may_take,
            collect_metrics,
        );
    }
}

#[inline(always)]
fn assign_packed_move_with_metrics(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    func: &Function,
    pc: usize,
    dst: u16,
    src: u16,
    collect_metrics: bool,
) {
    let may_take = packed_move_may_take_source(func, pc, src);
    assign_reg_from_reg_or_take_with_metrics(frame_raw, regs, dst as usize, src as usize, may_take, collect_metrics);
}

#[inline(always)]
fn assign_packed_const_with_metrics(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    func: &Function,
    dst: u16,
    kidx: u16,
    collect_metrics: bool,
) {
    assign_reg_const_copy_with_metrics(
        frame_raw,
        regs,
        dst as usize,
        &func.consts[kidx as usize],
        collect_metrics,
    );
}

#[inline(always)]
fn assign_packed_local_load_with_metrics(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    idx: u16,
    collect_metrics: bool,
) {
    assign_reg_from_local_load_with_metrics(frame_raw, regs, dst as usize, idx as usize, collect_metrics);
}

#[inline(always)]
fn assign_packed_local_store_with_metrics(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    func: &Function,
    pc: usize,
    idx: u16,
    src: u16,
    collect_metrics: bool,
) {
    let may_take = local_store_may_take_source(func, pc);
    assign_local_from_reg_or_take_with_metrics(frame_raw, regs, idx as usize, src as usize, may_take, collect_metrics);
}
mod stats;
use closure::make_closure_value;
use cold_basic::*;
use cold_math::*;
use decode::*;
use fetch::*;
use hot_exec::*;
use hot_values::*;
use named_args::load_named_pairs;
use stats::*;

fn hot_call_operands(kind: &PackedHotKind) -> Option<(u16, u16, u8, u8, PackedHotCallKind)> {
    match kind {
        PackedHotKind::Call { f, base, argc, retc } => Some((*f, *base, *argc, *retc, PackedHotCallKind::Generic)),
        PackedHotKind::CallNativeFast { f, base, argc, retc } => {
            Some((*f, *base, *argc, *retc, PackedHotCallKind::NativeFast))
        }
        PackedHotKind::CallClosureExact { f, base, argc, retc } => {
            Some((*f, *base, *argc, *retc, PackedHotCallKind::ClosureExact))
        }
        PackedHotKind::CallExact { f, base, argc, retc } => Some((*f, *base, *argc, *retc, PackedHotCallKind::Exact)),
        PackedHotKind::MoveCall {
            f,
            base,
            argc,
            retc,
            call_kind,
            ..
        } => Some((*f, *base, *argc, *retc, *call_kind)),
        _ => None,
    }
}

#[inline]
fn record_packed_call_metric(kind: PackedHotCallKind) {
    let metric = match kind {
        PackedHotCallKind::Generic => VmCallMetric::Generic,
        PackedHotCallKind::NativeFast => VmCallMetric::Native,
        PackedHotCallKind::ClosureExact => VmCallMetric::Closure,
        PackedHotCallKind::Exact => VmCallMetric::Exact,
    };
    record_call_op_known_enabled(metric);
}

#[allow(clippy::too_many_arguments)]
fn run_packed_call_kind(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: &mut usize,
    next_pc: usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    f: u16,
    base: u16,
    argc: u8,
    retc: u8,
    call_kind: PackedHotCallKind,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    match call_kind {
        PackedHotCallKind::Generic => call::run_call_packed(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc,
            next_pc,
            frame_base,
            region_allocator_ptr,
            self_ptr,
            f,
            base,
            argc,
            retc,
            collect_metrics,
        ),
        PackedHotCallKind::NativeFast => call::run_call_native_fast_packed(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc,
            next_pc,
            frame_base,
            region_allocator_ptr,
            self_ptr,
            f,
            base,
            argc,
            retc,
            collect_metrics,
        ),
        PackedHotCallKind::ClosureExact => call::run_call_closure_exact_packed(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc,
            next_pc,
            frame_base,
            region_allocator_ptr,
            self_ptr,
            f,
            base,
            argc,
            retc,
        ),
        PackedHotCallKind::Exact => call::run_call_exact_packed(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc,
            next_pc,
            frame_base,
            region_allocator_ptr,
            self_ptr,
            f,
            base,
            argc,
            retc,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_packed_code(runtime: &mut FrameRuntimeView<'_, '_>) -> Result<Option<Val>> {
    let frame_raw = runtime.frame_raw;
    let regs = &mut *runtime.regs;
    let ctx = &mut *runtime.ctx;
    let func = runtime.dispatch_plan.function();
    let RuntimeDispatchMode::Packed(packed_code) = runtime.dispatch_plan.mode() else {
        unreachable!("packed executor requires packed dispatch mode");
    };
    let code32 = packed_code.words;
    let decoded = packed_code.decoded;
    let frame_base = runtime.base;
    let frame_captures = runtime.captures;
    let frame_capture_specs = runtime.capture_specs;
    let region_plan = runtime.region_plan;
    let region_allocator_ptr = runtime.region_allocator;
    let self_ptr = runtime.self_ptr;
    let access_ic = &mut *runtime.caches.access_ic;
    let index_ic = &mut *runtime.caches.index_ic;
    let global_ic = &mut *runtime.caches.global_ic;
    let call_ic = &mut *runtime.caches.call_ic;
    let for_range_ic = &mut *runtime.caches.for_range;
    let packed_hot = &mut *runtime.caches.packed_hot;
    #[cfg(debug_assertions)]
    let _stats_guard = PackedHotStatsGuard::new();
    let mut pc = runtime.pc;
    let f = func;
    let collect_metrics = runtime.collect_metrics;

    while pc < code32.len() {
        if collect_metrics {
            record_opcode_step_known_enabled();
        }
        let word = code32[pc];
        let raw_tag = bc32::tag_of(word);
        if raw_tag == bc32::TAG_REG_EXT {
            pc += 1;
            continue;
        }
        if raw_tag == bc32::TAG_EXT {
            let is_decoded_instr = decoded
                .and_then(|decoded_table| decoded_table.word_to_instr.get(pc))
                .is_some_and(|idx| *idx != u32::MAX);
            if !is_decoded_instr && decoded.is_some() {
                pc += 1;
                continue;
            }
            if !is_decoded_instr {
                return frame_return_common(
                    frame_raw,
                    pc,
                    Err(anyhow!(
                        "bc32: unexpected Ext word without preceding opcode at pc {}",
                        pc
                    )),
                )
                .map(Some);
            }
        }
        let mut skip_build = false;
        if let Some(entry) = packed_hot.get(pc).and_then(|slot| slot.as_ref()) {
            match entry {
                PackedHotEntry::Slot(slot) => {
                    if slot.word == word {
                        if collect_metrics {
                            record_quickening_hit_known_enabled();
                        }
                        record_hot_hit();
                        if let PackedHotKind::Ret { base, retc } = &slot.kind {
                            let retc = *retc as usize;
                            let base_idx = *base as usize;
                            let ret_val = if retc > 0 {
                                take_register_value(regs, base_idx)
                            } else {
                                Val::Nil
                            };
                            return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr)
                                .map(Some);
                        }
                        if let Some((f_reg, base, argc, retc, call_kind)) = hot_call_operands(&slot.kind) {
                            if collect_metrics {
                                record_packed_call_metric(call_kind);
                            }
                            if let PackedHotKind::MoveCall { moves, .. } = &slot.kind {
                                assign_move_call_args(frame_raw, regs, f, pc, moves, collect_metrics);
                            }
                            if let Some(value) = run_packed_call_kind(
                                frame_raw,
                                regs,
                                ctx,
                                call_ic,
                                &mut pc,
                                slot.next_pc,
                                frame_base,
                                region_allocator_ptr,
                                self_ptr,
                                f_reg,
                                base,
                                argc,
                                retc,
                                call_kind,
                                collect_metrics,
                            )? {
                                return Ok(Some(value));
                            }
                            continue;
                        }
                        let override_pc = exec_hot_slot(
                            slot,
                            frame_raw,
                            regs,
                            f,
                            ctx,
                            frame_captures,
                            frame_capture_specs,
                            access_ic,
                            index_ic,
                            global_ic,
                            call_ic,
                            for_range_ic,
                            pc,
                            frame_base,
                            region_plan,
                            region_allocator_ptr,
                            collect_metrics,
                        )?;
                        pc = override_pc.unwrap_or(slot.next_pc);
                        continue;
                    }
                }
                PackedHotEntry::Miss(last_word) => {
                    if *last_word == word {
                        if collect_metrics {
                            record_quickening_sentinel_skip_known_enabled();
                            record_bc32_fallback_reason_known_enabled(VmBc32FallbackMetric::SentinelSkip);
                        }
                        record_sentinel_skip(word);
                        skip_build = true;
                    }
                }
            }
        }
        if !skip_build {
            if let Some(entry) = packed_hot.get_mut(pc)
                && let Some(existing) = entry
            {
                match existing {
                    PackedHotEntry::Slot(slot) if slot.word != word => {
                        if collect_metrics {
                            record_bc32_fallback_reason_known_enabled(VmBc32FallbackMetric::StaleSlot);
                        }
                        *entry = None;
                    }
                    PackedHotEntry::Miss(last_word) if *last_word != word => {
                        if collect_metrics {
                            record_bc32_fallback_reason_known_enabled(VmBc32FallbackMetric::StaleMiss);
                        }
                        *entry = None;
                    }
                    _ => {}
                }
            }
            record_build_attempt();
            if collect_metrics {
                record_quickening_build_attempt_known_enabled();
            }
            if let Some(entry) = build_hot_slot(code32, decoded, &func.consts, pc, word, raw_tag) {
                if collect_metrics {
                    record_quickening_build_success_known_enabled();
                }
                record_build_success();
                let next_pc = entry.next_pc;
                if let PackedHotKind::Ret { base, retc } = &entry.kind {
                    let retc = *retc as usize;
                    let base_idx = *base as usize;
                    let ret_val = if retc > 0 {
                        take_register_value(regs, base_idx)
                    } else {
                        Val::Nil
                    };
                    if packed_hot.len() <= pc {
                        packed_hot.resize(pc + 1, None);
                    }
                    packed_hot[pc] = Some(PackedHotEntry::Slot(entry));
                    return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr).map(Some);
                }
                if let Some((f_reg, base_reg, argc_count, retc_count, call_kind)) = hot_call_operands(&entry.kind) {
                    let next_pc = entry.next_pc;
                    if collect_metrics {
                        record_packed_call_metric(call_kind);
                    }
                    if let PackedHotKind::MoveCall { moves, .. } = &entry.kind {
                        assign_move_call_args(frame_raw, regs, f, pc, moves, collect_metrics);
                    }
                    if packed_hot.len() <= pc {
                        packed_hot.resize(pc + 1, None);
                    }
                    packed_hot[pc] = Some(PackedHotEntry::Slot(entry));
                    if let Some(value) = run_packed_call_kind(
                        frame_raw,
                        regs,
                        ctx,
                        call_ic,
                        &mut pc,
                        next_pc,
                        frame_base,
                        region_allocator_ptr,
                        self_ptr,
                        f_reg,
                        base_reg,
                        argc_count,
                        retc_count,
                        call_kind,
                        collect_metrics,
                    )? {
                        return Ok(Some(value));
                    }
                    continue;
                }
                let override_pc = exec_hot_slot(
                    &entry,
                    frame_raw,
                    regs,
                    f,
                    ctx,
                    frame_captures,
                    frame_capture_specs,
                    access_ic,
                    index_ic,
                    global_ic,
                    call_ic,
                    for_range_ic,
                    pc,
                    frame_base,
                    region_plan,
                    region_allocator_ptr,
                    collect_metrics,
                )?;
                if packed_hot.len() <= pc {
                    packed_hot.resize(pc + 1, None);
                }
                packed_hot[pc] = Some(PackedHotEntry::Slot(entry));
                pc = override_pc.unwrap_or(next_pc);
                continue;
            } else {
                if collect_metrics {
                    record_bc32_fallback_op_known_enabled();
                    record_bc32_fallback_reason_known_enabled(VmBc32FallbackMetric::BuildMiss);
                    record_quickening_miss_known_enabled();
                }
                record_build_miss(word);
                if packed_hot.len() <= pc {
                    packed_hot.resize(pc + 1, None);
                }
                packed_hot[pc] = Some(PackedHotEntry::Miss(word));
            }
        }
        let (op, next_pc_default) = match fetch_packed_op(decoded, code32, pc) {
            Ok(pair) => pair,
            Err(err) => {
                return frame_return_common(frame_raw, pc, Err(err)).map(Some);
            }
        };
        if let Some(next_pc) = try_exec_math_op(&op, frame_raw, regs, f, next_pc_default)? {
            pc = next_pc;
            continue;
        }
        if handles_basic_op(&op) {
            if let Some(value) = exec_basic_op(
                op,
                frame_raw,
                regs,
                ctx,
                f,
                &mut pc,
                next_pc_default,
                frame_base,
                access_ic,
                index_ic,
                global_ic,
                for_range_ic,
                region_plan,
                region_allocator_ptr,
                self_ptr,
            )? {
                return Ok(Some(value));
            }
            continue;
        }
        match op {
            Op::Nop => {
                pc = next_pc_default;
            }
            Op::LoadK(dst, k) => {
                assign_reg_const_copy_with_metrics(
                    frame_raw,
                    regs,
                    dst as usize,
                    &f.consts[k as usize],
                    collect_metrics,
                );
                pc = next_pc_default;
            }
            Op::Move(dst, src) => {
                let may_take = packed_move_may_take_source(f, pc, src);
                assign_reg_from_reg_or_take_with_metrics(
                    frame_raw,
                    regs,
                    dst as usize,
                    src as usize,
                    may_take,
                    collect_metrics,
                );
                pc = next_pc_default;
            }
            Op::ToStr(dst, src) => {
                let s = Val::to_str_value(&regs[src as usize]);
                assign_reg(frame_raw, regs, dst as usize, s);
                pc = next_pc_default;
            }
            Op::Call {
                f: rf,
                base,
                argc,
                retc,
            } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Generic);
                }
                if let Some(value) = call::run_call_packed(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    next_pc_default,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base,
                    argc,
                    retc,
                    collect_metrics,
                )? {
                    return Ok(Some(value));
                }
            }
            Op::CallNativeFast {
                f: rf,
                base,
                argc,
                retc,
            } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Native);
                }
                if let Some(value) = call::run_call_native_fast_packed(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    next_pc_default,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base,
                    argc,
                    retc,
                    collect_metrics,
                )? {
                    return Ok(Some(value));
                }
            }
            Op::CallMethod0 { dst, receiver, method } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Method);
                }
                method_ops::run_call_method0(frame_raw, regs, ctx, f, dst, receiver, method)?;
                pc = next_pc_default;
            }
            Op::CallGlobalMethod0 { dst, receiver, method } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Method);
                }
                method_ops::run_call_global_method0(frame_raw, regs, ctx, f, global_ic, pc, dst, receiver, method)?;
                pc = next_pc_default;
            }
            Op::CallExact {
                f: rf,
                base,
                argc,
                retc,
            } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Exact);
                }
                if let Some(value) = call::run_call_exact_packed(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    next_pc_default,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base,
                    argc,
                    retc,
                )? {
                    return Ok(Some(value));
                }
            }
            Op::CallClosureExact {
                f: rf,
                base,
                argc,
                retc,
            } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Closure);
                }
                if let Some(value) = call::run_call_closure_exact_packed(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    next_pc_default,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base,
                    argc,
                    retc,
                )? {
                    return Ok(Some(value));
                }
            }
            Op::CallNamed {
                f: rf,
                base_pos,
                posc,
                base_named,
                namedc,
                retc,
            } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Named);
                }
                if let Some(value) = call::run_call_named_packed(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    next_pc_default,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
                    collect_metrics,
                )? {
                    return Ok(Some(value));
                }
            }
            Op::CallNamedFallback {
                f: rf,
                base_pos,
                posc,
                base_named,
                namedc,
                retc,
            } => {
                if collect_metrics {
                    record_call_op_known_enabled(VmCallMetric::Named);
                }
                if let Some(value) = call::run_call_named_packed(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    next_pc_default,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
                    collect_metrics,
                )? {
                    return Ok(Some(value));
                }
            }
            Op::LoadCapture { dst, idx } => {
                closure::run_load_capture(frame_raw, regs, ctx, frame_captures, frame_capture_specs, dst, idx)?;
                pc = next_pc_default;
            }
            Op::JmpFalseSet { r, dst, ofs } => {
                let cond_falsey = matches!(regs[r as usize], Val::Nil | Val::Bool(false));
                if cond_falsey {
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(false));
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                } else {
                    pc = next_pc_default;
                }
            }
            Op::JmpTrueSet { r, dst, ofs } => {
                let cond_truthy = !matches!(regs[r as usize], Val::Nil | Val::Bool(false));
                if cond_truthy {
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(true));
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                } else {
                    pc = next_pc_default;
                }
            }
            Op::ListSlice { dst, src, start } => {
                let (list, start_idx) = match (&regs[src as usize], &regs[start as usize]) {
                    (Val::List(l), Val::Int(i)) => (l, *i),
                    (a, b) => {
                        return frame_return_common(
                            frame_raw,
                            pc,
                            Err(anyhow!("ListSlice expects (List, Int), got ({:?}, {:?})", a, b)),
                        )
                        .map(Some);
                    }
                };
                if start_idx <= 0 {
                    assign_reg(frame_raw, regs, dst as usize, Val::List(list.clone()));
                } else {
                    let s = start_idx as usize;
                    if s >= list.len() {
                        assign_reg(frame_raw, regs, dst as usize, Val::List(Vec::<Val>::new().into()));
                    } else {
                        let use_thread_local = region_plan
                            .as_ref()
                            .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                            .unwrap_or(false);
                        if use_thread_local {
                            let allocator = region_allocator(region_allocator_ptr);
                            let slice_val = allocator.with_val_buffer(list.len() - s, |scratch| {
                                scratch.extend(
                                    list[s..]
                                        .iter()
                                        .map(|value| copy_value_for_register_with_metrics(value, collect_metrics)),
                                );
                                let data = scratch.split_off(0);
                                Val::List(data.into())
                            });
                            assign_reg(frame_raw, regs, dst as usize, slice_val);
                        } else {
                            assign_reg(frame_raw, regs, dst as usize, Val::List((list[s..]).to_vec().into()));
                        }
                    }
                }
                pc = next_pc_default;
            }
            Op::ListPush { list, val } => {
                let pushed_val = copy_value_for_register_with_metrics(&regs[val as usize], collect_metrics);
                match &mut regs[list as usize] {
                    Val::List(arc) => {
                        Arc::make_mut(arc).push(pushed_val);
                    }
                    _ => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("ListPush target is not a List")))
                            .map(Some);
                    }
                }
                pc = next_pc_default;
            }
            Op::ListPushMove { list, val } => {
                let list_idx = list as usize;
                let val_idx = val as usize;
                if list_idx == val_idx {
                    let pushed_val = copy_value_for_register_with_metrics(&regs[val_idx], collect_metrics);
                    match &mut regs[list_idx] {
                        Val::List(arc) => {
                            Arc::make_mut(arc).push(pushed_val);
                        }
                        _ => {
                            return frame_return_common(frame_raw, pc, Err(anyhow!("ListPush target is not a List")))
                                .map(Some);
                        }
                    }
                } else {
                    if !matches!(regs[list_idx], Val::List(_)) {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("ListPush target is not a List")))
                            .map(Some);
                    }
                    let pushed_val = take_register_value(regs, val_idx);
                    match &mut regs[list_idx] {
                        Val::List(arc) => {
                            Arc::make_mut(arc).push(pushed_val);
                        }
                        _ => unreachable!("ListPush target was checked before moving value"),
                    }
                }
                pc = next_pc_default;
            }
            _ => {
                // Unreachable for bc32-packed functions (subset only)
                return frame_return_common(
                    frame_raw,
                    pc,
                    Err(anyhow!("bc32: unsupported opcode in packed function")),
                )
                .map(Some);
            }
        }
    }
    runtime.pc = pc;
    Ok(None)
}
