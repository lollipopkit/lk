//! Standard bytecode interpreter — match-dispatch on Op enum.
//!
//! This module implements the primary execution loop for LK bytecode.
//! Each iteration does a `match &f.code[pc]` on the Op enum (70+ variants)
//! and executes the instruction. Inline caches (Access, Index, Global, Call,
//! ForRange) are per-instruction-site to accelerate polymorphic operations.
//!
//! When a Function has a `code32` packed encoding, the BC32 fast path in
//! `packed.rs` is preferred. This interpreter handles all remaining cases
//! including peephole-fused ops (CmpLtImmJmp, AddIntImmJmp, etc.) that

use std::sync::Arc;

mod arithmetic_ops;
mod call_ops;
mod closure_ops;
mod compare_ops;
mod container_ops;
mod control_ops;
mod global_ops;
mod pattern_ops;
mod string_ops;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, ClosureInit, ClosureValue, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::RegionAllocator;
use crate::vm::bytecode::{CaptureSpec, Function, Op};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{CallIc, CallReturnLayout, ForRangeState, VmCaches};
use crate::vm::vm::frame::{CallArgs, CallFrameMeta, CallFrameStackGuard, FrameState, RegisterSpan};
use crate::vm::{
    VmCallMetric, VmContainerMetric, record_branch_op, record_call_op, record_container_op, record_opcode_step,
    vm_runtime_metrics_enabled,
};

