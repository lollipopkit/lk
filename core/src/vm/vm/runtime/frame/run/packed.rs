//! Packed 32-bit bytecode fast-path interpreter.
//!
//! This module provides a switch-free dispatch loop for BC32-encoded functions.
//! It uses a two-tier caching strategy:
//!
//! 1. **Packed Hot Cache** (`PackedHotEntry`): Per-instruction-site cache that
//!    stores pre-decoded hot slots. Each slot maps a raw 32-bit word to a
//!    `PackedHotKind` (Arith, Cmp, ForRange, etc.) and can execute the
//!    instruction without a full `match` dispatch.
//!
//! 2. **Sentinel-based skip**: For cold instruction sites, a `Miss` entry
//!    records the last word seen. If the same word appears again (monomorphic
//!    site), the interpreter skips the hot-slot build attempt and directly
//!    decodes the instruction via `fetch_packed_op`.
//!
//! ## ForRange Fusion
//!
//! For-range loops execute as:
//!   ForRangePrep → ForRangeLoop → body → ForRangeStep → (back to Loop)
//!
//! The `ForRangeLoop` hot slot peeks at the next word: if it's `ForRangeStep`,
//! it jumps directly back to the loop guard, saving one hot-slot dispatch per
//! iteration. This is the equivalent of peephole fusion for the packed path.

use arcstr::ArcStr;
use std::sync::Arc;
#[cfg(debug_assertions)]
use std::{
    collections::BTreeMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, anyhow};

use crate::op::BinOp;
use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use crate::val::{ClosureCapture, ClosureInit, ClosureValue, Type, Val};
use crate::vm::RegionPlan;
use crate::vm::alloc::{AllocationRegion, RegionAllocator};
use crate::vm::bc32::{self, Bc32Decoded, Tag};
use crate::vm::bytecode::{CaptureSpec, Function, Op, rk_index, rk_is_const, rk_make_const};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    AccessIc, CallIc, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, PackedArithOp, PackedCmpImmOp,
    PackedCmpOp, PackedHotEntry, PackedHotKind, PackedHotSlot, PackedRangeFusion, TinyCallPlan, VmCaches,
};
use crate::vm::vm::frame::{CallArgs, CallFrameMeta, CallFrameStackGuard, FrameState, RegisterSpan, RegisterWindowRef};

use super::helpers::{
    assign_reg, fetch_for_range_state, frame_return_common, handle_return_common, insert_map_entry, push_list_entry,
};
use super::invoke::{invoke_rust_function, invoke_rust_function_named};
use super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, float_binop, int_binop, int_binop_imm, rk_read};
use super::plan::build_named_call_plan;

mod cold_basic;
mod cold_math;
mod decode;
mod fetch;
mod hot_exec;
use cold_basic::*;
use cold_math::*;
use decode::*;
use fetch::*;
use hot_exec::*;

#[cfg(debug_assertions)]
static PACKED_HOT_HITS: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_SENTINEL_SKIPS: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_BUILD_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_BUILD_SUCCESSES: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PACKED_HOT_SENTINEL_TAGS: OnceLock<Mutex<BTreeMap<u8, usize>>> = OnceLock::new();

#[cfg(debug_assertions)]
struct PackedHotStatsGuard {
    dump: bool,
}

#[cfg(debug_assertions)]
impl PackedHotStatsGuard {
    fn new() -> Self {
        let dump = std::env::var("LK_DUMP_PACKED_STATS")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false);
        Self { dump }
    }
}

#[cfg(debug_assertions)]
impl Drop for PackedHotStatsGuard {
    fn drop(&mut self) {
        if self.dump {
            let hits = PACKED_HOT_HITS.swap(0, Ordering::Relaxed);
            let sentinel_skips = PACKED_HOT_SENTINEL_SKIPS.swap(0, Ordering::Relaxed);
            let attempts = PACKED_HOT_BUILD_ATTEMPTS.swap(0, Ordering::Relaxed);
            let successes = PACKED_HOT_BUILD_SUCCESSES.swap(0, Ordering::Relaxed);
            eprintln!(
                "[packed-hot-cache] hits={} sentinel_skips={} build_successes={} build_attempts={}",
                hits, sentinel_skips, successes, attempts
            );
            if let Some(map) = PACKED_HOT_SENTINEL_TAGS.get() {
                let mut guard = map.lock().unwrap();
                if !guard.is_empty() {
                    eprintln!("[packed-hot-cache] sentinel breakdown:");
                    for (raw_tag, count) in guard.iter() {
                        let label = match bc32::decode_tag_byte(*raw_tag) {
                            bc32::DecodedTag::Regular { tag, .. } => format!("  {:?}", tag),
                            bc32::DecodedTag::RegExt => "  <RegExt>".to_string(),
                            bc32::DecodedTag::Ext => format!("  <Ext:{}>", raw_tag),
                        };
                        eprintln!("{} => {}", label, count);
                    }
                    guard.clear();
                }
            }
        }
    }
}

