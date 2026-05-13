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
//! combine multiple operations into a single dispatch.

use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::op::BinOp;
use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use crate::val::{ClosureCapture, ClosureInit, ClosureValue, Type, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::{AllocationRegion, RegionAllocator};
use crate::vm::bytecode::{CaptureSpec, Function, Op};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    AccessIc, CallIc, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, TinyCallPlan, VmCaches,
};
use crate::vm::vm::frame::{CallArgs, CallFrameMeta, CallFrameStackGuard, FrameState, RegisterSpan, RegisterWindowRef};

use super::helpers::{assign_reg, frame_return_common, handle_return_common};
use super::invoke::{invoke_rust_function, invoke_rust_function_named};
use super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, float_binop, int_binop, int_binop_imm, rk_read};
use super::plan::build_named_call_plan;

fn range_iteration_count(start: i64, limit: i64, step: i64, inclusive: bool) -> i64 {
    if step > 0 {
        if inclusive {
            if start > limit { 0 } else { ((limit - start) / step) + 1 }
        } else if start >= limit {
            0
        } else {
            ((limit - start - 1) / step) + 1
        }
    } else {
        let stride = -step;
        if inclusive {
            if start < limit {
                0
            } else {
                ((start - limit) / stride) + 1
            }
        } else if start <= limit {
            0
        } else {
            ((start - limit - 1) / stride) + 1
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_opcode_code(
    frame_raw: *mut FrameState<'_>,
    regs: &mut Vec<Val>,
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
    while pc < f.code.len() {
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
                let s = Val::to_str_value(&regs[*src as usize]);
                assign_reg(frame_raw, regs, *dst as usize, s);
                pc += 1;
            }
            Op::Add(dst, a, b) => {
                let a_val = rk_read(regs, &f.consts, *a);
                let b_val = rk_read(regs, &f.consts, *b);
                if let Val::Str(a_str) = &a_val
                    && let Some(out) = Val::concat_str_add_rhs(a_str, &b_val)
                {
                    assign_reg(frame_raw, regs, *dst as usize, out);
                } else if let Val::Str(b_str) = &b_val
                    && let Some(out) = Val::concat_add_lhs_str(&a_val, b_str)
                {
                    assign_reg(frame_raw, regs, *dst as usize, out);
                } else if !Vm::arith2_try_numeric(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *b,
                    "add",
                    |x, y| x + y,
                    |x, y| x + y,
                ) {
                    // Fallback to high-level semantics (strings, lists, maps under features)
                    let out = BinOp::Add.eval_vals(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, out);
                }
                pc += 1;
            }
            Op::Sub(dst, a, b) => {
                if !Vm::arith2_try_numeric(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *b,
                    "sub",
                    |x, y| x - y,
                    |x, y| x - y,
                ) {
                    let out = BinOp::Sub.eval_vals(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, out);
                }
                pc += 1;
            }
            Op::Mul(dst, a, b) => {
                if !Vm::arith2_try_numeric(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *b,
                    "mul",
                    |x, y| x * y,
                    |x, y| x * y,
                ) {
                    let out = BinOp::Mul.eval_vals(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, out);
                }
                pc += 1;
            }
            Op::Div(dst, a, b) => {
                if !Vm::arith2_try_numeric(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *b,
                    "div",
                    |x, y| x / y,
                    |x, y| x / y,
                ) {
                    let out = BinOp::Div.eval_vals(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, out);
                }
                pc += 1;
            }
            Op::Mod(dst, a, b) => {
                match (rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b)) {
                    (Val::Int(x), Val::Int(y)) => assign_reg(frame_raw, regs, *dst as usize, Val::Int(x % y)),
                    _ => {
                        let lhs = rk_read(regs, &f.consts, *a);
                        let rhs = rk_read(regs, &f.consts, *b);
                        tracing::debug!(
                            target: "lk::vm::slowpath",
                            op = "mod",
                            lhs = lhs.type_name(),
                            rhs = rhs.type_name(),
                            "mod fallback"
                        );
                        let out = BinOp::Mod.eval_vals(lhs, rhs)?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                }
                pc += 1;
            }
            Op::AddInt(dst, a, b) => {
                let a_val = &regs[*a as usize];
                let b_val = &regs[*b as usize];
                if let (Val::Str(a_str), Val::Str(b_str)) = (a_val, b_val) {
                    assign_reg(frame_raw, regs, *dst as usize, Val::concat_strings(a_str, b_str));
                } else {
                    int_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x + y, BinOp::Add)?;
                }
                pc += 1;
            }
            Op::AddFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x + y, BinOp::Add)?;
                pc += 1;
            }
            Op::AddIntImm(dst, a, imm) => {
                let src_idx = *a as usize;
                let dst_idx = *dst as usize;
                if let Val::Int(x) = regs[src_idx] {
                    assign_reg(frame_raw, regs, dst_idx, Val::Int(x + *imm as i64));
                } else {
                    int_binop_imm(frame_raw, regs, &f.consts, *dst, *a, *imm, |x, y| x + y, BinOp::Add)?;
                }
                pc += 1;
            }
            Op::SubInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x - y, BinOp::Sub)?;
                pc += 1;
            }
            Op::SubFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x - y, BinOp::Sub)?;
                pc += 1;
            }
            Op::CmpEqImm(dst, a, imm) => {
                cmp_eq_imm(frame_raw, regs, &f.consts, *dst, *a, *imm, BinOp::Eq)?;
                pc += 1;
            }
            Op::CmpNeImm(dst, a, imm) => {
                cmp_ne_imm(frame_raw, regs, &f.consts, *dst, *a, *imm, BinOp::Ne)?;
                pc += 1;
            }
            Op::CmpLtImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *imm,
                    |x, y| x < y,
                    |x, y| x < y,
                    BinOp::Lt,
                )?;
                pc += 1;
            }
            Op::CmpLeImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *imm,
                    |x, y| x <= y,
                    |x, y| x <= y,
                    BinOp::Le,
                )?;
                pc += 1;
            }
            Op::CmpGtImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *imm,
                    |x, y| x > y,
                    |x, y| x > y,
                    BinOp::Gt,
                )?;
                pc += 1;
            }
            Op::CmpGeImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    *dst,
                    *a,
                    *imm,
                    |x, y| x >= y,
                    |x, y| x >= y,
                    BinOp::Ge,
                )?;
                pc += 1;
            }
            Op::MulInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x * y, BinOp::Mul)?;
                pc += 1;
            }
            Op::MulFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x * y, BinOp::Mul)?;
                pc += 1;
            }
            Op::DivFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x / y, BinOp::Div)?;
                pc += 1;
            }
            Op::ModInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x % y, BinOp::Mod)?;
                pc += 1;
            }
            Op::ModFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x % y, BinOp::Mod)?;
                pc += 1;
            }
            Op::CmpEq(dst, a, b) => {
                let r = rk_read(regs, &f.consts, *a) == rk_read(regs, &f.consts, *b);
                assign_reg(frame_raw, regs, *dst as usize, Val::Bool(r));
                pc += 1;
            }
            Op::CmpNe(dst, a, b) => {
                let r = rk_read(regs, &f.consts, *a) != rk_read(regs, &f.consts, *b);
                assign_reg(frame_raw, regs, *dst as usize, Val::Bool(r));
                pc += 1;
            }
            Op::CmpLt(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x < y, |x, y| x < y) {
                    let res = BinOp::Lt.cmp(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                }
                pc += 1;
            }
            Op::CmpLe(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x <= y, |x, y| x <= y) {
                    let res = BinOp::Le.cmp(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                }
                pc += 1;
            }
            Op::CmpGt(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x > y, |x, y| x > y) {
                    let res = BinOp::Gt.cmp(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                }
                pc += 1;
            }
            Op::CmpGe(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, *dst, *a, *b, |x, y| x >= y, |x, y| x >= y) {
                    let res = BinOp::Ge.cmp(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                }
                pc += 1;
            }
            Op::In(dst, a, b) => {
                let res = BinOp::In.cmp(rk_read(regs, &f.consts, *a), rk_read(regs, &f.consts, *b))?;
                assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
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
                let name_val = &f.consts[*name_k as usize];
                let mut out = Val::Nil;
                if let Val::Str(s) = name_val {
                    let key_ptr = s.as_ref().as_ptr() as usize;
                    let cur_gen = ctx.generation();
                    let local_shadowed = ctx.is_local_name(s.as_ref());
                    if !local_shadowed
                        && let Some(GlobalEntry(ptr, v, generation)) = &global_ic[pc]
                        && *ptr == key_ptr
                        && *generation == cur_gen
                    {
                        out = v.clone();
                    } else if !local_shadowed && let Some(v) = ctx.get(s.as_ref()) {
                        out = v.clone();
                        global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                    }
                    if matches!(out, Val::Nil)
                        && let Some(v) = ctx.get_value(s.as_ref())
                    {
                        out = v;
                        if !local_shadowed {
                            global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                        }
                    }
                } else {
                    let fallback_name = format!("{}", name_val);
                    if let Some(v) = ctx.get(fallback_name.as_str()) {
                        out = v.clone();
                    }
                }
                assign_reg(frame_raw, regs, *dst as usize, out);
                pc += 1;
            }
            Op::DefineGlobal(name_k, src) => {
                let name_val = &f.consts[*name_k as usize];
                if let Val::Str(s) = name_val {
                    ctx.set(s.as_ref().to_owned(), regs[*src as usize].clone());
                }
                pc += 1;
            }
            Op::LoadCapture { dst, idx } => {
                let capture_idx = *idx as usize;
                let mut captured = frame_captures
                    .as_ref()
                    .and_then(|caps| caps.value_at(capture_idx).cloned())
                    .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))?;
                if let Some(specs) = frame_capture_specs
                    && let Some(spec) = specs.get(capture_idx)
                    && let CaptureSpec::Global { name } = spec
                {
                    if let Some(val) = ctx.get(name.as_str()).cloned() {
                        captured = val;
                    } else {
                        captured = ctx.get_value(name.as_str()).unwrap_or(Val::Nil);
                    }
                }
                assign_reg(frame_raw, regs, *dst as usize, captured);
                pc += 1;
            }
            Op::Access(dst, base, field) => {
                // Polymorphic 2-way IC for Map[String] and Object[String]
                let hit_val = match (&regs[*base as usize], &regs[*field as usize]) {
                    (Val::Map(m), Val::Str(s)) => {
                        let mp = Arc::as_ptr(m) as usize;
                        let kp = s.as_ref().as_ptr() as usize;
                        match access_ic[pc].as_mut() {
                            Some(AccessIc::MapStr(slots)) => {
                                Vm::lookup_promote(slots, |e| e.map_ptr == mp && e.key_ptr == kp)
                                    .map(|entry| entry.value.clone())
                            }
                            _ => None,
                        }
                    }
                    (Val::Object(object), Val::Str(s)) => {
                        let fields = &object.fields;
                        let optr = Arc::as_ptr(fields) as usize;
                        match access_ic[pc].as_mut() {
                            Some(AccessIc::ObjectStr(slots)) => {
                                Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == s.as_ref())
                                    .map(|entry| entry.value.clone())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let res = if let Some(v) = hit_val {
                    v
                } else {
                    let v = regs[*base as usize].access(&regs[*field as usize]).unwrap_or(Val::Nil);
                    match (&regs[*base as usize], &regs[*field as usize]) {
                        (Val::Map(m), Val::Str(s)) => {
                            let mp = Arc::as_ptr(m) as usize;
                            let kp = s.as_ref().as_ptr() as usize;
                            Vm::update_map_ic(access_ic.as_mut_slice(), pc, mp, kp, &v);
                        }
                        (Val::Object(object), Val::Str(s)) => {
                            let fields = &object.fields;
                            let optr = Arc::as_ptr(fields) as usize;
                            Vm::update_object_ic(access_ic.as_mut_slice(), pc, optr, s.as_ref(), &v);
                        }
                        _ => {}
                    }
                    v
                };
                assign_reg(frame_raw, regs, *dst as usize, res);
                pc += 1;
            }
            Op::AccessK(dst, base, kidx) => {
                let key = &f.consts[*kidx as usize];
                // Only valid for string constants; otherwise yield Nil
                let res = if let Val::Str(s) = key {
                    let (hit_val, mp, kp, obj_ptr) = match &regs[*base as usize] {
                        Val::Map(m) => {
                            let mp = Arc::as_ptr(m) as usize;
                            let kp = s.as_ref().as_ptr() as usize;
                            let out = match access_ic[pc].as_mut() {
                                Some(AccessIc::MapStr(slots)) => {
                                    Vm::lookup_promote(slots, |e| e.map_ptr == mp && e.key_ptr == kp)
                                        .map(|entry| entry.value.clone())
                                }
                                _ => None,
                            };
                            (out, Some(mp), Some(kp), None)
                        }
                        Val::Object(object) => {
                            let fields = &object.fields;
                            let optr = Arc::as_ptr(fields) as usize;
                            let out = match access_ic[pc].as_mut() {
                                Some(AccessIc::ObjectStr(slots)) => {
                                    Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == s.as_ref())
                                        .map(|entry| entry.value.clone())
                                }
                                _ => None,
                            };
                            (out, None, None, Some(optr))
                        }
                        _ => (None, None, None, None),
                    };
                    if let Some(v) = hit_val {
                        v
                    } else {
                        let v = regs[*base as usize].access(key).unwrap_or(Val::Nil);
                        if let (Some(mp), Some(kp)) = (mp, kp) {
                            Vm::update_map_ic(access_ic.as_mut_slice(), pc, mp, kp, &v);
                        } else if let Some(optr) = obj_ptr {
                            Vm::update_object_ic(access_ic.as_mut_slice(), pc, optr, s.as_ref(), &v);
                        }
                        v
                    }
                } else {
                    Val::Nil
                };
                assign_reg(frame_raw, regs, *dst as usize, res);
                pc += 1;
            }
            Op::Len { dst, src } => {
                let v = &regs[*src as usize];
                let out = match v {
                    Val::List(l) => Val::Int(l.len() as i64),
                    Val::Str(s) => Val::Int(s.len() as i64),
                    Val::Map(m) => Val::Int(m.len() as i64),
                    _ => Val::Int(0),
                };
                assign_reg(frame_raw, regs, *dst as usize, out);
                pc += 1;
            }
            Op::ListFoldAdd { acc, list } => {
                let folded = if let Val::List(items) = &regs[*list as usize] {
                    Some(if let Val::Int(mut total) = regs[*acc as usize] {
                        let mut all_int = true;
                        for item in items.iter() {
                            if let Val::Int(value) = item {
                                total = total.wrapping_add(*value);
                            } else {
                                all_int = false;
                                break;
                            }
                        }
                        if all_int {
                            Val::Int(total)
                        } else {
                            let mut out = regs[*acc as usize].clone();
                            for item in items.iter() {
                                out = BinOp::Add.eval_vals(&out, item)?;
                            }
                            out
                        }
                    } else {
                        let mut out = regs[*acc as usize].clone();
                        for item in items.iter() {
                            out = BinOp::Add.eval_vals(&out, item)?;
                        }
                        out
                    })
                } else {
                    None
                };
                if let Some(out) = folded {
                    assign_reg(frame_raw, regs, *acc as usize, out);
                }
                pc += 1;
            }
            Op::MapValuesFoldAdd { acc, map } => {
                let folded = if let Val::Map(values) = &regs[*map as usize] {
                    Some(if let Val::Int(mut total) = regs[*acc as usize] {
                        let mut all_int = true;
                        for item in values.values() {
                            if let Val::Int(value) = item {
                                total = total.wrapping_add(*value);
                            } else {
                                all_int = false;
                                break;
                            }
                        }
                        if all_int {
                            Val::Int(total)
                        } else {
                            let mut out = regs[*acc as usize].clone();
                            for item in values.values() {
                                out = BinOp::Add.eval_vals(&out, item)?;
                            }
                            out
                        }
                    } else {
                        let mut out = regs[*acc as usize].clone();
                        for item in values.values() {
                            out = BinOp::Add.eval_vals(&out, item)?;
                        }
                        out
                    })
                } else {
                    None
                };
                if let Some(out) = folded {
                    assign_reg(frame_raw, regs, *acc as usize, out);
                }
                pc += 1;
            }
            Op::Index { dst, base, idx } => {
                let res = match (&regs[*base as usize], &regs[*idx as usize]) {
                    (Val::List(l), Val::Int(i)) => {
                        if *i < 0 {
                            Val::Nil
                        } else {
                            let lptr = Arc::as_ptr(l) as *const Val as usize;
                            let hit = match index_ic[pc].as_mut() {
                                Some(IndexIc::List(slots)) => {
                                    Vm::lookup_promote(slots, |e| e.base_ptr == lptr && e.idx == *i)
                                        .map(|entry| entry.value.clone())
                                }
                                _ => None,
                            };
                            if let Some(v) = hit {
                                v
                            } else {
                                let v = l.get(*i as usize).cloned().unwrap_or(Val::Nil);
                                Vm::update_list_ic(index_ic.as_mut_slice(), pc, lptr, *i, &v);
                                v
                            }
                        }
                    }
                    (Val::Str(s), Val::Int(i)) => {
                        if *i < 0 {
                            Val::Nil
                        } else {
                            let idx = *i as usize;
                            let sptr = s.as_ref().as_ptr() as usize;
                            let hit = match index_ic[pc].as_mut() {
                                Some(IndexIc::Str(slots)) => {
                                    Vm::lookup_promote(slots, |e| e.base_ptr == sptr && e.idx == *i)
                                        .map(|entry| entry.value.clone())
                                }
                                _ => None,
                            };
                            if let Some(v) = hit {
                                v
                            } else {
                                let v = if s.is_ascii() {
                                    let bs = s.as_bytes();
                                    if idx < bs.len() {
                                        let ch = bs[idx] as char;
                                        Val::Str(ch.to_string().into())
                                    } else {
                                        Val::Nil
                                    }
                                } else {
                                    s.chars()
                                        .nth(idx)
                                        .map(|c| Val::Str(c.to_string().into()))
                                        .unwrap_or(Val::Nil)
                                };
                                Vm::update_str_ic(index_ic.as_mut_slice(), pc, sptr, *i, &v);
                                v
                            }
                        }
                    }
                    _ => Val::Nil,
                };
                assign_reg(frame_raw, regs, *dst as usize, res);
                pc += 1;
            }
            Op::IndexK(dst, base, kidx) => {
                let key = &f.consts[*kidx as usize];
                let res = if let Val::Int(i) = key {
                    match &regs[*base as usize] {
                        Val::List(l) => {
                            if *i < 0 {
                                Val::Nil
                            } else {
                                l.get(*i as usize).cloned().unwrap_or(Val::Nil)
                            }
                        }
                        Val::Str(s) => {
                            if *i < 0 {
                                Val::Nil
                            } else if s.is_ascii() {
                                let bi = *i as usize;
                                let bs = s.as_bytes();
                                if bi < bs.len() {
                                    let ch = bs[bi] as char;
                                    Val::Str(ch.to_string().into())
                                } else {
                                    Val::Nil
                                }
                            } else {
                                s.chars()
                                    .nth(*i as usize)
                                    .map(|c| Val::Str(c.to_string().into()))
                                    .unwrap_or(Val::Nil)
                            }
                        }
                        _ => Val::Nil,
                    }
                } else {
                    Val::Nil
                };
                assign_reg(frame_raw, regs, *dst as usize, res);
                pc += 1;
            }
            Op::PatternMatch { dst, src, plan } => {
                let plan = &f.pattern_plans[*plan as usize];
                let value = &regs[*src as usize];
                match plan.pattern.matches(value, Some(&*ctx))? {
                    Some(bound) => {
                        for binding in &plan.bindings {
                            if let Some((_, v)) = bound.iter().find(|(name, _)| name == &binding.name) {
                                assign_reg(frame_raw, regs, binding.reg as usize, v.clone());
                            } else {
                                assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                            }
                        }
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(true));
                    }
                    None => {
                        for binding in &plan.bindings {
                            assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                        }
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(false));
                    }
                }
                pc += 1;
            }
            Op::PatternMatchOrFail {
                src,
                plan,
                err_kidx,
                is_const,
            } => {
                let plan = &f.pattern_plans[*plan as usize];
                let value = &regs[*src as usize];
                match plan.pattern.matches(value, Some(&*ctx))? {
                    Some(bound) => {
                        let mut assigned = Vec::with_capacity(plan.bindings.len());
                        for binding in &plan.bindings {
                            if let Some((_, v)) = bound.iter().find(|(name, _)| name == &binding.name) {
                                let cloned = v.clone();
                                assign_reg(frame_raw, regs, binding.reg as usize, cloned.clone());
                                assigned.push((binding.name.clone(), cloned));
                            } else {
                                assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                            }
                        }
                        for (name, val) in assigned {
                            if *is_const {
                                ctx.define_const(name, val);
                            } else {
                                ctx.set(name, val);
                            }
                        }
                    }
                    None => {
                        let msg_val = &f.consts[*err_kidx as usize];
                        let msg = match msg_val {
                            Val::Str(s) => s.as_ref().to_string(),
                            other => other.to_string(),
                        };
                        return frame_return_common(frame_raw, pc, Err(anyhow!(msg))).map(Some);
                    }
                }
                pc += 1;
            }
            Op::Raise { err_kidx } => {
                let msg_val = &f.consts[*err_kidx as usize];
                let msg = match msg_val {
                    Val::Str(s) => s.as_ref().to_string(),
                    other => other.to_string(),
                };
                return frame_return_common(frame_raw, pc, Err(anyhow!(msg))).map(Some);
            }
            Op::ToIter { dst, src } => {
                let use_thread_local = region_plan
                    .as_ref()
                    .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                    .unwrap_or(false);
                let out = match &regs[*src as usize] {
                    Val::List(_) | Val::Str(_) => regs[*src as usize].clone(),
                    Val::Map(m) => {
                        let mut keys: Vec<&str> = m.keys().map(|k| k.as_ref()).collect();
                        keys.sort();
                        if use_thread_local && !keys.is_empty() {
                            let allocator = unsafe { &*region_allocator_ptr };
                            allocator.with_val_buffer(keys.len(), |scratch| {
                                for key in keys.iter() {
                                    if let Some(v) = m.get(*key) {
                                        let pair =
                                            Val::List(vec![Val::Str((*key).to_string().into()), v.clone()].into());
                                        scratch.push(pair);
                                    }
                                }
                                let data = scratch.split_off(0);
                                Val::List(data.into())
                            })
                        } else {
                            let mut pairs = Vec::with_capacity(keys.len());
                            for key in keys {
                                if let Some(v) = m.get(key) {
                                    let pair = Val::List(vec![Val::Str(key.to_string().into()), v.clone()].into());
                                    pairs.push(pair);
                                }
                            }
                            Val::List(pairs.into())
                        }
                    }
                    _ => Val::List(Vec::<Val>::new().into()),
                };
                assign_reg(frame_raw, regs, *dst as usize, out);
                pc += 1;
            }
            Op::BuildList { dst, base, len } => {
                let start = *base as usize;
                let n = *len as usize;
                let use_thread_local = region_plan
                    .as_ref()
                    .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                    .unwrap_or(false);
                if use_thread_local {
                    let allocator = unsafe { &*region_allocator_ptr };
                    let list_val = allocator.with_val_buffer(n, |scratch| {
                        scratch.extend((0..n).map(|i| regs[start + i].clone()));
                        let data = scratch.split_off(0);
                        Val::List(data.into())
                    });
                    assign_reg(frame_raw, regs, *dst as usize, list_val);
                } else {
                    let mut v = Vec::with_capacity(n);
                    for i in 0..n {
                        v.push(regs[start + i].clone());
                    }
                    assign_reg(frame_raw, regs, *dst as usize, Val::List(v.into()));
                }
                pc += 1;
            }
            Op::BuildMap { dst, base, len } => {
                let start = *base as usize;
                let n = *len as usize;
                let use_thread_local = region_plan
                    .as_ref()
                    .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                    .unwrap_or(false);
                if use_thread_local {
                    let allocator = unsafe { &*region_allocator_ptr };
                    let map_val = allocator.with_map_entries(n, |entries| {
                        for i in 0..n {
                            let key_val = &regs[start + 2 * i];
                            let value = regs[start + 2 * i + 1].clone();
                            let key_arc: Arc<str> = match key_val {
                                Val::Str(s) => s.clone(),
                                Val::Int(i) => Arc::from(i.to_string()),
                                Val::Float(f) => Arc::from(f.to_string()),
                                Val::Bool(b) => Arc::from(b.to_string()),
                                _ => {
                                    return Err(anyhow!("Map key must be a primitive type, got: {:?}", key_val));
                                }
                            };
                            entries.push((key_arc, value));
                        }
                        let mut map = fast_hash_map_with_capacity(entries.len());
                        for (k, v) in entries.drain(..) {
                            map.insert(k, v);
                        }
                        Ok(Val::Map(Arc::new(map)))
                    });
                    match map_val {
                        Ok(val) => assign_reg(frame_raw, regs, *dst as usize, val),
                        Err(err) => {
                            return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                        }
                    }
                } else {
                    let mut map: FastHashMap<Arc<str>, Val> = fast_hash_map_with_capacity(n);
                    for i in 0..n {
                        let k = &regs[start + 2 * i];
                        let v = regs[start + 2 * i + 1].clone();
                        let key_arc: Arc<str> = match k {
                            Val::Str(s) => s.clone(),
                            Val::Int(i) => Arc::from(i.to_string()),
                            Val::Float(f) => Arc::from(f.to_string()),
                            Val::Bool(b) => Arc::from(b.to_string()),
                            _ => {
                                return frame_return_common(
                                    frame_raw,
                                    pc,
                                    Err(anyhow!("Map key must be a primitive type, got: {:?}", k)),
                                )
                                .map(Some);
                            }
                        };
                        map.insert(key_arc, v);
                    }
                    assign_reg(frame_raw, regs, *dst as usize, Val::Map(Arc::new(map)));
                }
                pc += 1;
            }
            Op::ListSlice { dst, src, start } => {
                let (list, start_idx) = match (&regs[*src as usize], &regs[*start as usize]) {
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
                    assign_reg(frame_raw, regs, *dst as usize, Val::List(list.clone()));
                } else {
                    let s = start_idx as usize;
                    if s >= list.len() {
                        assign_reg(frame_raw, regs, *dst as usize, Val::List(Vec::<Val>::new().into()));
                    } else {
                        let use_thread_local = region_plan
                            .as_ref()
                            .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                            .unwrap_or(false);
                        if use_thread_local {
                            let allocator = unsafe { &*region_allocator_ptr };
                            let slice_val = allocator.with_val_buffer(list.len() - s, |scratch| {
                                scratch.extend(list[s..].iter().cloned());
                                let data = scratch.split_off(0);
                                Val::List(data.into())
                            });
                            assign_reg(frame_raw, regs, *dst as usize, slice_val);
                        } else {
                            assign_reg(frame_raw, regs, *dst as usize, Val::List((list[s..]).to_vec().into()));
                        }
                    }
                }
                pc += 1;
            }
            Op::ListPush { list, val } => {
                let pushed_val = regs[*val as usize].clone();
                match &mut regs[*list as usize] {
                    Val::List(arc) => {
                        if let Some(items) = Arc::get_mut(arc) {
                            items.push(pushed_val);
                        } else {
                            Arc::make_mut(arc).push(pushed_val);
                        }
                    }
                    _ => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("ListPush target is not a List")))
                            .map(Some);
                    }
                }
                pc += 1;
            }
            Op::MapSet { map, key, val } => {
                let key_arc = match &regs[*key as usize] {
                    Val::Str(s) => s.clone(),
                    _ => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet key must be a String")))
                            .map(Some);
                    }
                };
                let pushed_val = regs[*val as usize].clone();
                match &mut regs[*map as usize] {
                    Val::Map(arc) => {
                        if let Some(map) = Arc::get_mut(arc) {
                            map.insert(key_arc, pushed_val);
                        } else {
                            Arc::make_mut(arc).insert(key_arc, pushed_val);
                        }
                    }
                    _ => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet target is not a Map")))
                            .map(Some);
                    }
                }
                pc += 1;
            }
            Op::MapSetMove { map, key, val } => {
                let map_idx = *map as usize;
                let key_idx = *key as usize;
                let val_idx = *val as usize;
                if map_idx == key_idx || map_idx == val_idx || key_idx == val_idx {
                    let key_arc = match &regs[key_idx] {
                        Val::Str(s) => s.clone(),
                        _ => {
                            return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet key must be a String")))
                                .map(Some);
                        }
                    };
                    let pushed_val = regs[val_idx].clone();
                    match &mut regs[map_idx] {
                        Val::Map(arc) => {
                            if let Some(map) = Arc::get_mut(arc) {
                                map.insert(key_arc, pushed_val);
                            } else {
                                Arc::make_mut(arc).insert(key_arc, pushed_val);
                            }
                        }
                        _ => {
                            return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet target is not a Map")))
                                .map(Some);
                        }
                    }
                    pc += 1;
                    continue;
                }
                if !matches!(regs[map_idx], Val::Map(_)) {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet target is not a Map"))).map(Some);
                }
                let key_val = std::mem::replace(&mut regs[key_idx], Val::Nil);
                let key_arc = match key_val {
                    Val::Str(s) => s,
                    other => {
                        regs[key_idx] = other;
                        return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet key must be a String")))
                            .map(Some);
                    }
                };
                let pushed_val = std::mem::replace(&mut regs[val_idx], Val::Nil);
                match &mut regs[map_idx] {
                    Val::Map(arc) => {
                        if let Some(map) = Arc::get_mut(arc) {
                            map.insert(key_arc, pushed_val);
                        } else {
                            Arc::make_mut(arc).insert(key_arc, pushed_val);
                        }
                    }
                    _ => unreachable!("MapSet target was checked before moving key/value"),
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
            } => {
                let idx_reg = *idx as usize;
                let slot = unsafe { for_range_ic.get_unchecked_mut(pc) };
                if let Some(state) = slot {
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
                        pc += 1;
                    } else {
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
                // Safety: for_range_ic is pre-sized, guard_pc is a valid code position.
                // ForRangeLoop already pre-increments state.current, so ForRangeStep only jumps.
                pc = guard_pc;
            }
            Op::MakeClosure { dst, proto } => {
                let p = f
                    .protos
                    .get(*proto as usize)
                    .ok_or_else(|| anyhow!("closure proto out of range"))?;
                if p.self_name.is_none() && p.captures.is_empty() {
                    let clo = p
                        .empty_closure
                        .get_or_init(|| {
                            let clo = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
                                params: Arc::clone(&p.params),
                                named_params: Arc::clone(&p.named_params),
                                body: Arc::clone(&p.body),
                                env: Arc::clone(&p.empty_env),
                                upvalues: Arc::clone(&p.empty_upvalues),
                                captures: Arc::clone(&p.empty_captures),
                                capture_specs: Arc::clone(&p.captures),
                                default_funcs: Arc::clone(&p.default_funcs),
                                code: Arc::clone(&p.code),
                                debug_name: None,
                                debug_location: None,
                            })));
                            if p.func.is_none()
                                && p.code.get().is_none()
                                && let Val::Closure(closure_arc) = &clo
                            {
                                let c = Compiler::new();
                                let compiled = c.compile_function_with_captures(
                                    p.params.as_ref(),
                                    p.named_params.as_ref(),
                                    p.body.as_ref(),
                                    p.captures.as_ref(),
                                );
                                let _ = closure_arc.code.set(Arc::new(compiled));
                            }
                            clo
                        })
                        .clone();
                    assign_reg(frame_raw, regs, *dst as usize, clo);
                    pc += 1;
                    continue;
                }
                let captured_env = if p.self_name.is_some() {
                    Arc::new(ctx.snapshot())
                } else {
                    Arc::clone(&p.empty_env)
                };

                let captures = if p.captures.is_empty() {
                    Arc::clone(&p.empty_captures)
                } else if let [spec] = p.captures.as_ref().as_slice() {
                    let value = match spec {
                        CaptureSpec::Register { src, .. } => {
                            let idx = frame_base + (*src as usize);
                            regs.get(idx).cloned().unwrap_or(Val::Nil)
                        }
                        CaptureSpec::Const { kidx, .. } => f.consts.get(*kidx as usize).cloned().unwrap_or(Val::Nil),
                        CaptureSpec::Global { name } => ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil),
                    };
                    ClosureCapture::from_shared_names_one(Arc::clone(&p.capture_names), value)
                } else {
                    let mut values: Vec<Val> = Vec::with_capacity(p.captures.len());
                    for spec in p.captures.iter() {
                        match spec {
                            CaptureSpec::Register { src, .. } => {
                                let idx = frame_base + (*src as usize);
                                let val = regs.get(idx).cloned().unwrap_or(Val::Nil);
                                values.push(val);
                            }
                            CaptureSpec::Const { kidx, .. } => {
                                let val = f.consts.get(*kidx as usize).cloned().unwrap_or(Val::Nil);
                                values.push(val);
                            }
                            CaptureSpec::Global { name } => {
                                let val = ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil);
                                values.push(val);
                            }
                        }
                    }
                    ClosureCapture::from_shared_names(Arc::clone(&p.capture_names), values)
                };

                let mut clo = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
                    params: Arc::clone(&p.params),
                    named_params: Arc::clone(&p.named_params),
                    body: Arc::clone(&p.body),
                    env: captured_env,
                    upvalues: Arc::clone(&p.empty_upvalues),
                    captures,
                    capture_specs: Arc::clone(&p.captures),
                    default_funcs: Arc::clone(&p.default_funcs),
                    code: Arc::clone(&p.code),
                    debug_name: p.self_name.clone(),
                    debug_location: None,
                })));
                if let (Some(name), Val::Closure(closure_arc)) = (&p.self_name, &mut clo)
                    && let Some(closure) = Arc::get_mut(closure_arc)
                    && let Some(env_mut) = Arc::get_mut(&mut closure.env)
                {
                    let env_ptr: *mut VmContext = env_mut;
                    let clone_for_env = clo.clone();
                    unsafe {
                        (*env_ptr).define(name.clone(), clone_for_env);
                    }
                }
                if p.func.is_none()
                    && p.code.get().is_none()
                    && let Val::Closure(closure_arc) = &clo
                {
                    // Eagerly pre-compile closures to eliminate OnceCell overhead from hot calls
                    let c = Compiler::new();
                    let compiled = c.compile_function_with_captures(
                        p.params.as_ref(),
                        p.named_params.as_ref(),
                        p.body.as_ref(),
                        p.captures.as_ref(),
                    );
                    let _ = closure_arc.code.set(Arc::new(compiled));
                }
                assign_reg(frame_raw, regs, *dst as usize, clo);
                pc += 1;
            }
            Op::Not(dst, src) => {
                match &regs[*src as usize] {
                    Val::Bool(b) => assign_reg(frame_raw, regs, *dst as usize, Val::Bool(!b)),
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
                pc = ((pc as isize) + (*ofs as isize)) as usize;
            }
            Op::JmpFalse(r, ofs) => {
                let cond_falsey = matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if cond_falsey {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::CmpLtImmJmp { r, imm, ofs } => {
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
                // Fused: if r is nil or false, jump by ofs, else fall through.
                let is_falsey = matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if is_falsey {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::AddIntImmJmp { r, imm, ofs } => {
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
                let count = range_iteration_count(start, end, step_val, *inclusive);
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
            Op::JmpFalseSet { r, dst, ofs } => {
                let cond_falsey = matches!(regs[*r as usize], Val::Nil | Val::Bool(false));
                if cond_falsey {
                    assign_reg(frame_raw, regs, *dst as usize, Val::Bool(false));
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpIfNil(r, ofs) => {
                if matches!(regs[*r as usize], Val::Nil) {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpIfNotNil(r, ofs) => {
                if !matches!(regs[*r as usize], Val::Nil) {
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::NullishPick { l, dst, ofs } => {
                if !matches!(regs[*l as usize], Val::Nil) {
                    assign_reg(frame_raw, regs, *dst as usize, regs[*l as usize].clone());
                    pc = ((pc as isize) + (*ofs as isize)) as usize;
                } else {
                    pc += 1;
                }
            }
            Op::JmpTrueSet { r, dst, ofs } => {
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
                let resume_pc = pc + 1;
                let start = *base as usize;
                let n = *argc as usize;
                let allocator = unsafe { &*region_allocator_ptr };
                let mut next_pc = resume_pc;
                // Fast path: check IC first to avoid cloning the closure Arc.
                let mut ic_fast_path_taken = false;
                if let Some(CallIc::ClosurePositional {
                    closure_ptr,
                    fun_ptr,
                    argc: ic_argc,
                    tiny,
                    ..
                }) = call_ic[pc].as_ref()
                    && *ic_argc == *argc
                {
                    let reg_val = &regs[*rf as usize];
                    if let Val::Closure(arc) = reg_val {
                        let closure_matches = Arc::as_ptr(arc) as usize == *closure_ptr
                            || arc
                                .code
                                .get()
                                .map(|fun| std::ptr::eq(Arc::as_ptr(fun), *fun_ptr))
                                .unwrap_or(false);
                        if closure_matches {
                            // IC hit — skip Arc clone and go straight to fast path.
                            let fun_ptr_val = *fun_ptr;
                            let fun = unsafe { &*fun_ptr_val };
                            let args_slice_fast = &regs[*base as usize..*base as usize + *argc as usize];
                            if let Some(val) = tiny
                                .as_ref()
                                .and_then(|plan| plan.try_eval(args_slice_fast, Some(&arc.captures)))
                            {
                                if *retc > 0 {
                                    assign_reg(frame_raw, regs, *base as usize, val);
                                }
                                ic_fast_path_taken = true;
                            } else {
                                let return_meta = CallFrameMeta {
                                    resume_pc,
                                    ret_base: *base,
                                    retc: *retc,
                                    caller_window: RegisterWindowRef::Current,
                                };
                                let (captures, capture_specs) = arc.frame_captures();
                                // Now get mutable access to the IC cache.
                                if let Some(CallIc::ClosurePositional { cache, frame_info, .. }) = call_ic[pc].as_mut()
                                {
                                    let val = unsafe { &mut *self_ptr }.exec_function_positional_fast(
                                        fun,
                                        args_slice_fast,
                                        ctx,
                                        Some(frame_info),
                                        captures,
                                        capture_specs,
                                        Some(cache),
                                        Some(return_meta),
                                    );
                                    match val {
                                        Ok(val) => {
                                            if *retc > 0 {
                                                assign_reg(frame_raw, regs, *base as usize, val);
                                            }
                                        }
                                        Err(err) => {
                                            return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                        }
                                    }
                                    ic_fast_path_taken = true;
                                }
                            }
                        }
                    }
                }
                if ic_fast_path_taken {
                    if let Some(pending) = unsafe { &mut *self_ptr }.pending_resume_pc.take() {
                        next_pc = pending;
                    }
                    pc = next_pc;
                    continue;
                }
                // Slow path: clone and dispatch as before.
                let func = regs[*rf as usize].clone();
                let args_slice = &regs[*base as usize..*base as usize + *argc as usize];
                match &func {
                    Val::Closure(closure_arc) => {
                        let closure_ptr = Arc::as_ptr(closure_arc) as usize;
                        let mut cached_fast = matches!(call_ic[pc].as_ref(), Some(CallIc::ClosurePositional { closure_ptr: cached_ptr, argc: cached_argc, .. }) if *cached_ptr == closure_ptr && *cached_argc == *argc);
                        let supports_fast = cached_fast || closure_arc.supports_vm_positional_fast_path();
                        if supports_fast && closure_arc.named_params.is_empty() {
                            if !cached_fast && args_slice.len() != closure_arc.params.len() {
                                return frame_return_common(
                                    frame_raw,
                                    pc,
                                    Err(anyhow!(
                                        "Function expects {} positional arguments, got {}",
                                        closure_arc.params.len(),
                                        args_slice.len()
                                    )),
                                )
                                .map(Some);
                            }
                            let closure = closure_arc.as_ref();
                            let mut cached_fun_ptr = None;
                            if let Some(CallIc::ClosurePositional {
                                closure_ptr: cached_ptr,
                                fun_ptr,
                                argc: cached_argc,
                                ..
                            }) = call_ic[pc].as_ref()
                                && *cached_ptr == closure_ptr
                                && *cached_argc == *argc
                            {
                                cached_fun_ptr = Some(*fun_ptr);
                            }
                            let fun: &Function = if let Some(ptr) = cached_fun_ptr {
                                unsafe { &*ptr }
                            } else {
                                closure
                                    .code
                                    .get_or_init(|| {
                                        let c = Compiler::new();
                                        Arc::new(c.compile_function_with_captures(
                                            closure.params.as_ref(),
                                            closure.named_params.as_ref(),
                                            closure.body.as_ref(),
                                            closure.capture_specs.as_ref(),
                                        ))
                                    })
                                    .as_ref()
                            };
                            if !cached_fast
                                && let Some(CallIc::ClosurePositional {
                                    fun_ptr,
                                    argc: cached_argc,
                                    ..
                                }) = call_ic[pc].as_ref()
                                && *cached_argc == *argc
                                && std::ptr::eq(*fun_ptr, fun as *const Function)
                            {
                                cached_fast = true;
                            }
                            let return_meta = CallFrameMeta {
                                resume_pc,
                                ret_base: *base,
                                retc: *retc,
                                caller_window: RegisterWindowRef::Current,
                            };
                            let (captures, capture_specs) = closure.frame_captures();
                            let vm_mut = unsafe { &mut *self_ptr };
                            if let Some(CallIc::ClosurePositional {
                                closure_ptr: _,
                                fun_ptr: _,
                                argc: _,
                                tiny: _,
                                cache,
                                frame_info,
                            }) = call_ic[pc].as_mut()
                                && cached_fast
                            {
                                match vm_mut.exec_function_positional_fast(
                                    fun,
                                    args_slice,
                                    ctx,
                                    Some(&*frame_info),
                                    captures.clone(),
                                    capture_specs.clone(),
                                    Some(cache),
                                    Some(return_meta),
                                ) {
                                    Ok(val) => {
                                        if *retc > 0 {
                                            assign_reg(frame_raw, regs, *base as usize, val);
                                        }
                                    }
                                    Err(err) => {
                                        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                    }
                                }
                            } else {
                                let mut cache = ClosureFastCache::new();
                                let frame_info = closure.frame_info();
                                match vm_mut.exec_function_positional_fast(
                                    fun,
                                    args_slice,
                                    ctx,
                                    Some(&frame_info),
                                    captures,
                                    capture_specs,
                                    Some(&mut cache),
                                    Some(return_meta),
                                ) {
                                    Ok(val) => {
                                        if *retc > 0 {
                                            assign_reg(frame_raw, regs, *base as usize, val);
                                        }
                                        call_ic[pc] = Some(CallIc::ClosurePositional {
                                            closure_ptr,
                                            fun_ptr: fun as *const Function,
                                            argc: *argc,
                                            tiny: TinyCallPlan::analyze(fun),
                                            cache,
                                            frame_info,
                                        });
                                    }
                                    Err(err) => {
                                        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                    }
                                }
                            }
                        } else {
                            let call_args = CallArgs::registers(RegisterSpan::current(start, n));
                            let _frame_guard = CallFrameStackGuard::push(
                                self_ptr,
                                CallFrameMeta {
                                    resume_pc,
                                    ret_base: *base,
                                    retc: *retc,
                                    caller_window: RegisterWindowRef::Current,
                                },
                            );
                            if call_args.len() != closure_arc.params.len() {
                                return frame_return_common(
                                    frame_raw,
                                    pc,
                                    Err(anyhow!(
                                        "Function expects {} positional arguments, got {}",
                                        closure_arc.params.len(),
                                        call_args.len()
                                    )),
                                )
                                .map(Some);
                            }
                            let closure = closure_arc.as_ref();
                            let fun = closure.code.get_or_init(|| {
                                let c = Compiler::new();
                                Arc::new(c.compile_function_with_captures(
                                    closure.params.as_ref(),
                                    closure.named_params.as_ref(),
                                    closure.body.as_ref(),
                                    closure.capture_specs.as_ref(),
                                ))
                            });
                            let frame_info = closure.frame_info();
                            let captures_arc = Arc::clone(&closure.captures);
                            let capture_specs_arc = Arc::clone(&closure.capture_specs);
                            let call_result = if closure.named_params.is_empty() {
                                Vm::exec_function_with_args(
                                    fun.as_ref(),
                                    call_args,
                                    &[],
                                    Some(Arc::clone(&captures_arc)),
                                    Some(Arc::clone(&capture_specs_arc)),
                                    ctx,
                                    self_ptr,
                                    Some(frame_info.clone()),
                                )
                            } else {
                                let named_params = closure.named_params.as_ref();
                                allocator.with_indexed_vals(named_params.len(), |resolved_seed| -> Result<Val> {
                                    for (idx, decl) in named_params.iter().enumerate() {
                                        if let Some(default_fun) =
                                            closure.default_funcs.get(idx).and_then(|opt| opt.as_ref())
                                        {
                                            let default_frame = closure
                                                .default_frame_info(idx)
                                                .expect("default frame info should exist");
                                            let hidden_frame = unsafe { &mut *self_ptr }.frames.pop();
                                            let default_result =
                                                allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                                                    Vm::map_named_seed(
                                                        default_fun,
                                                        resolved_seed.as_slice(),
                                                        seed_regs,
                                                    )?;
                                                    Vm::exec_function_with_args(
                                                        default_fun,
                                                        call_args,
                                                        seed_regs.as_slice(),
                                                        Some(Arc::clone(&captures_arc)),
                                                        Some(Arc::clone(&capture_specs_arc)),
                                                        ctx,
                                                        self_ptr,
                                                        Some(default_frame.clone()),
                                                    )
                                                });
                                            if let Some(meta) = hidden_frame {
                                                unsafe { &mut *self_ptr }.frames.push(meta);
                                            }
                                            let default_val = default_result?;
                                            unsafe { &mut *self_ptr }.pending_resume_pc.take();
                                            resolved_seed.push((idx, default_val));
                                        } else if matches!(decl.type_annotation, Some(Type::Optional(_))) {
                                            resolved_seed.push((idx, Val::Nil));
                                        } else {
                                            return Err(anyhow!("Missing required named argument: {}", decl.name));
                                        }
                                    }
                                    allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                                        Vm::map_named_seed(fun, resolved_seed.as_slice(), seed_regs)?;
                                        Vm::exec_function_with_args(
                                            fun.as_ref(),
                                            call_args,
                                            seed_regs.as_slice(),
                                            Some(Arc::clone(&captures_arc)),
                                            Some(Arc::clone(&capture_specs_arc)),
                                            ctx,
                                            self_ptr,
                                            Some(frame_info.clone()),
                                        )
                                    })
                                })
                            };
                            match call_result {
                                Ok(val) => {
                                    if *retc > 0 {
                                        assign_reg(frame_raw, regs, *base as usize, val);
                                    }
                                }
                                Err(err) => {
                                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                }
                            }
                        }
                    }
                    Val::RustFunction(_) | Val::RustFunctionNamed(_) => {
                        #[cfg(debug_assertions)]
                        eprintln!("encountered rust function call variant");
                        let call_result = if let Some(CallIc::Rust(fp, cached_argc)) = call_ic[pc].as_ref()
                            && *argc == *cached_argc
                            && matches!(func, Val::RustFunction(_))
                        {
                            invoke_rust_function(ctx, *fp, args_slice)
                        } else if let Some(CallIc::RustNamed(fp, cached_argc)) = call_ic[pc].as_ref()
                            && *argc == *cached_argc
                            && matches!(func, Val::RustFunctionNamed(_))
                        {
                            invoke_rust_function_named(ctx, *fp, args_slice, &[])
                        } else {
                            match func.clone() {
                                Val::RustFunction(fptr) => {
                                    call_ic[pc] = Some(CallIc::Rust(fptr, *argc));
                                    invoke_rust_function(ctx, fptr, args_slice)
                                }
                                Val::RustFunctionNamed(fptr) => {
                                    call_ic[pc] = Some(CallIc::RustNamed(fptr, *argc));
                                    invoke_rust_function_named(ctx, fptr, args_slice, &[])
                                }
                                _ => unreachable!(),
                            }
                        };
                        match call_result {
                            Ok(val) => {
                                if *retc > 0 {
                                    assign_reg(frame_raw, regs, *base as usize, val);
                                }
                            }
                            Err(err) => {
                                return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                            }
                        }
                    }
                    _ => {
                        return frame_return_common(
                            frame_raw,
                            pc,
                            Err(anyhow!("{} is not a function", func.type_name())),
                        )
                        .map(Some);
                    }
                }
                if let Some(pending) = unsafe { &mut *self_ptr }.pending_resume_pc.take() {
                    next_pc = pending;
                }
                pc = next_pc;
            }
            Op::CallNamed {
                f: rf,
                base_pos,
                posc,
                base_named,
                namedc,
                retc,
            } => {
                let resume_pc = pc + 1;
                let frame_guard = CallFrameStackGuard::push(
                    self_ptr,
                    CallFrameMeta {
                        resume_pc,
                        ret_base: *base_pos,
                        retc: *retc,
                        caller_window: RegisterWindowRef::Current,
                    },
                );
                let func = regs[*rf as usize].clone();
                let start_pos = *base_pos as usize;
                let npos = *posc as usize;
                let start_named = *base_named as usize;
                let nnamed = *namedc as usize;
                let mut next_pc = resume_pc;
                let allocator = unsafe { &*region_allocator_ptr };
                let pos_slice = &regs[start_pos..start_pos + npos];
                let call_result: Result<()> = match &func {
                    Val::Closure(closure_arc) => {
                        let closure = closure_arc.as_ref();
                        let frame_info = closure.frame_info();
                        if npos != closure.params.len() {
                            return Err(anyhow!(
                                "Function expects {} positional arguments, got {}",
                                closure.params.len(),
                                npos
                            ));
                        }
                        let named_params = closure.named_params.as_ref();
                        let fun = closure.code.get_or_init(|| {
                            let c = Compiler::new();
                            Arc::new(c.compile_function_with_captures(
                                closure.params.as_ref(),
                                named_params,
                                closure.body.as_ref(),
                                closure.capture_specs.as_ref(),
                            ))
                        });
                        let layout = &fun.named_param_layout;
                        if layout.len() != named_params.len() {
                            return Err(anyhow!(
                                "Named parameter layout mismatch (layout={}, decls={})",
                                layout.len(),
                                named_params.len()
                            ));
                        }
                        let positional_span = RegisterSpan::current(start_pos, npos);
                        let call_args = CallArgs::registers(positional_span);
                        let named_slice = &regs[start_named..start_named + nnamed * 2];
                        let closure_ptr = Arc::as_ptr(closure_arc) as usize;
                        let cached_plan = if let Some(CallIc::ClosureNamed {
                            closure_ptr: cached_ptr,
                            named_len,
                            plan,
                        }) = call_ic[pc].as_ref()
                        {
                            if *cached_ptr == closure_ptr && *named_len as usize == nnamed {
                                Some(plan.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let plan = if let Some(plan) = cached_plan {
                            plan
                        } else {
                            match build_named_call_plan(closure, named_slice) {
                                Ok(plan) => {
                                    call_ic[pc] = Some(CallIc::ClosureNamed {
                                        closure_ptr,
                                        named_len: nnamed as u8,
                                        plan: plan.clone(),
                                    });
                                    plan
                                }
                                Err(err) => return Err(err),
                            }
                        };
                        allocator.with_indexed_vals(
                            plan.provided_indices.len() + plan.defaults_to_eval.len() + plan.optional_nil.len(),
                            |seed_pairs| {
                                seed_pairs.clear();
                                for (arg_idx, param_idx) in plan.provided_indices.iter().enumerate() {
                                    let val_reg = start_named + 2 * arg_idx + 1;
                                    seed_pairs.push((*param_idx, regs[val_reg].clone()));
                                }
                                for &default_idx in plan.defaults_to_eval.iter() {
                                    let default_fun = closure
                                        .default_funcs
                                        .get(default_idx)
                                        .and_then(|opt| opt.as_ref())
                                        .expect("default function must exist for DefaultThunk");
                                    let default_frame = closure
                                        .default_frame_info(default_idx)
                                        .expect("default frame info should exist");
                                    let default_layout = closure
                                        .default_seed_regs(default_idx)
                                        .expect("default seed layout should exist for default thunk");
                                    let hidden_frame = unsafe { &mut *self_ptr }.frames.pop();
                                    let default_result = allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
                                        for (seed_idx, seed_val) in seed_pairs.iter() {
                                            let reg = default_layout
                                                .get(*seed_idx)
                                                .copied()
                                                .expect("default seed layout must cover parent index");
                                            seed_regs.push((reg, seed_val.clone()));
                                        }
                                        Vm::exec_function_with_args(
                                            default_fun,
                                            call_args,
                                            seed_regs.as_slice(),
                                            Some(Arc::clone(&closure.captures)),
                                            Some(Arc::clone(&closure.capture_specs)),
                                            ctx,
                                            self_ptr,
                                            Some(default_frame.clone()),
                                        )
                                    });
                                    if let Some(meta) = hidden_frame {
                                        unsafe { &mut *self_ptr }.frames.push(meta);
                                    }
                                    let default_val = default_result?;
                                    unsafe { &mut *self_ptr }.pending_resume_pc.take();
                                    seed_pairs.push((default_idx, default_val));
                                }
                                for &optional_idx in plan.optional_nil.iter() {
                                    seed_pairs.push((optional_idx, Val::Nil));
                                }

                                allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
                                    for (seed_idx, seed_val) in seed_pairs.iter() {
                                        let reg = fun.named_param_regs.get(*seed_idx).copied().ok_or_else(|| {
                                            anyhow!("Named parameter index {} out of range", seed_idx)
                                        })?;
                                        seed_regs.push((reg, seed_val.clone()));
                                    }
                                    let captures = Some(Arc::clone(&closure.captures));
                                    let capture_specs = Some(Arc::clone(&closure.capture_specs));
                                    let result = Vm::exec_function_with_args(
                                        fun.as_ref(),
                                        call_args,
                                        seed_regs.as_slice(),
                                        captures,
                                        capture_specs,
                                        ctx,
                                        self_ptr,
                                        Some(frame_info.clone()),
                                    );
                                    match result {
                                        Ok(val) => {
                                            if *retc > 0 {
                                                assign_reg(frame_raw, regs, *base_pos as usize, val);
                                            }
                                            Ok(())
                                        }
                                        Err(err) => Err(err),
                                    }
                                })
                            },
                        )
                    }
                    Val::RustFunction(_) => {
                        if nnamed > 0 {
                            return Err(anyhow!("Named arguments are not supported for native functions"));
                        }
                        let call_result = if let Some(CallIc::Rust(fp, cached_argc)) = call_ic[pc].as_ref()
                            && *posc == *cached_argc
                            && matches!(func, Val::RustFunction(_))
                        {
                            invoke_rust_function(ctx, *fp, pos_slice)
                        } else {
                            match func.clone() {
                                Val::RustFunction(fptr) => {
                                    call_ic[pc] = Some(CallIc::Rust(fptr, *posc));
                                    invoke_rust_function(ctx, fptr, pos_slice)
                                }
                                _ => unreachable!(),
                            }
                        };
                        match call_result {
                            Ok(val) => {
                                if *retc > 0 {
                                    assign_reg(frame_raw, regs, *base_pos as usize, val);
                                }
                                Ok(())
                            }
                            Err(err) => Err(err),
                        }
                    }
                    Val::RustFunctionNamed(_) => {
                        let call_output = allocator.with_named_pairs(nnamed, |named_vec| {
                            for i in 0..nnamed {
                                let key_val = &regs[start_named + 2 * i];
                                let val = regs[start_named + 2 * i + 1].clone();
                                let key = match key_val {
                                    Val::Str(s) => s.to_string(),
                                    Val::Int(i) => i.to_string(),
                                    Val::Float(f) => f.to_string(),
                                    Val::Bool(b) => b.to_string(),
                                    _ => {
                                        return Err(anyhow!("Named argument key must be primitive, got {:?}", key_val));
                                    }
                                };
                                named_vec.push((key, val));
                            }
                            let fptr = if let Val::RustFunctionNamed(ptr) = func.clone() {
                                ptr
                            } else {
                                unreachable!()
                            };
                            invoke_rust_function_named(ctx, fptr, pos_slice, named_vec.as_slice())
                        });
                        match call_output {
                            Ok(val) => {
                                if *retc > 0 {
                                    assign_reg(frame_raw, regs, *base_pos as usize, val);
                                }
                                Ok(())
                            }
                            Err(err) => Err(err),
                        }
                    }
                    _ => Err(anyhow!("{} is not a function", func.type_name())),
                };
                if let Err(err) = call_result {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
                if let Some(pending) = unsafe { &mut *self_ptr }.pending_resume_pc.take() {
                    next_pc = pending;
                }
                drop(frame_guard);
                pc = next_pc;
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
