//! Packed 32-bit bytecode fast-path interpreter.
//!
//! Switch-free BC32 dispatch with packed hot slots, sentinel skips, and range tail fusion.

use arcstr::ArcStr;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::op::BinOp;
use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use crate::val::{ClosureCapture, Type, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::{AllocationRegion, RegionAllocator};
use crate::vm::bc32::{self, Bc32Decoded, Tag};
use crate::vm::bytecode::{CaptureSpec, Function, Op, rk_is_const, rk_make_const};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    AccessIc, CallIc, CallReturnLayout, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, PackedArithOp,
    PackedCmpImmOp, PackedCmpOp, PackedHotEntry, PackedHotKind, PackedHotSlot, PackedRangeTail, TinyCallPlan, VmCaches,
};
use crate::vm::vm::frame::{CallArgs, CallFrameMeta, CallFrameStackGuard, FrameState, RegisterSpan, RegisterWindowRef};
use crate::vm::{
    record_quickening_build_attempt, record_quickening_build_success, record_quickening_hit, record_quickening_miss,
    record_quickening_sentinel_skip,
};

use super::helpers::{
    advance_for_range_tail, assign_reg, fetch_for_range_state, frame_return_common, handle_return_common,
    insert_map_entry, push_list_entry,
};
use super::invoke::{
    ArgWindow, NativeCallable, ReturnSlot, clear_pending_resume_pc, invoke_native_callable_with_ic,
    invoke_rust_fast_function_named, invoke_rust_function_named_fast, invoke_vm_closure_fast, take_pending_resume_pc,
};
use super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, float_binop, int_binop, int_binop_imm, rk_read};
use super::method_ops;
use super::plan::build_named_call_plan;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum PackedHotCallKind {
    Generic,
    ClosureExact,
    Exact,
}

fn hot_call_operands(kind: &PackedHotKind) -> Option<(u16, u16, u8, u8, PackedHotCallKind)> {
    match kind {
        PackedHotKind::Call { f, base, argc, retc } => Some((*f, *base, *argc, *retc, PackedHotCallKind::Generic)),
        PackedHotKind::CallClosureExact { f, base, argc, retc } => {
            Some((*f, *base, *argc, *retc, PackedHotCallKind::ClosureExact))
        }
        PackedHotKind::CallExact { f, base, argc, retc } => Some((*f, *base, *argc, *retc, PackedHotCallKind::Exact)),
        _ => None,
    }
}