#[cfg(debug_assertions)]
fn record_sentinel_tag(word: u32) {
    let raw_tag = bc32::tag_of(word);
    let map = PACKED_HOT_SENTINEL_TAGS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut guard = map.lock().unwrap();
    *guard.entry(raw_tag).or_insert(0) += 1;
}

fn make_closure_value(f: &Function, proto: u16, ctx: &mut VmContext, regs: &[Val], _frame_base: usize) -> Result<Val> {
    let p = f
        .protos
        .get(proto as usize)
        .ok_or_else(|| anyhow!("closure proto out of range"))?;
    if p.self_name.is_none() && p.captures.is_empty() {
        return Ok(p
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
            .clone());
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
                let idx = *src as usize;
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
                    let idx = *src as usize;
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
    Ok(clo)
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
                    Err(anyhow!("bc32: unexpected Ext word without preceding opcode")),
                )
                .map(Some);
            }
        }
        let mut skip_build = false;
        if let Some(entry) = packed_hot.get(pc).and_then(|slot| slot.as_ref()) {
            match entry {
                PackedHotEntry::Slot(slot) => {
                    if slot.word == word {
                        #[cfg(debug_assertions)]
                        PACKED_HOT_HITS.fetch_add(1, Ordering::Relaxed);
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
                        let override_pc = exec_hot_slot(
                            slot,
                            frame_raw,
                            regs,
                            f,
                            ctx,
                            global_ic,
                            call_ic,
                            for_range_ic,
                            pc,
                            frame_base,
                        )?;
                        pc = override_pc.unwrap_or(slot.next_pc);
                        continue;
                    }
                }
                PackedHotEntry::Miss(last_word) => {
                    if *last_word == word {
                        #[cfg(debug_assertions)]
                        PACKED_HOT_SENTINEL_SKIPS.fetch_add(1, Ordering::Relaxed);
                        #[cfg(debug_assertions)]
                        record_sentinel_tag(word);
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
            #[cfg(debug_assertions)]
            PACKED_HOT_BUILD_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
            if let Some(entry) = build_hot_slot(code32, pc, word, raw_tag) {
                #[cfg(debug_assertions)]
                PACKED_HOT_BUILD_SUCCESSES.fetch_add(1, Ordering::Relaxed);
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
                let override_pc = exec_hot_slot(
                    &entry,
                    frame_raw,
                    regs,
                    f,
                    ctx,
                    global_ic,
                    call_ic,
                    for_range_ic,
                    pc,
                    frame_base,
                )?;
                if packed_hot.len() <= pc {
                    packed_hot.resize(pc + 1, None);
                }
                packed_hot[pc] = Some(PackedHotEntry::Slot(entry));
                pc = override_pc.unwrap_or(next_pc);
                continue;
            } else {
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
                let resume_pc = next_pc_default;
                let start = base as usize;
                let n = argc as usize;
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
                    && *ic_argc == argc
                {
                    let reg_val = &regs[rf as usize];
                    if let Val::Closure(arc) = reg_val {
                        let closure_matches = Arc::as_ptr(arc) as usize == *closure_ptr
                            || arc
                                .code
                                .get()
                                .map(|fun| std::ptr::eq(Arc::as_ptr(fun), *fun_ptr))
                                .unwrap_or(false);
                        if closure_matches {
                            let fun_ptr_val = *fun_ptr;
                            let fun = unsafe { &*fun_ptr_val };
                            let args_slice_fast = &regs[start..start + n];
                            if let Some(val) = tiny
                                .as_ref()
                                .and_then(|plan| plan.try_eval(args_slice_fast, Some(&arc.captures)))
                            {
                                if retc > 0 {
                                    assign_reg(frame_raw, regs, base as usize, val);
                                }
                                ic_fast_path_taken = true;
                            } else {
                                let return_meta = CallFrameMeta {
                                    resume_pc,
                                    ret_base: base,
                                    retc,
                                    caller_window: RegisterWindowRef::Base(frame_base),
                                };
                                let (captures, capture_specs) = arc.frame_captures();
                                if let Some(CallIc::ClosurePositional { cache, frame_info, .. }) = call_ic[pc].as_mut()
                                {
                                    let val = unsafe { &mut *self_ptr }.exec_function_positional_fast_span(
                                        fun,
                                        RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
                                        ctx,
                                        Some(frame_info),
                                        captures,
                                        capture_specs,
                                        Some(cache),
                                        Some(return_meta),
                                    );
                                    match val {
                                        Ok(val) => {
                                            if retc > 0 {
                                                assign_reg(frame_raw, regs, base as usize, val);
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
                // Slow path.
                let args_slice = &regs[start..start + n];
                let func = regs[rf as usize].clone();
                let call_args = CallArgs::registers(RegisterSpan::current(start, n));
                match &func {
                    Val::Closure(closure_arc) => {
                        let closure_ptr = Arc::as_ptr(closure_arc) as usize;
                        let mut cached_fast = matches!(call_ic[pc].as_ref(), Some(CallIc::ClosurePositional { closure_ptr: cached_ptr, argc: cached_argc, .. }) if *cached_ptr == closure_ptr && *cached_argc == argc);
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
                            let return_meta = CallFrameMeta {
                                resume_pc,
                                ret_base: base,
                                retc,
                                caller_window: RegisterWindowRef::Base(frame_base),
                            };
                            let closure = closure_arc.as_ref();
                            let mut cached_fun_ptr = None;
                            if let Some(CallIc::ClosurePositional {
                                closure_ptr: cached_ptr,
                                fun_ptr,
                                argc: cached_argc,
                                ..
                            }) = call_ic[pc].as_ref()
                                && *cached_ptr == closure_ptr
                                && *cached_argc == argc
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
                                && *cached_argc == argc
                                && std::ptr::eq(*fun_ptr, fun as *const Function)
                            {
                                cached_fast = true;
                            }
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
                                match vm_mut.exec_function_positional_fast_span(
                                    fun,
                                    RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
                                    ctx,
                                    Some(&*frame_info),
                                    captures.clone(),
                                    capture_specs.clone(),
                                    Some(cache),
                                    Some(return_meta),
                                ) {
                                    Ok(val) => {
                                        if retc > 0 {
                                            assign_reg(frame_raw, regs, base as usize, val);
                                        }
                                    }
                                    Err(err) => {
                                        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                    }
                                }
                            } else {
                                let mut cache = ClosureFastCache::new();
                                let frame_info = closure.frame_info();
                                match vm_mut.exec_function_positional_fast_span(
                                    fun,
                                    RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
                                    ctx,
                                    Some(&frame_info),
                                    captures,
                                    capture_specs,
                                    Some(&mut cache),
                                    Some(return_meta),
                                ) {
                                    Ok(val) => {
                                        if retc > 0 {
                                            assign_reg(frame_raw, regs, base as usize, val);
                                        }
                                        call_ic[pc] = Some(CallIc::ClosurePositional {
                                            closure_ptr,
                                            fun_ptr: fun as *const Function,
                                            argc,
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
                            let _frame_guard = CallFrameStackGuard::push(
                                self_ptr,
                                CallFrameMeta {
                                    resume_pc,
                                    ret_base: base,
                                    retc,
                                    caller_window: RegisterWindowRef::Base(frame_base),
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
                                            fun,
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
                                    if retc > 0 {
                                        assign_reg(frame_raw, regs, base as usize, val);
                                    }
                                }
                                Err(err) => {
                                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                }
                            }
                        }
                    }
                    Val::RustFunction(_) | Val::RustFunctionNamed(_) => {
                        let call_result = if let Some(CallIc::Rust(fp, cached_argc)) = call_ic[pc].as_ref()
                            && argc == *cached_argc
                            && matches!(func, Val::RustFunction(_))
                        {
                            invoke_rust_function(ctx, *fp, args_slice)
                        } else if let Some(CallIc::RustNamed(fp, cached_argc)) = call_ic[pc].as_ref()
                            && argc == *cached_argc
                            && matches!(func, Val::RustFunctionNamed(_))
                        {
                            invoke_rust_function_named(ctx, *fp, args_slice, &[])
                        } else {
                            match func.clone() {
                                Val::RustFunction(fptr) => {
                                    call_ic[pc] = Some(CallIc::Rust(fptr, argc));
                                    invoke_rust_function(ctx, fptr, args_slice)
                                }
                                Val::RustFunctionNamed(fptr) => {
                                    call_ic[pc] = Some(CallIc::RustNamed(fptr, argc));
                                    invoke_rust_function_named(ctx, fptr, args_slice, &[])
                                }
                                _ => unreachable!(),
                            }
                        };
                        match call_result {
                            Ok(val) => {
                                if retc > 0 {
                                    assign_reg(frame_raw, regs, base as usize, val);
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
                let resume_pc = next_pc_default;
                let frame_guard = CallFrameStackGuard::push(
                    self_ptr,
                    CallFrameMeta {
                        resume_pc,
                        ret_base: base_pos,
                        retc,
                        caller_window: RegisterWindowRef::Base(frame_base),
                    },
                );
                let func = regs[rf as usize].clone();
                let pos_start = base_pos as usize;
                let pos_len = posc as usize;
                let named_start = base_named as usize;
                let named_len = namedc as usize;
                let args_slice = &regs[pos_start..pos_start + pos_len];
                let call_args = CallArgs::registers(RegisterSpan::current(pos_start, pos_len));
                let named_slice = &regs[named_start..named_start + named_len * 2];
                let allocator = unsafe { &*region_allocator_ptr };
                let mut next_pc = resume_pc;
                match &func {
                    Val::Closure(closure_arc) => {
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
                        let closure_ptr = Arc::as_ptr(closure_arc) as usize;
                        let cached_plan = if let Some(CallIc::ClosureNamed {
                            closure_ptr: cached_ptr,
                            named_len: cached_len,
                            plan,
                        }) = call_ic[pc].as_ref()
                        {
                            if *cached_ptr == closure_ptr && *cached_len as usize == named_len {
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
                                        named_len: named_len as u8,
                                        plan: plan.clone(),
                                    });
                                    plan
                                }
                                Err(err) => {
                                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                                }
                            }
                        };
                        let call_result = allocator.with_indexed_vals(
                            plan.provided_indices.len() + plan.defaults_to_eval.len() + plan.optional_nil.len(),
                            |seed_pairs| {
                                seed_pairs.clear();
                                for (arg_idx, param_idx) in plan.provided_indices.iter().enumerate() {
                                    let value_val = named_slice[2 * arg_idx + 1].clone();
                                    seed_pairs.push((*param_idx, value_val));
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
                            },
                        );
                        match call_result {
                            Ok(val) => {
                                if retc > 0 {
                                    assign_reg(frame_raw, regs, base_pos as usize, val);
                                }
                            }
                            Err(err) => {
                                return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                            }
                        }
                    }
                    Val::RustFunctionNamed(_) => {
                        let call_output = allocator.with_named_pairs(named_len, |named_vec| {
                            for i in 0..named_len {
                                let key_val = &regs[named_start + 2 * i];
                                let val = regs[named_start + 2 * i + 1].clone();
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
                            invoke_rust_function_named(ctx, fptr, args_slice, named_vec.as_slice())
                        });
                        match call_output {
                            Ok(val) => {
                                if retc > 0 {
                                    assign_reg(frame_raw, regs, base_pos as usize, val);
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
                drop(frame_guard);
                pc = next_pc;
            }
            Op::LoadCapture { dst, idx } => {
                let capture_idx = idx as usize;
                if let Some(spec) = frame_capture_specs.as_ref().and_then(|specs| specs.get(capture_idx)) {
                    match spec {
                        CaptureSpec::Global { name } => {
                            let value = ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil);
                            assign_reg(frame_raw, regs, dst as usize, value);
                        }
                        _ => {
                            let captured = frame_captures
                                .as_ref()
                                .and_then(|caps| caps.value_at(capture_idx).cloned())
                                .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))?;
                            assign_reg(frame_raw, regs, dst as usize, captured);
                        }
                    }
                } else {
                    let captured = frame_captures
                        .as_ref()
                        .and_then(|caps| caps.value_at(capture_idx).cloned())
                        .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))?;
                    assign_reg(frame_raw, regs, dst as usize, captured);
                }
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
                            let allocator = unsafe { &*region_allocator_ptr };
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