use super::helpers::{advance_for_range_tail, assign_reg, frame_return_common, handle_return_common};
use super::math::floor_div_i64;
use super::method_ops;
use super::plan::build_named_call_plan;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_opcode_code(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    caches: &mut VmCaches<'_>,
    func: &Function,
    pc_ref: &mut usize,
    frame_base: usize,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
) -> Result<Option<Val>> {
    let access_ic = &mut *caches.access_ic;
    let index_ic = &mut *caches.index_ic;
    let global_ic = &mut *caches.global_ic;
    let call_ic = &mut *caches.call_ic;
    let for_range_ic = &mut *caches.for_range;
    let mut pc = *pc_ref;
    let f = func;
    if access_ic.len() < f.code.len() {
        access_ic.resize(f.code.len(), None);
    }
    if index_ic.len() < f.code.len() {
        index_ic.resize(f.code.len(), None);
    }
    if global_ic.len() < f.code.len() {
        global_ic.resize(f.code.len(), None);
    }
    if call_ic.len() < f.code.len() {
        call_ic.resize(f.code.len(), None);
    }
    if for_range_ic.len() < f.code.len() {
        for_range_ic.resize(f.code.len(), None);
    }
    let collect_metrics = vm_runtime_metrics_enabled();
    let record_branch = |typed| {
        if collect_metrics {
            record_branch_op(typed);
        }
    };
    let record_call = |kind| {
        if collect_metrics {
            record_call_op(kind);
        }
    };
    let record_container = |kind| {
        if collect_metrics {
            record_container_op(kind);
        }
    };
    while pc < f.code.len() {
        if collect_metrics {
            record_opcode_step();
        }
        match &f.code[pc] {
            Op::LoadK(dst, k) => {
                assign_reg(frame_raw, regs, *dst as usize, f.consts[*k as usize].clone());
                pc += 1;
            }
            Op::Move(dst, src) => {
                assign_reg(frame_raw, regs, *dst as usize, regs[*src as usize].clone());
                pc += 1;
            }
            Op::ToStr(dst, src) => {
                if string_ops::run_to_str(frame_raw, regs, &f.consts, &f.code, pc, *dst, *src) {
                    pc += 2;
                } else {
                    pc += 1;
                }
            }
            Op::Add(dst, a, b) => {
                arithmetic_ops::run_add(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::StrConcatKnownCap(dst, a, b) => {
                arithmetic_ops::run_str_concat_known_cap(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::StrConcatToStr(dst, lhs, src) => {
                arithmetic_ops::run_str_concat_to_str(frame_raw, regs, &f.consts, *dst, *lhs, *src)?;
                pc += 1;
            }
            Op::Sub(dst, a, b) => {
                arithmetic_ops::run_sub(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::Mul(dst, a, b) => {
                arithmetic_ops::run_mul(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::Div(dst, a, b) => {
                arithmetic_ops::run_div(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::Mod(dst, a, b) => {
                arithmetic_ops::run_mod(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::AddInt(dst, a, b) => {
                arithmetic_ops::run_add_int(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::AddFloat(dst, a, b) => {
                arithmetic_ops::run_add_float(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::AddIntImm(dst, a, imm) => {
                arithmetic_ops::run_add_int_imm(frame_raw, regs, &f.consts, *dst, *a, *imm)?;
                pc += 1;
            }
            Op::SubInt(dst, a, b) => {
                arithmetic_ops::run_sub_int(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::SubFloat(dst, a, b) => {
                arithmetic_ops::run_sub_float(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            op @ (Op::CmpEqImm(..)
            | Op::CmpNeImm(..)
            | Op::CmpLtImm(..)
            | Op::CmpLeImm(..)
            | Op::CmpGtImm(..)
            | Op::CmpGeImm(..)) => {
                pc = compare_ops::run_cmp_imm_or_branch(frame_raw, regs, &f.consts, &f.code, pc, op)?;
            }
            Op::MulInt(dst, a, b) => {
                arithmetic_ops::run_mul_int(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::MulFloat(dst, a, b) => {
                arithmetic_ops::run_mul_float(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::DivFloat(dst, a, b) => {
                arithmetic_ops::run_div_float(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::ModInt(dst, a, b) => {
                arithmetic_ops::run_mod_int(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::ModFloat(dst, a, b) => {
                arithmetic_ops::run_mod_float(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpEq(dst, a, b) => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_eq_jmp_false(regs, &f.consts, pc, *ofs, *a, *b);
                    continue;
                }
                compare_ops::run_cmp_eq(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpNe(dst, a, b) => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_ne_jmp_false(regs, &f.consts, pc, *ofs, *a, *b);
                    continue;
                }
                compare_ops::run_cmp_ne(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpLt(dst, a, b) => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_lt_jmp_false(regs, &f.consts, pc, *ofs, *a, *b)?;
                    continue;
                }
                compare_ops::run_cmp_lt(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpLe(dst, a, b) => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_le_jmp_false(regs, &f.consts, pc, *ofs, *a, *b)?;
                    continue;
                }
                compare_ops::run_cmp_le(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpGt(dst, a, b) => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_gt_jmp_false(regs, &f.consts, pc, *ofs, *a, *b)?;
                    continue;
                }
                compare_ops::run_cmp_gt(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpGe(dst, a, b) => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_ge_jmp_false(regs, &f.consts, pc, *ofs, *a, *b)?;
                    continue;
                }
                compare_ops::run_cmp_ge(frame_raw, regs, &f.consts, caches.quickening, pc, *dst, *a, *b)?;
                pc += 1;
            }
            Op::CmpI { dst, a, b, kind } => {
                if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = f.code.get(pc + 1)
                    && *r == *dst
                {
                    pc = compare_ops::run_cmp_i_jmp_false(regs, pc, *ofs, *a, *b, *kind)?;
                    continue;
                }
                compare_ops::run_cmp_i(frame_raw, regs, *dst, *a, *b, *kind)?;
                pc += 1;
            }
            Op::CmpIntJmp { kind, a, b, ofs } => {
                record_branch(true);
                let (Val::Int(lhs), Val::Int(rhs)) = (&regs[*a as usize], &regs[*b as usize]) else {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("CmpIntJmp expects integer registers")))
                        .map(Some);
                };
                if kind.eval(*lhs, *rhs) {
                    pc += 1;
                } else {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                }
            }
            Op::In(dst, a, b) => {
                compare_ops::run_in(frame_raw, regs, &f.consts, *dst, *a, *b)?;
                pc += 1;
            }
            Op::LoadLocal(dst, idx) => {
                assign_reg(frame_raw, regs, *dst as usize, regs[*idx as usize].clone());
                pc += 1;
            }
            Op::StoreLocal(idx, src) => {
                let v = regs[*src as usize].clone();
                assign_reg(frame_raw, regs, *idx as usize, v);
                pc += 1;
            }
            Op::LoadGlobal(dst, name_k) => {
                global_ops::run_load_global(frame_raw, regs, &f.consts, ctx, global_ic, pc, *dst, *name_k);
                pc += 1;
            }
            Op::DefineGlobal(name_k, src) => {
                global_ops::run_define_global(regs, &f.consts, ctx, *name_k, *src);
                pc += 1;
            }
            Op::LoadCapture { dst, idx } => {
                global_ops::run_load_capture(frame_raw, regs, ctx, frame_captures, frame_capture_specs, *dst, *idx)?;
                pc += 1;
            }
            Op::Access(dst, base, field) => {
                record_container(VmContainerMetric::Generic);
                container_ops::run_access(frame_raw, regs, access_ic, pc, *dst, *base, *field);
                pc += 1;
            }
            Op::AccessK(dst, base, kidx) => {
                record_container(VmContainerMetric::Generic);
                container_ops::run_access_k(frame_raw, regs, &f.consts, access_ic, pc, *dst, *base, *kidx);
                pc += 1;
            }
            Op::Len { dst, src } => {
                record_container(VmContainerMetric::Generic);
                container_ops::run_len(frame_raw, regs, *dst, *src);
                pc += 1;
            }
            Op::ListLen { dst, src } => {
                record_container(VmContainerMetric::List);
                container_ops::run_list_len(frame_raw, regs, *dst, *src);
                pc += 1;
            }
            Op::MapLen { dst, src } => {
                record_container(VmContainerMetric::Map);
                container_ops::run_map_len(frame_raw, regs, *dst, *src);
                pc += 1;
            }
            Op::StrLen { dst, src } => {
                record_container(VmContainerMetric::String);
                container_ops::run_str_len(frame_raw, regs, *dst, *src);
                pc += 1;
            }
            Op::Floor { dst, src } => {
                container_ops::run_floor(frame_raw, regs, *dst, *src);
                pc += 1;
            }
            Op::FloorDivImm { dst, src, imm } => {
                let out = match &regs[*src as usize] {
                    Val::Int(value) => Val::Int(floor_div_i64(*value, *imm as i64)),
                    Val::Float(value) => Val::Int((value / *imm as f64).floor() as i64),
                    _ => Val::Int(0),
                };
                assign_reg(frame_raw, regs, *dst as usize, out);
                pc += 1;
            }
            Op::StartsWithK(dst, src, kidx) => {
                record_container(VmContainerMetric::String);
                string_ops::run_starts_with_k(frame_raw, regs, &f.consts, *dst, *src, *kidx);
                pc += 1;
            }
            Op::ContainsK(dst, src, kidx) => {
                record_container(VmContainerMetric::String);
                string_ops::run_contains_k(frame_raw, regs, &f.consts, *dst, *src, *kidx);
                pc += 1;
            }
            Op::MapHas(dst, map, key) => {
                record_container(VmContainerMetric::Map);
                if let Err(err) = container_ops::run_map_has(frame_raw, regs, *dst, *map, *key) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::MapGetInterned(dst, map, kidx) => {
                record_container(VmContainerMetric::Map);
                container_ops::run_map_get_interned(frame_raw, regs, &f.consts, *dst, *map, *kidx);
                pc += 1;
            }
            Op::MapGetDynamic(dst, map, key) => {
                record_container(VmContainerMetric::Map);
                container_ops::run_map_get_dynamic(frame_raw, regs, *dst, *map, *key);
                pc += 1;
            }
            Op::MapHasK(dst, map, kidx) => {
                record_container(VmContainerMetric::Map);
                if let Err(err) = container_ops::run_map_has_k(frame_raw, regs, &f.consts, *dst, *map, *kidx) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::ListFoldAdd { acc, list } => {
                record_container(VmContainerMetric::List);
                container_ops::run_list_fold_add(frame_raw, regs, *acc, *list)?;
                pc += 1;
            }
            Op::MapValuesFoldAdd { acc, map } => {
                record_container(VmContainerMetric::Map);
                container_ops::run_map_values_fold_add(frame_raw, regs, *acc, *map)?;
                pc += 1;
            }
            Op::Index { dst, base, idx } => {
                record_container(VmContainerMetric::Generic);
                container_ops::run_index(frame_raw, regs, index_ic, caches.quickening, pc, *dst, *base, *idx)?;
                pc += 1;
            }
            Op::IndexK(dst, base, kidx) => {
                record_container(VmContainerMetric::Generic);
                container_ops::run_index_k(frame_raw, regs, &f.consts, *dst, *base, *kidx);
                pc += 1;
            }
            Op::ListIndexI(dst, base, index) => {
                record_container(VmContainerMetric::List);
                container_ops::run_list_index_i(frame_raw, regs, *dst, *base, *index);
                pc += 1;
            }
            Op::StrIndexI(dst, base, index) => {
                record_container(VmContainerMetric::String);
                container_ops::run_str_index_i(frame_raw, regs, *dst, *base, *index);
                pc += 1;
            }
            Op::PatternMatch { dst, src, plan } => {
                pattern_ops::run_pattern_match(frame_raw, regs, ctx, f, *dst, *src, *plan)?;
                pc += 1;
            }
            Op::PatternMatchOrFail {
                src,
                plan,
                err_kidx,
                is_const,
            } => {
                if let Err(err) =
                    pattern_ops::run_pattern_match_or_fail(frame_raw, regs, ctx, f, *src, *plan, *err_kidx, *is_const)
                {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::Raise { err_kidx } => {
                return frame_return_common(frame_raw, pc, pattern_ops::run_raise(f, *err_kidx)).map(Some);
            }
            Op::ToIter { dst, src } => {
                record_container(VmContainerMetric::Generic);
                container_ops::run_to_iter(frame_raw, regs, *dst, *src, region_plan, region_allocator_ptr);
                pc += 1;
            }
            Op::BuildList { dst, base, len } => {
                record_container(VmContainerMetric::List);
                container_ops::run_build_list(frame_raw, regs, *dst, *base, *len, region_plan, region_allocator_ptr);
                pc += 1;
            }
            Op::BuildMap { dst, base, len } => {
                record_container(VmContainerMetric::Map);
                if let Err(err) =
                    container_ops::run_build_map(frame_raw, regs, *dst, *base, *len, region_plan, region_allocator_ptr)
                {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::ListSlice { dst, src, start } => {
                record_container(VmContainerMetric::List);
                if let Err(err) = container_ops::run_list_slice(
                    frame_raw,
                    regs,
                    *dst,
                    *src,
                    *start,
                    region_plan,
                    region_allocator_ptr,
                ) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::ListPush { list, val } => {
                record_container(VmContainerMetric::List);
                if let Err(err) = container_ops::run_list_push(regs, *list, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::ListPushMove { list, val } => {
                record_container(VmContainerMetric::List);
                if let Err(err) = container_ops::run_list_push_move(regs, *list, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::ListSetI { dst, list, index, val } => {
                record_container(VmContainerMetric::List);
                if let Err(err) = container_ops::run_list_set_i(frame_raw, regs, *dst, *list, *index, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::MapSet { map, key, val } => {
                record_container(VmContainerMetric::Map);
                if let Err(err) = container_ops::run_map_set(regs, *map, *key, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::MapSetInterned(map, kidx, val) => {
                record_container(VmContainerMetric::Map);
                if let Err(err) = container_ops::run_map_set_interned(regs, &f.consts, *map, *kidx, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::MapSetInternedMove(map, kidx, val) => {
                record_container(VmContainerMetric::Map);
                if let Err(err) = container_ops::run_map_set_interned_move(regs, &f.consts, *map, *kidx, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::MapSetMove { map, key, val } => {
                record_container(VmContainerMetric::Map);
                if let Err(err) = container_ops::run_map_set_move(regs, *map, *key, *val) {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                pc += 1;
            }
            Op::ForRangePrep {
                idx,
                limit,
                step,
                inclusive,
                explicit,
            } => {
                let idx_reg = *idx as usize;
                let limit_reg = *limit as usize;
                let step_reg = *step as usize;
                let (i0, ilim) = match (&regs[idx_reg], &regs[limit_reg]) {
                    (Val::Int(a), Val::Int(b)) => (*a, *b),
                    _ => {
                        return frame_return_common(
                            frame_raw,
                            pc,
                            Err(anyhow!(
                                "For-range requires integer bounds, got idx={:?}, limit={:?}",
                                regs[idx_reg],
                                regs[limit_reg]
                            )),
                        )
                        .map(Some);
                    }
                };
                let step_val = if !*explicit {
                    let step_val = if i0 <= ilim { 1 } else { -1 };
                    assign_reg(frame_raw, regs, step_reg, Val::Int(step_val));
                    step_val
                } else {
                    match &regs[step_reg] {
                        Val::Int(0) => {
                            return frame_return_common(frame_raw, pc, Err(anyhow!("For-range step cannot be zero")))
                                .map(Some);
                        }
                        Val::Int(v) => *v,
                        other => {
                            return frame_return_common(
                                frame_raw,
                                pc,
                                Err(anyhow!("For-range step must be Int when explicit, got {:?}", other)),
                            )
                            .map(Some);
                        }
                    }
                };
                if step_val == 0 {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("For-range step cannot be zero"))).map(Some);
                }
                if let Some(slot) = for_range_ic.get_mut(pc + 1) {
                    *slot = Some(ForRangeState::new(i0, ilim, step_val, *inclusive));
                }
                pc += 1;
            }
            Op::ForRangeLoop {
                idx, write_idx, ofs, ..
            }
            | Op::RangeLoopI {
                idx, write_idx, ofs, ..
            } => {
                let idx_reg = *idx as usize;
                if let Some(slot) = for_range_ic.get_mut(pc)
                    && let Some(state) = slot
                {
                    // Inline should_continue — eliminates fn call on hot loop guard
                    let keep_going = if state.positive {
                        if state.inclusive {
                            state.current <= state.limit
                        } else {
                            state.current < state.limit
                        }
                    } else if state.inclusive {
                        state.current >= state.limit
                    } else {
                        state.current > state.limit
                    };
                    if keep_going {
                        if *write_idx {
                            assign_reg(frame_raw, regs, idx_reg, Val::Int(state.current));
                        }
                        state.current += state.step;
                        if let Some(Op::ForRangeStep { back_ofs, .. }) = f.code.get(pc + 1) {
                            pc = (((pc + 1) as isize) + (*back_ofs as isize)) as usize;
                        } else {
                            pc += 1;
                        }
                    } else {
                        // Write final counter value on exit. For while-lowered
                        // loops like `while (i < N) { ...; i += 1; }`, the user expects
                        // i == N after the loop. For-range writes 0..N-1 per iteration, so
                        // on exit we write state.current (== N or limit) to complete the
                        // semantics. For native `for i in 0..N {}`, the loop variable is scoped
                        // so this extra write is harmless.
                        if *write_idx {
                            assign_reg(frame_raw, regs, idx_reg, Val::Int(state.current));
                        }
                        *slot = None;
                        pc = ((pc as isize) + (*ofs as isize)) as usize;
                    }
                } else {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("For-range state missing at pc {}", pc)))
                        .map(Some);
                }
            }
            Op::ForRangeStep { back_ofs, .. } => {
                let guard_pc = ((pc as isize) + (*back_ofs as isize)) as usize;
                if let Some(
                    Op::ForRangeLoop {
                        idx, write_idx, ofs, ..
                    }
                    | Op::RangeLoopI {
                        idx, write_idx, ofs, ..
                    },
                ) = f.code.get(guard_pc)
                {
                    let body_pc = guard_pc + 1;
                    let exit_pc = ((guard_pc as isize) + (*ofs as isize)) as usize;
                    pc = match advance_for_range_tail(
                        frame_raw,
                        regs,
                        for_range_ic,
                        guard_pc,
                        body_pc,
                        exit_pc,
                        *idx,
                        *write_idx,
                    ) {
                        Ok(next_pc) => next_pc,
                        Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
                    };
                } else {
                    pc = guard_pc;
                }
            }
            Op::MakeClosure { dst, proto } => {
                if let Some(value) = closure_ops::run_make_closure_opcode(frame_raw, regs, ctx, &mut pc, f, dst, proto)?
                {
                    return Ok(Some(value));
                }
            }

            Op::Not(dst, src) => {
                match &regs[*src as usize] {
                    Val::Bool(b) => assign_reg(frame_raw, regs, *dst as usize, Val::Bool(!b)),
                    Val::Nil => assign_reg(frame_raw, regs, *dst as usize, Val::Bool(true)),
                    other => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("Invalid operand: !{:?}", other)))
                            .map(Some);
                    }
                }
                pc += 1;
            }
            Op::ToBool(dst, src) => {
                let truthy = !matches!(regs[*src as usize], Val::Nil | Val::Bool(false));
                assign_reg(frame_raw, regs, *dst as usize, Val::Bool(truthy));
                pc += 1;
            }
            Op::Jmp(ofs) => {
                record_branch(false);
                pc = ((pc as isize) + (*ofs as isize)) as usize;
            }
            Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => {
                record_branch(false);
                let cond_falsey = matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if cond_falsey {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::CmpLtImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused CmpLtImm + JmpFalse: if r < imm, fall through; else jump.
                let skip = match &regs[*r as usize] {
                    Val::Int(x) => *x >= (*imm as i64),
                    _ => true, // non-integers always skip the loop
                };
                if skip {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpNilOrFalseJmp { r, ofs } => {
                record_branch(false);
                // Fused: if r is nil or false, jump by ofs, else fall through.
                let is_falsey = matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if is_falsey {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::AddIntImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused: r += imm, then jump by ofs. Common loop tail.
                if let Val::Int(x) = regs[*r as usize] {
                    // Check for overflow and wrap to avoid panics
                    let result = x.wrapping_add(*imm as i64);
                    regs[*r as usize] = Val::Int(result);
                }
                pc = ((pc as isize) + (*ofs as isize)) as usize;
            }
            Op::AddRangeCountImm {
                target,
                idx,
                limit,
                step,
                inclusive,
                explicit,
                imm,
            } => {
                let (start, end) = match (&regs[*idx as usize], &regs[*limit as usize]) {
                    (Val::Int(start), Val::Int(end)) => (*start, *end),
                    _ => {
                        return frame_return_common(
                            frame_raw,
                            pc,
                            Err(anyhow!(
                                "For-range requires integer bounds, got idx={:?}, limit={:?}",
                                regs[*idx as usize],
                                regs[*limit as usize]
                            )),
                        )
                        .map(Some);
                    }
                };
                let step_val = if !*explicit {
                    if start <= end { 1 } else { -1 }
                } else {
                    match &regs[*step as usize] {
                        Val::Int(0) => {
                            return frame_return_common(frame_raw, pc, Err(anyhow!("For-range step cannot be zero")))
                                .map(Some);
                        }
                        Val::Int(value) => *value,
                        other => {
                            return frame_return_common(
                                frame_raw,
                                pc,
                                Err(anyhow!("For-range step must be Int when explicit, got {:?}", other)),
                            )
                            .map(Some);
                        }
                    }
                };
                let count = control_ops::range_iteration_count(start, end, step_val, *inclusive);
                if count > 0 {
                    let target_idx = *target as usize;
                    match &regs[target_idx] {
                        Val::Int(value) => {
                            let delta = count.wrapping_mul(*imm as i64);
                            assign_reg(frame_raw, regs, target_idx, Val::Int((*value).wrapping_add(delta)));
                        }
                        other => {
                            return frame_return_common(
                                frame_raw,
                                pc,
                                Err(anyhow!("AddRangeCountImm target must be Int, got {:?}", other)),
                            )
                            .map(Some);
                        }
                    }
                }
                pc += 1;
            }
            Op::CmpLeImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused CmpLeImm + JmpFalse: if r <= imm, fall through; else jump.
                let skip = match &regs[*r as usize] {
                    Val::Int(x) => *x > (*imm as i64),
                    _ => true,
                };
                if skip {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::CmpEqImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused CmpEqImm + JmpFalse: if r == imm, fall through; else jump.
                let skip = match &regs[*r as usize] {
                    Val::Int(x) => *x != (*imm as i64),
                    _ => true,
                };
                if skip {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::CmpGtImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused CmpGtImm + JmpFalse: if r > imm, fall through; else jump.
                let skip = match &regs[*r as usize] {
                    Val::Int(x) => *x <= (*imm as i64),
                    _ => true,
                };
                if skip {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::CmpGeImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused CmpGeImm + JmpFalse: if r >= imm, fall through; else jump.
                let skip = match &regs[*r as usize] {
                    Val::Int(x) => *x < (*imm as i64),
                    _ => true,
                };
                if skip {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::CmpNeImmJmp { r, imm, ofs } => {
                record_branch(true);
                // Fused CmpNeImm + JmpFalse: if r == imm, jump; else fall through.
                // Common for while (x != N) loop exit checks.
                let skip = match &regs[*r as usize] {
                    Val::Int(x) => *x == (*imm as i64),
                    _ => true,
                };
                if skip {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpFalseSet { r, dst, ofs } => {
                record_branch(false);
                let cond_falsey = matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if cond_falsey {
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(false));
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpIfNil(r, ofs) => {
                record_branch(false);
                if matches!(regs[*r as usize], Val::Nil) {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpIfNotNil(r, ofs) => {
                record_branch(false);
                if !matches!(regs[*r as usize], Val::Nil) {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::NullishPick { l, dst, ofs } => {
                record_branch(false);
                if !matches!(regs[*l as usize], Val::Nil) {
                    assign_reg(frame_raw, regs, *dst as usize, regs[*l as usize].clone());
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpTrueSet { r, dst, ofs } => {
                record_branch(false);
                let cond_truthy = !matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if cond_truthy {
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(true));
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::Call {
                f: rf,
                base,
                argc,
                retc,
            } => {
                record_call(VmCallMetric::Generic);
                if let Some(value) = call_ops::run_call_opcode(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
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
            Op::CallNativeFast {
                f: rf,
                base,
                argc,
                retc,
            } => {
                record_call(VmCallMetric::Native);
                if let Some(value) = call_ops::run_call_native_fast_opcode(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
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
            Op::CallMethod0 { dst, receiver, method } => {
                record_call(VmCallMetric::Method);
                method_ops::run_call_method0(frame_raw, regs, ctx, f, *dst, *receiver, *method)?;
                pc += 1;
            }
            Op::CallGlobalMethod0 { dst, receiver, method } => {
                record_call(VmCallMetric::Method);
                method_ops::run_call_global_method0(frame_raw, regs, ctx, f, global_ic, pc, *dst, *receiver, *method)?;
                pc += 1;
            }
            Op::CallExact {
                f: rf,
                base,
                argc,
                retc,
            } => {
                record_call(VmCallMetric::Exact);
                if let Some(value) = call_ops::run_call_exact_opcode(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
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
                record_call(VmCallMetric::Closure);
                if let Some(value) = call_ops::run_call_closure_exact_opcode(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
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
                record_call(VmCallMetric::Named);
                if let Some(value) = call_ops::run_call_named_opcode(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
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
                record_call(VmCallMetric::Named);
                if let Some(value) = call_ops::run_call_named_opcode(
                    frame_raw,
                    regs,
                    ctx,
                    call_ic,
                    &mut pc,
                    frame_base,
                    region_allocator_ptr,
                    self_ptr,
                    rf,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
                )? {
                    return Ok(Some(value));
                }
            }

            Op::Ret { base, retc } => {
                let retc = *retc as usize;
                let base_idx = *base as usize;
                let ret_val = if retc > 0 {
                    std::mem::replace(&mut regs[base_idx], Val::Nil)
                } else {
                    Val::Nil
                };
                return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr).map(Some);
            }
            Op::Break(ofs) => {
                // Break: jump to loop end
                pc = ((pc as isize) + (*ofs as isize)) as usize;
            }
            Op::Continue(ofs) => {
                // Continue: jump to loop head
                pc = ((pc as isize) + (*ofs as isize)) as usize;
            }
        }
    }
    *pc_ref = pc;
    Ok(None)
}