fn validate_hot_exact_call(regs: &[Val], f: u16, argc: u8, kind: PackedHotCallKind) -> Result<()> {
    match kind {
        PackedHotCallKind::Generic => Ok(()),
        PackedHotCallKind::ClosureExact => match &regs[f as usize] {
            Val::Closure(closure) if closure.named_params.is_empty() && closure.params.len() == argc as usize => Ok(()),
            Val::Closure(closure) if !closure.named_params.is_empty() => {
                Err(anyhow!("exact closure call does not accept named fallback"))
            }
            Val::Closure(closure) => Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                closure.params.len(),
                argc
            )),
            other => Err(anyhow!("{} is not an exact closure", other.type_name())),
        },
        PackedHotCallKind::Exact => match &regs[f as usize] {
            Val::Closure(closure) if closure.named_params.is_empty() && closure.params.len() == argc as usize => Ok(()),
            Val::RustFunction(_) | Val::RustFastFunction(_) => Ok(()),
            Val::Closure(closure) if !closure.named_params.is_empty() => {
                Err(anyhow!("exact call does not accept named fallback"))
            }
            Val::Closure(closure) => Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                closure.params.len(),
                argc
            )),
            other => Err(anyhow!("{} is not an exact positional callable", other.type_name())),
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_packed_code(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    caches: &mut VmCaches<'_>,
    func: &Function,
    pc_ref: &mut usize,
    frame_base: usize,
    code32: &[u32],
    decoded: Option<&Bc32Decoded>,
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
    let packed_hot = &mut *caches.packed_hot;
    #[cfg(debug_assertions)]
    let _stats_guard = PackedHotStatsGuard::new();
    let mut pc = *pc_ref;
    let f = func;
    if access_ic.len() < f.code.len() {
        access_ic.resize(f.code.len(), None);
    }
    // Persist instruction-site caches across executions; only grow when needed.
    if access_ic.len() < code32.len() {
        access_ic.resize(code32.len(), None);
    }
    if index_ic.len() < code32.len() {
        index_ic.resize(code32.len(), None);
    }
    if global_ic.len() < code32.len() {
        global_ic.resize(code32.len(), None);
    }
    if call_ic.len() < code32.len() {
        call_ic.resize(code32.len(), None);
    }
    if for_range_ic.len() < f.code.len() {
        for_range_ic.resize(f.code.len(), None);
    }
    if for_range_ic.len() < code32.len() {
        for_range_ic.resize(code32.len(), None);
    }
    if packed_hot.len() < code32.len() {
        packed_hot.resize(code32.len(), None);
    }

    while pc < code32.len() {
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
                        record_quickening_hit();
                        record_hot_hit();
                        if let PackedHotKind::Ret { base, retc } = &slot.kind {
                            let retc = *retc as usize;
                            let base_idx = *base as usize;
                            let ret_val = if retc > 0 {
                                std::mem::replace(&mut regs[base_idx], Val::Nil)
                            } else {
                                Val::Nil
                            };
                            return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr)
                                .map(Some);
                        }
                        if let Some((f, base, argc, retc, call_kind)) = hot_call_operands(&slot.kind) {
                            validate_hot_exact_call(regs, f, argc, call_kind)?;
                            if let Some(value) = call::run_call_packed(
                                frame_raw,
                                regs,
                                ctx,
                                call_ic,
                                &mut pc,
                                slot.next_pc,
                                frame_base,
                                region_allocator_ptr,
                                self_ptr,
                                f,
                                base,
                                argc,
                                retc,
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
                            access_ic,
                            index_ic,
                            global_ic,
                            call_ic,
                            for_range_ic,
                            pc,
                            frame_base,
                            region_plan,
                            region_allocator_ptr,
                        )?;
                        pc = override_pc.unwrap_or(slot.next_pc);
                        continue;
                    }
                }
                PackedHotEntry::Miss(last_word) => {
                    if *last_word == word {
                        record_quickening_sentinel_skip();
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
                        *entry = None;
                    }
                    PackedHotEntry::Miss(last_word) if *last_word != word => {
                        *entry = None;
                    }
                    _ => {}
                }
            }
            record_build_attempt();
            record_quickening_build_attempt();
            if let Some(entry) = build_hot_slot(code32, pc, word, raw_tag) {
                record_quickening_build_success();
                record_build_success();
                let next_pc = entry.next_pc;
                if let PackedHotKind::Ret { base, retc } = &entry.kind {
                    let retc = *retc as usize;
                    let base_idx = *base as usize;
                    let ret_val = if retc > 0 {
                        std::mem::replace(&mut regs[base_idx], Val::Nil)
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
                    validate_hot_exact_call(regs, f_reg, argc_count, call_kind)?;
                    if packed_hot.len() <= pc {
                        packed_hot.resize(pc + 1, None);
                    }
                    packed_hot[pc] = Some(PackedHotEntry::Slot(entry));
                    if let Some(value) = call::run_call_packed(
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
                    access_ic,
                    index_ic,
                    global_ic,
                    call_ic,
                    for_range_ic,
                    pc,
                    frame_base,
                    region_plan,
                    region_allocator_ptr,
                )?;
                if packed_hot.len() <= pc {
                    packed_hot.resize(pc + 1, None);
                }
                packed_hot[pc] = Some(PackedHotEntry::Slot(entry));
                pc = override_pc.unwrap_or(next_pc);
                continue;
            } else {
                record_quickening_miss();
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
            Op::LoadK(dst, k) => {
                assign_reg(frame_raw, regs, dst as usize, f.consts[k as usize].clone());
                pc = next_pc_default;
            }
            Op::Move(dst, src) => {
                assign_reg(frame_raw, regs, dst as usize, regs[src as usize].clone());
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
                )? {
                    return Ok(Some(value));
                }
            }
            Op::CallMethod0 { dst, receiver, method } => {
                method_ops::run_call_method0(frame_raw, regs, ctx, f, dst, receiver, method)?;
                pc = next_pc_default;
            }
            Op::CallGlobalMethod0 { dst, receiver, method } => {
                method_ops::run_call_global_method0(frame_raw, regs, ctx, f, global_ic, pc, dst, receiver, method)?;
                pc = next_pc_default;
            }
            Op::CallExact {
                f: rf,
                base,
                argc,
                retc,
            } => {
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
                                scratch.extend(list[s..].iter().cloned());
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
                let pushed_val = regs[val as usize].clone();
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
    *pc_ref = pc;
    Ok(None)
}
