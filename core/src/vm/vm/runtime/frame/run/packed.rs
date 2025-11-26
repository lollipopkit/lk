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
use crate::vm::bytecode::{CaptureSpec, Function, Op, rk_make_const};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    AccessIc, CallIc, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, PackedArithOp, PackedCmpImmOp,
    PackedCmpOp, PackedHotEntry, PackedHotKind, PackedHotSlot, VmCaches,
};
use crate::vm::vm::frame::{CallArgs, CallFrameMeta, CallFrameStackGuard, FrameState, RegisterSpan, RegisterWindowRef};
use crate::vm::vm::guards::VmCurrentGuard;

use super::helpers::{assign_reg, fetch_for_range_state, frame_return_common, handle_return_common};
use super::invoke::{invoke_rust_function, invoke_rust_function_named};
use super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, float_binop, int_binop, int_binop_imm, rk_read};
use super::plan::build_named_call_plan;

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
        let dump = std::env::var("LKR_DUMP_PACKED_STATS")
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

const RK_FLAG_B: u8 = 0x01;
const RK_FLAG_C: u8 = 0x02;

#[inline(always)]
fn decode_abc(word: u32, reg_ext: Option<u32>) -> (u16, u16, u16) {
    let lo_a = ((word >> 16) & 0xFF) as u16;
    let lo_b = ((word >> 8) & 0xFF) as u16;
    let lo_c = (word & 0xFF) as u16;
    let (hi_a, hi_b, hi_c) = bc32::unpack_reg_ext(reg_ext);
    (
        bc32::combine_reg(hi_a, lo_a),
        bc32::combine_reg(hi_b, lo_b),
        bc32::combine_reg(hi_c, lo_c),
    )
}

#[inline(always)]
fn decode_rk_pair(word: u32, reg_ext: Option<u32>, flags: u8) -> (u16, u16, u16) {
    let (dst, b_reg, c_reg) = decode_abc(word, reg_ext);
    let b_rk = if (flags & RK_FLAG_B) != 0 {
        rk_make_const(b_reg)
    } else {
        b_reg
    };
    let c_rk = if (flags & RK_FLAG_C) != 0 {
        rk_make_const(c_reg)
    } else {
        c_reg
    };
    (dst, b_rk, c_rk)
}

#[inline(always)]
fn decode_ab_imm(word: u32, reg_ext: Option<u32>) -> (u16, u16, i16) {
    let (dst, src, _) = decode_abc(word, reg_ext);
    let imm = ((word & 0xFF) as u8 as i8) as i16;
    (dst, src, imm)
}

#[inline(always)]
fn build_hot_slot(code32: &[u32], pc: usize, word: u32, raw_tag: u8) -> Option<PackedHotSlot> {
    if let bc32::DecodedTag::Regular { tag, flags } = bc32::decode_tag_byte(raw_tag) {
        let mut next_pc = pc + 1;
        let mut reg_ext = None;
        if next_pc < code32.len() && bc32::tag_of(code32[next_pc]) == bc32::TAG_REG_EXT {
            reg_ext = Some(code32[next_pc]);
            next_pc += 1;
        }
        let kind = match tag {
            Tag::Move => {
                let (dst, src, _) = decode_abc(word, reg_ext);
                PackedHotKind::Move { dst, src }
            }
            Tag::LoadK => {
                let (dst, kidx, _) = decode_abc(word, reg_ext);
                PackedHotKind::LoadK { dst, kidx }
            }
            Tag::LoadLocal => {
                let (dst, idx, _) = decode_abc(word, reg_ext);
                PackedHotKind::LoadLocal { dst, idx }
            }
            Tag::StoreLocal => {
                let (idx, src, _) = decode_abc(word, reg_ext);
                PackedHotKind::StoreLocal { idx, src }
            }
            Tag::LoadGlobal => {
                let (dst, kidx, _) = decode_abc(word, reg_ext);
                PackedHotKind::LoadGlobal { dst, name_k: kidx }
            }
            Tag::DefineGlobal => {
                let (name_k, src, _) = decode_abc(word, reg_ext);
                PackedHotKind::DefineGlobal { name_k, src }
            }
            Tag::ForRangeLoop => {
                let (idx, _, _) = decode_abc(word, reg_ext);
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                next_pc += 1;
                PackedHotKind::ForRangeLoop { idx, ofs }
            }
            Tag::ForRangeStep => {
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let back_ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                next_pc += 1;
                PackedHotKind::ForRangeStep { back_ofs }
            }
            Tag::ToStr => {
                let (dst, src, _) = decode_abc(word, reg_ext);
                PackedHotKind::ToStr { dst, src }
            }
            Tag::Add => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Arith {
                    op: PackedArithOp::Add,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Sub => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Arith {
                    op: PackedArithOp::Sub,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Mul => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Arith {
                    op: PackedArithOp::Mul,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Div => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Arith {
                    op: PackedArithOp::Div,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Mod => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Arith {
                    op: PackedArithOp::Mod,
                    dst,
                    a,
                    b,
                }
            }
            Tag::AddIntImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::AddIntImm { dst, src, imm }
            }
            Tag::CmpEqImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::CmpImm {
                    op: PackedCmpImmOp::Eq,
                    dst,
                    src,
                    imm,
                }
            }
            Tag::CmpNeImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::CmpImm {
                    op: PackedCmpImmOp::Ne,
                    dst,
                    src,
                    imm,
                }
            }
            Tag::CmpLtImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::CmpImm {
                    op: PackedCmpImmOp::Lt,
                    dst,
                    src,
                    imm,
                }
            }
            Tag::CmpLeImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::CmpImm {
                    op: PackedCmpImmOp::Le,
                    dst,
                    src,
                    imm,
                }
            }
            Tag::CmpGtImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::CmpImm {
                    op: PackedCmpImmOp::Gt,
                    dst,
                    src,
                    imm,
                }
            }
            Tag::CmpGeImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::CmpImm {
                    op: PackedCmpImmOp::Ge,
                    dst,
                    src,
                    imm,
                }
            }
            Tag::Eq => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Cmp {
                    op: PackedCmpOp::Eq,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Ne => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Cmp {
                    op: PackedCmpOp::Ne,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Lt => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Cmp {
                    op: PackedCmpOp::Lt,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Le => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Cmp {
                    op: PackedCmpOp::Le,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Gt => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Cmp {
                    op: PackedCmpOp::Gt,
                    dst,
                    a,
                    b,
                }
            }
            Tag::Ge => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                PackedHotKind::Cmp {
                    op: PackedCmpOp::Ge,
                    dst,
                    a,
                    b,
                }
            }
            _ => return None,
        };
        Some(PackedHotSlot { word, next_pc, kind })
    } else {
        None
    }
}

#[inline(always)]
fn exec_hot_slot(
    entry: &PackedHotSlot,
    frame_raw: *mut FrameState<'_>,
    regs: &mut Vec<Val>,
    func: &Function,
    ctx: &mut VmContext,
    global_ic: &mut Vec<Option<GlobalEntry>>,
    for_range_ic: &mut Vec<Option<ForRangeState>>,
    pc: usize,
) -> Result<Option<usize>> {
    let result = match &entry.kind {
        PackedHotKind::Move { dst, src } => {
            assign_reg(frame_raw, regs, *dst as usize, regs[*src as usize].clone());
            None
        }
        PackedHotKind::LoadK { dst, kidx } => {
            assign_reg(frame_raw, regs, *dst as usize, func.consts[*kidx as usize].clone());
            None
        }
        PackedHotKind::LoadLocal { dst, idx } => {
            assign_reg(frame_raw, regs, *dst as usize, regs[*idx as usize].clone());
            None
        }
        PackedHotKind::StoreLocal { idx, src } => {
            let value = regs[*src as usize].clone();
            assign_reg(frame_raw, regs, *idx as usize, value);
            None
        }
        PackedHotKind::LoadGlobal { dst, name_k } => {
            let name_val = &func.consts[*name_k as usize];
            let mut out = Val::Nil;
            if let Val::Str(s) = name_val {
                let key_ptr = s.as_ref().as_ptr() as usize;
                let cur_gen = ctx.generation();
                let local_shadowed = ctx.is_local_name(s.as_ref());
                if !local_shadowed {
                    if let Some(GlobalEntry(ptr, v, generation)) = &global_ic[pc]
                        && *ptr == key_ptr
                        && *generation == cur_gen
                    {
                        out = v.clone();
                    } else if let Some(v) = ctx.get(s.as_ref()) {
                        out = v.clone();
                        global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                    }
                }
                if matches!(out, Val::Nil) {
                    if let Some(v) = ctx.get_value(s.as_ref()) {
                        out = v;
                        if !local_shadowed {
                            global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                        }
                    }
                }
                if matches!(out, Val::Nil) {
                    if let Some(builtin) = ctx.resolver().get_builtin(s.as_ref()) {
                        out = builtin.clone();
                        if !local_shadowed {
                            global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                        }
                    }
                }
            } else {
                let fallback_name = format!("{}", name_val);
                if let Some(v) = ctx.get(fallback_name.as_str()) {
                    out = v.clone();
                }
            }
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::DefineGlobal { name_k, src } => {
            if let Val::Str(s) = &func.consts[*name_k as usize] {
                ctx.set(s.as_ref().to_owned(), regs[*src as usize].clone());
            }
            None
        }
        PackedHotKind::ForRangeLoop { idx, ofs } => {
            let state_entry = fetch_for_range_state(for_range_ic, pc)?;
            if state_entry.should_continue() {
                assign_reg(frame_raw, regs, *idx as usize, Val::Int(state_entry.current));
                None
            } else {
                for_range_ic[pc] = None;
                Some(((pc as isize) + (*ofs as isize)) as usize)
            }
        }
        PackedHotKind::ForRangeStep { back_ofs } => {
            let guard_pc = ((pc as isize) + (*back_ofs as isize)) as usize;
            let state_entry = fetch_for_range_state(for_range_ic, guard_pc)?;
            state_entry.current += state_entry.step;
            Some(guard_pc)
        }
        PackedHotKind::ToStr { dst, src } => {
            let s = regs[*src as usize].to_string();
            assign_reg(frame_raw, regs, *dst as usize, Val::Str(s.into()));
            None
        }
        PackedHotKind::Arith { op, dst, a, b } => {
            match op {
                PackedArithOp::Add => {
                    if !Vm::arith2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        "add",
                        |x, y| x + y,
                        |x, y| x + y,
                    ) {
                        let out =
                            BinOp::Add.eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                }
                PackedArithOp::Sub => {
                    if !Vm::arith2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        "sub",
                        |x, y| x - y,
                        |x, y| x - y,
                    ) {
                        let out =
                            BinOp::Sub.eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                }
                PackedArithOp::Mul => {
                    if !Vm::arith2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        "mul",
                        |x, y| x * y,
                        |x, y| x * y,
                    ) {
                        let out =
                            BinOp::Mul.eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                }
                PackedArithOp::Div => {
                    if !Vm::arith2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        "div",
                        |x, y| x / y,
                        |x, y| x / y,
                    ) {
                        let out =
                            BinOp::Div.eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                }
                PackedArithOp::Mod => match (rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b)) {
                    (Val::Int(x), Val::Int(y)) => assign_reg(frame_raw, regs, *dst as usize, Val::Int(x % y)),
                    _ => {
                        let out =
                            BinOp::Mod.eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                },
            }
            None
        }
        PackedHotKind::Cmp { op, dst, a, b } => {
            match op {
                PackedCmpOp::Eq => assign_reg(
                    frame_raw,
                    regs,
                    *dst as usize,
                    Val::Bool(rk_read(regs, &func.consts, *a) == rk_read(regs, &func.consts, *b)),
                ),
                PackedCmpOp::Ne => assign_reg(
                    frame_raw,
                    regs,
                    *dst as usize,
                    Val::Bool(rk_read(regs, &func.consts, *a) != rk_read(regs, &func.consts, *b)),
                ),
                PackedCmpOp::Lt => {
                    if !Vm::cmp2_try_numeric(frame_raw, regs, &func.consts, *dst, *a, *b, |x, y| x < y, |x, y| x < y) {
                        let res = BinOp::Lt.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
                PackedCmpOp::Le => {
                    if !Vm::cmp2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        |x, y| x <= y,
                        |x, y| x <= y,
                    ) {
                        let res = BinOp::Le.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
                PackedCmpOp::Gt => {
                    if !Vm::cmp2_try_numeric(frame_raw, regs, &func.consts, *dst, *a, *b, |x, y| x > y, |x, y| x > y) {
                        let res = BinOp::Gt.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
                PackedCmpOp::Ge => {
                    if !Vm::cmp2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        |x, y| x >= y,
                        |x, y| x >= y,
                    ) {
                        let res = BinOp::Ge.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
            }
            None
        }
        PackedHotKind::AddIntImm { dst, src, imm } => {
            int_binop_imm(
                frame_raw,
                regs,
                &func.consts,
                *dst,
                *src,
                *imm,
                |x, y| x + y,
                BinOp::Add,
            )?;
            None
        }
        PackedHotKind::CmpImm { op, dst, src, imm } => {
            match op {
                PackedCmpImmOp::Eq => cmp_eq_imm(frame_raw, regs, &func.consts, *dst, *src, *imm, BinOp::Eq)?,
                PackedCmpImmOp::Ne => cmp_ne_imm(frame_raw, regs, &func.consts, *dst, *src, *imm, BinOp::Ne)?,
                PackedCmpImmOp::Lt => cmp_ord_imm(
                    frame_raw,
                    regs,
                    &func.consts,
                    *dst,
                    *src,
                    *imm,
                    |x, y| x < y,
                    |x, y| x < y,
                    BinOp::Lt,
                )?,
                PackedCmpImmOp::Le => cmp_ord_imm(
                    frame_raw,
                    regs,
                    &func.consts,
                    *dst,
                    *src,
                    *imm,
                    |x, y| x <= y,
                    |x, y| x <= y,
                    BinOp::Le,
                )?,
                PackedCmpImmOp::Gt => cmp_ord_imm(
                    frame_raw,
                    regs,
                    &func.consts,
                    *dst,
                    *src,
                    *imm,
                    |x, y| x > y,
                    |x, y| x > y,
                    BinOp::Gt,
                )?,
                PackedCmpImmOp::Ge => cmp_ord_imm(
                    frame_raw,
                    regs,
                    &func.consts,
                    *dst,
                    *src,
                    *imm,
                    |x, y| x >= y,
                    |x, y| x >= y,
                    BinOp::Ge,
                )?,
            }
            None
        }
    };
    Ok(result)
}

fn decode_packed_op(code32: &[u32], pc: usize, w: u32, tag: u8) -> anyhow::Result<(Op, usize)> {
    let mut next = pc + 1;
    let reg_ext_word = if next < code32.len() && bc32::tag_of(code32[next]) == bc32::TAG_REG_EXT {
        let ext = code32[next];
        next += 1;
        Some(ext)
    } else {
        None
    };
    let (hi_a, hi_b, hi_c) = bc32::unpack_reg_ext(reg_ext_word);

    let decoded_tag = bc32::decode_tag_byte(tag);

    let op = match decoded_tag {
        bc32::DecodedTag::RegExt | bc32::DecodedTag::Ext => {
            return Err(anyhow!("bc32: unexpected standalone extension word"));
        }
        bc32::DecodedTag::Regular { tag, flags } => match tag {
            Tag::ForRangePrep => {
                let idx = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let limit = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let step = bc32::combine_reg(hi_c, (w & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for ForRangePrep"))?;
                next += 1;
                let flags = ((w2 >> 16) & 0xFF) as u8;
                let inclusive = (flags & 1) != 0;
                let explicit = (flags & 2) != 0;
                Op::ForRangePrep {
                    idx,
                    limit,
                    step,
                    inclusive,
                    explicit,
                }
            }
            Tag::ForRangeLoop => {
                let idx = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let limit = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let step = bc32::combine_reg(hi_c, (w & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for ForRangeLoop"))?;
                next += 1;
                let flags = ((w2 >> 16) & 0xFF) as u8;
                let inclusive = (flags & 1) != 0;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::ForRangeLoop {
                    idx,
                    limit,
                    step,
                    inclusive,
                    ofs,
                }
            }
            Tag::ForRangeStep => {
                let idx = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let step = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for ForRangeStep"))?;
                next += 1;
                let back_ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::ForRangeStep { idx, step, back_ofs }
            }
            Tag::JmpFalseSetX => {
                let r = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let dst = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for JmpFalseSetX"))?;
                next += 1;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::JmpFalseSet { r, dst, ofs }
            }
            Tag::JmpTrueSetX => {
                let r = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let dst = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for JmpTrueSetX"))?;
                next += 1;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::JmpTrueSet { r, dst, ofs }
            }
            Tag::NullishPickX => {
                let left = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let dst = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for NullishPickX"))?;
                next += 1;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::NullishPick { l: left, dst, ofs }
            }
            Tag::CallX => {
                let f = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let base = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let retc = (w & 0xFF) as u8;
                let w2 = *code32.get(next).ok_or_else(|| anyhow!("bc32: missing Ext for CallX"))?;
                next += 1;
                let argc = ((w2 >> 16) & 0xFF) as u8;
                Op::Call { f, base, argc, retc }
            }
            Tag::CallNamedX => {
                let f = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let base_pos = bc32::combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                let base_named = bc32::combine_reg(hi_c, (w & 0xFF) as u16);
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for CallNamedX"))?;
                next += 1;
                let posc = ((w2 >> 16) & 0xFF) as u8;
                let namedc = ((w2 >> 8) & 0xFF) as u8;
                let retc = (w2 & 0xFF) as u8;
                Op::CallNamed {
                    f,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
                }
            }
            _ => bc32::decode_word_with_hi(tag, flags, w, (hi_a, hi_b, hi_c)),
        },
    };

    Ok((op, next))
}

#[inline(always)]
fn fetch_packed_op(decoded: Option<&Bc32Decoded>, code32: &[u32], pc: usize) -> anyhow::Result<(Op, usize)> {
    if let Some(decoded_table) = decoded {
        let idx = decoded_table.word_to_instr.get(pc).copied().unwrap_or(u32::MAX);
        if idx != u32::MAX {
            let entry = &decoded_table.instrs[idx as usize];
            return Ok((entry.op, entry.next_pc));
        }
    }
    let w = code32
        .get(pc)
        .copied()
        .ok_or_else(|| anyhow!("bc32: pc {} out of bounds", pc))?;
    let tag = bc32::tag_of(w);
    if tag == bc32::TAG_REG_EXT {
        return Err(anyhow!("bc32: unexpected RegExt word at pc {}", pc));
    }
    if tag == bc32::TAG_EXT {
        return Err(anyhow!("bc32: unexpected Ext word without preceding opcode"));
    }
    decode_packed_op(code32, pc, w, tag)
}

pub(super) fn run_packed_code(
    frame_raw: *mut FrameState<'_>,
    regs: &mut Vec<Val>,
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
            if decoded.is_some() {
                pc += 1;
                continue;
            }
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!("bc32: unexpected Ext word without preceding opcode")),
            )
            .map(Some);
        }
        let mut skip_build = false;
        if let Some(entry) = packed_hot.get(pc).and_then(|slot| slot.as_ref()) {
            match entry {
                PackedHotEntry::Slot(slot) => {
                    if slot.word == word {
                        #[cfg(debug_assertions)]
                        PACKED_HOT_HITS.fetch_add(1, Ordering::Relaxed);
                        let override_pc = exec_hot_slot(slot, frame_raw, regs, f, ctx, global_ic, for_range_ic, pc)?;
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
            if let Some(entry) = packed_hot.get_mut(pc) {
                if let Some(existing) = entry {
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
            }
            #[cfg(debug_assertions)]
            PACKED_HOT_BUILD_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
            if let Some(entry) = build_hot_slot(code32, pc, word, raw_tag) {
                #[cfg(debug_assertions)]
                PACKED_HOT_BUILD_SUCCESSES.fetch_add(1, Ordering::Relaxed);
                let next_pc = entry.next_pc;
                let override_pc = exec_hot_slot(&entry, frame_raw, regs, f, ctx, global_ic, for_range_ic, pc)?;
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
                let s = regs[src as usize].to_string();
                assign_reg(frame_raw, regs, dst as usize, Val::Str(s.into()));
                pc = next_pc_default;
            }
            Op::Add(dst, a, b) => {
                if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "add", |x, y| x + y, |x, y| x + y) {
                    let out = BinOp::Add.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, out);
                }
                pc = next_pc_default;
            }
            Op::Sub(dst, a, b) => {
                if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "sub", |x, y| x - y, |x, y| x - y) {
                    let out = BinOp::Sub.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, out);
                }
                pc = next_pc_default;
            }
            Op::Mul(dst, a, b) => {
                if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "mul", |x, y| x * y, |x, y| x * y) {
                    let out = BinOp::Mul.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, out);
                }
                pc = next_pc_default;
            }
            Op::Div(dst, a, b) => {
                if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "div", |x, y| x / y, |x, y| x / y) {
                    let out = BinOp::Div.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, out);
                }
                pc = next_pc_default;
            }
            Op::Mod(dst, a, b) => {
                match (rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b)) {
                    (Val::Int(x), Val::Int(y)) => assign_reg(frame_raw, regs, dst as usize, Val::Int(x % y)),
                    _ => {
                        let out = BinOp::Mod.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                        assign_reg(frame_raw, regs, dst as usize, out);
                    }
                }
                pc = next_pc_default;
            }
            Op::AddInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x + y, BinOp::Add)?;
                pc = next_pc_default;
            }
            Op::AddFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x + y, BinOp::Add)?;
                pc = next_pc_default;
            }
            Op::AddIntImm(dst, a, imm) => {
                int_binop_imm(frame_raw, regs, &f.consts, dst, a, imm, |x, y| x + y, BinOp::Add)?;
                pc = next_pc_default;
            }
            Op::SubInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x - y, BinOp::Sub)?;
                pc = next_pc_default;
            }
            Op::SubFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x - y, BinOp::Sub)?;
                pc = next_pc_default;
            }
            Op::CmpEqImm(dst, a, imm) => {
                cmp_eq_imm(frame_raw, regs, &f.consts, dst, a, imm, BinOp::Eq)?;
                pc = next_pc_default;
            }
            Op::CmpNeImm(dst, a, imm) => {
                cmp_ne_imm(frame_raw, regs, &f.consts, dst, a, imm, BinOp::Ne)?;
                pc = next_pc_default;
            }
            Op::CmpLtImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    dst,
                    a,
                    imm,
                    |x, y| x < y,
                    |x, y| x < y,
                    BinOp::Lt,
                )?;
                pc = next_pc_default;
            }
            Op::CmpLeImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    dst,
                    a,
                    imm,
                    |x, y| x <= y,
                    |x, y| x <= y,
                    BinOp::Le,
                )?;
                pc = next_pc_default;
            }
            Op::CmpGtImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    dst,
                    a,
                    imm,
                    |x, y| x > y,
                    |x, y| x > y,
                    BinOp::Gt,
                )?;
                pc = next_pc_default;
            }
            Op::CmpGeImm(dst, a, imm) => {
                cmp_ord_imm(
                    frame_raw,
                    regs,
                    &f.consts,
                    dst,
                    a,
                    imm,
                    |x, y| x >= y,
                    |x, y| x >= y,
                    BinOp::Ge,
                )?;
                pc = next_pc_default;
            }
            Op::MulInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x * y, BinOp::Mul)?;
                pc = next_pc_default;
            }
            Op::MulFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x * y, BinOp::Mul)?;
                pc = next_pc_default;
            }
            Op::DivFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x / y, BinOp::Div)?;
                pc = next_pc_default;
            }
            Op::ModInt(dst, a, b) => {
                int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x % y, BinOp::Mod)?;
                pc = next_pc_default;
            }
            Op::ModFloat(dst, a, b) => {
                float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x % y, BinOp::Mod)?;
                pc = next_pc_default;
            }
            Op::CmpEq(dst, a, b) => {
                assign_reg(
                    frame_raw,
                    regs,
                    dst as usize,
                    Val::Bool(rk_read(regs, &f.consts, a) == rk_read(regs, &f.consts, b)),
                );
                pc = next_pc_default;
            }
            Op::CmpNe(dst, a, b) => {
                assign_reg(
                    frame_raw,
                    regs,
                    dst as usize,
                    Val::Bool(rk_read(regs, &f.consts, a) != rk_read(regs, &f.consts, b)),
                );
                pc = next_pc_default;
            }
            Op::CmpLt(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x < y, |x, y| x < y) {
                    let res = BinOp::Lt.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
                }
                pc = next_pc_default;
            }
            Op::CmpLe(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x <= y, |x, y| x <= y) {
                    let res = BinOp::Le.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
                }
                pc = next_pc_default;
            }
            Op::CmpGt(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x > y, |x, y| x > y) {
                    let res = BinOp::Gt.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
                }
                pc = next_pc_default;
            }
            Op::CmpGe(dst, a, b) => {
                if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x >= y, |x, y| x >= y) {
                    let res = BinOp::Ge.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
                }
                pc = next_pc_default;
            }
            Op::Len { dst, src } => {
                let v = &regs[src as usize];
                let out = match v {
                    Val::List(l) => Val::Int(l.len() as i64),
                    Val::Str(s) => Val::Int(s.len() as i64),
                    Val::Map(m) => Val::Int(m.len() as i64),
                    _ => Val::Int(0),
                };
                assign_reg(frame_raw, regs, dst as usize, out);
                pc = next_pc_default;
            }
            Op::Index { dst, base, idx } => {
                let res = match (&regs[base as usize], &regs[idx as usize]) {
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
                                };
                                Vm::update_str_ic(index_ic.as_mut_slice(), pc, sptr, *i, &v);
                                v
                            }
                        }
                    }
                    _ => Val::Nil,
                };
                assign_reg(frame_raw, regs, dst as usize, res);
                pc = next_pc_default;
            }
            Op::Jmp(ofs) => {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            }
            Op::JmpFalse(r, ofs) => {
                let cond_falsey = matches!(regs[r as usize], Val::Nil | Val::Bool(false));
                if cond_falsey {
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                } else {
                    pc = next_pc_default;
                }
            }
            Op::JmpIfNil(r, ofs) => {
                if matches!(regs[r as usize], Val::Nil) {
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                } else {
                    pc = next_pc_default;
                }
            }
            Op::JmpIfNotNil(r, ofs) => {
                if !matches!(regs[r as usize], Val::Nil) {
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                } else {
                    pc = next_pc_default;
                }
            }
            Op::ToBool(dst, src) => {
                let truthy = !matches!(regs[src as usize], Val::Nil | Val::Bool(false));
                assign_reg(frame_raw, regs, dst as usize, Val::Bool(truthy));
                pc = next_pc_default;
            }
            Op::Not(dst, src) => {
                match &regs[src as usize] {
                    Val::Bool(b) => assign_reg(frame_raw, regs, dst as usize, Val::Bool(!b)),
                    other => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("Invalid operand: !{:?}", other)))
                            .map(Some);
                    }
                }
                pc = next_pc_default;
            }
            Op::NullishPick { l, dst, ofs } => {
                if !matches!(regs[l as usize], Val::Nil) {
                    assign_reg(frame_raw, regs, dst as usize, regs[l as usize].clone());
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                } else {
                    pc = next_pc_default;
                }
            }
            Op::Ret { base, retc } => {
                let retc = retc as usize;
                let base_idx = base as usize;
                let ret_val = if retc > 0 { regs[base_idx].clone() } else { Val::Nil };
                return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr).map(Some);
            }
            Op::Break(ofs) => {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            }
            Op::Continue(ofs) => {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            }
            Op::LoadGlobal(dst, name_k) => {
                let name_val = &f.consts[name_k as usize];
                let mut out = Val::Nil;
                if let Val::Str(s) = name_val {
                    let key_ptr = s.as_ref().as_ptr() as usize;
                    let cur_gen = ctx.generation();
                    let local_shadowed = ctx.is_local_name(s.as_ref());
                    if !local_shadowed {
                        if let Some(GlobalEntry(ptr, v, generation)) = &global_ic[pc]
                            && *ptr == key_ptr
                            && *generation == cur_gen
                        {
                            out = v.clone();
                        } else if let Some(v) = ctx.get(s.as_ref()) {
                            out = v.clone();
                            global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                        }
                    }
                    if matches!(out, Val::Nil)
                        && let Some(v) = ctx.get_value(s.as_ref())
                    {
                        out = v;
                        if !local_shadowed {
                            global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                        }
                    }
                    if matches!(out, Val::Nil)
                        && let Some(builtin) = ctx.resolver().get_builtin(s.as_ref())
                    {
                        out = builtin.clone();
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
                assign_reg(frame_raw, regs, dst as usize, out);
                pc = next_pc_default;
            }
            Op::DefineGlobal(name_k, src) => {
                let name_val = &f.consts[name_k as usize];
                if let Val::Str(s) = name_val {
                    ctx.set(s.as_ref().to_owned(), regs[src as usize].clone());
                }
                pc = next_pc_default;
            }
            Op::Access(dst, base, field) => {
                let hit_val = match (&regs[base as usize], &regs[field as usize]) {
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
                        let kstr = s.as_ref();
                        match access_ic[pc].as_mut() {
                            Some(AccessIc::ObjectStr(slots)) => {
                                Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == kstr)
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
                    let v = regs[base as usize].access(&regs[field as usize]).unwrap_or(Val::Nil);
                    match (&regs[base as usize], &regs[field as usize]) {
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
                assign_reg(frame_raw, regs, dst as usize, res);
                pc = next_pc_default;
            }
            Op::AccessK(dst, base, kidx) => {
                let key = &f.consts[kidx as usize];
                let res = if let Val::Str(s) = key {
                    let (hit_val, mp, kp, obj_ptr) = match &regs[base as usize] {
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
                        let v = regs[base as usize].access(key).unwrap_or(Val::Nil);
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
                assign_reg(frame_raw, regs, dst as usize, res);
                pc = next_pc_default;
            }
            Op::IndexK(dst, base, kidx) => {
                let key = &f.consts[kidx as usize];
                let res = if let Val::Int(i) = key {
                    match &regs[base as usize] {
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
                assign_reg(frame_raw, regs, dst as usize, res);
                pc = next_pc_default;
            }
            Op::ForRangePrep {
                idx,
                limit,
                step,
                inclusive,
                explicit,
            } => {
                let idx_reg = idx as usize;
                let limit_reg = limit as usize;
                let step_reg = step as usize;
                let (i0, ilim) = match (&regs[idx_reg], &regs[limit_reg]) {
                    (Val::Int(a0), Val::Int(b0)) => (*a0, *b0),
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
                let step_val = if !explicit {
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
                if let Some(slot) = for_range_ic.get_mut(next_pc_default) {
                    *slot = Some(ForRangeState::new(i0, ilim, step_val, inclusive));
                }
                pc = next_pc_default;
            }
            Op::ForRangeLoop { idx, ofs, .. } => {
                let idx_reg = idx as usize;
                let state_entry = match fetch_for_range_state(for_range_ic, pc) {
                    Ok(state) => state,
                    Err(err) => {
                        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                    }
                };
                if state_entry.should_continue() {
                    assign_reg(frame_raw, regs, idx_reg, Val::Int(state_entry.current));
                    pc = next_pc_default;
                } else {
                    for_range_ic[pc] = None;
                    pc = ((pc as isize) + (ofs as isize)) as usize;
                }
            }
            Op::ForRangeStep { back_ofs, .. } => {
                let guard_pc = ((pc as isize) + (back_ofs as isize)) as usize;
                let state_entry = match fetch_for_range_state(for_range_ic, guard_pc) {
                    Ok(state) => state,
                    Err(err) => {
                        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                    }
                };
                state_entry.current += state_entry.step;
                pc = guard_pc;
            }
            Op::PatternMatch { dst, src, plan } => {
                let plan = &f.pattern_plans[plan as usize];
                let value = &regs[src as usize];
                match plan.pattern.matches(value, Some(&*ctx))? {
                    Some(bound) => {
                        for binding in &plan.bindings {
                            if let Some((_, v)) = bound.iter().find(|(name, _)| name == &binding.name) {
                                assign_reg(frame_raw, regs, binding.reg as usize, v.clone());
                            } else {
                                assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                            }
                        }
                        assign_reg(frame_raw, regs, dst as usize, Val::Bool(true));
                    }
                    None => {
                        for binding in &plan.bindings {
                            assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                        }
                        assign_reg(frame_raw, regs, dst as usize, Val::Bool(false));
                    }
                }
                pc = next_pc_default;
            }
            Op::PatternMatchOrFail {
                src,
                plan,
                err_kidx,
                is_const,
            } => {
                let plan = &f.pattern_plans[plan as usize];
                let value = &regs[src as usize];
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
                            if is_const {
                                ctx.define_const(name, val);
                            } else {
                                ctx.set(name, val);
                            }
                        }
                        pc = next_pc_default;
                    }
                    None => {
                        let msg_val = &f.consts[err_kidx as usize];
                        let msg = match msg_val {
                            Val::Str(s) => s.as_ref().to_string(),
                            other => other.to_string(),
                        };
                        return frame_return_common(frame_raw, pc, Err(anyhow!(msg))).map(Some);
                    }
                }
            }
            Op::Raise { err_kidx } => {
                let msg_val = &f.consts[err_kidx as usize];
                let msg = match msg_val {
                    Val::Str(s) => s.as_ref().to_string(),
                    other => other.to_string(),
                };
                return frame_return_common(frame_raw, pc, Err(anyhow!(msg))).map(Some);
            }
            Op::BuildList { dst, base, len } => {
                let start = base as usize;
                let n = len as usize;
                let use_thread_local = region_plan
                    .as_ref()
                    .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                    .unwrap_or(false);
                if use_thread_local {
                    let allocator = unsafe { &*region_allocator_ptr };
                    let list_val = allocator.with_val_buffer(n, |scratch| {
                        scratch.extend((0..n).map(|i| regs[start + i].clone()));
                        let data = scratch.split_off(0);
                        Val::List(data.into())
                    });
                    assign_reg(frame_raw, regs, dst as usize, list_val);
                } else {
                    let mut v = Vec::with_capacity(n);
                    for i in 0..n {
                        v.push(regs[start + i].clone());
                    }
                    assign_reg(frame_raw, regs, dst as usize, Val::List(v.into()));
                }
                pc = next_pc_default;
            }
            Op::BuildMap { dst, base, len } => {
                let start = base as usize;
                let n = len as usize;
                let use_thread_local = region_plan
                    .as_ref()
                    .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                    .unwrap_or(false);
                if use_thread_local {
                    let allocator = unsafe { &*region_allocator_ptr };
                    let map_val = allocator.with_map_entries(n, |entries| {
                        for i in 0..n {
                            let key_arc;
                            let value;
                            {
                                let key_val = &regs[start + 2 * i];
                                value = regs[start + 2 * i + 1].clone();
                                key_arc = match key_val {
                                    Val::Str(s) => s.clone(),
                                    Val::Int(i) => Arc::from(i.to_string()),
                                    Val::Float(f) => Arc::from(f.to_string()),
                                    Val::Bool(b) => Arc::from(b.to_string()),
                                    _ => {
                                        return Err(anyhow!("Map key must be a primitive type, got: {:?}", key_val));
                                    }
                                };
                            }
                            entries.push((key_arc, value));
                        }
                        let mut map = fast_hash_map_with_capacity(entries.len());
                        for (k, v) in entries.drain(..) {
                            map.insert(k, v);
                        }
                        Ok(Val::Map(Arc::new(map)))
                    });
                    match map_val {
                        Ok(val) => assign_reg(frame_raw, regs, dst as usize, val),
                        Err(err) => {
                            return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                        }
                    }
                } else {
                    let mut map: FastHashMap<Arc<str>, Val> = fast_hash_map_with_capacity(n);
                    for i in 0..n {
                        let key_arc;
                        let value;
                        {
                            let key_val = &regs[start + 2 * i];
                            value = regs[start + 2 * i + 1].clone();
                            key_arc = match key_val {
                                Val::Str(s) => s.clone(),
                                Val::Int(i) => Arc::from(i.to_string()),
                                Val::Float(f) => Arc::from(f.to_string()),
                                Val::Bool(b) => Arc::from(b.to_string()),
                                _ => {
                                    return frame_return_common(
                                        frame_raw,
                                        pc,
                                        Err(anyhow!("Map key must be a primitive type, got: {:?}", key_val)),
                                    )
                                    .map(Some);
                                }
                            };
                        }
                        map.insert(key_arc, value);
                    }
                    assign_reg(frame_raw, regs, dst as usize, Val::Map(Arc::new(map)));
                }
                pc = next_pc_default;
            }
            Op::MakeClosure { dst, proto } => {
                let p = f
                    .protos
                    .get(proto as usize)
                    .ok_or_else(|| anyhow!("closure proto out of range"))?;
                // Use snapshot of ctx as captured environment
                let captured_env = Arc::new(ctx.snapshot());
                let captures = if p.captures.is_empty() {
                    ClosureCapture::empty()
                } else {
                    let mut names: Vec<String> = Vec::with_capacity(p.captures.len());
                    let mut values: Vec<Val> = Vec::with_capacity(p.captures.len());
                    for spec in &p.captures {
                        match spec {
                            CaptureSpec::Register { name, src } => {
                                let idx = frame_base + (*src as usize);
                                let val = regs.get(idx).cloned().unwrap_or(Val::Nil);
                                names.push(name.clone());
                                values.push(val);
                            }
                            CaptureSpec::Const { name, kidx } => {
                                let val = f.consts.get(*kidx as usize).cloned().unwrap_or(Val::Nil);
                                names.push(name.clone());
                                values.push(val);
                            }
                            CaptureSpec::Global { name } => {
                                let val = ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil);
                                names.push(name.clone());
                                values.push(val);
                            }
                        }
                    }
                    ClosureCapture::from_pairs(names, values)
                };
                let mut clo = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
                    params: Arc::new(p.params.clone()),
                    named_params: Arc::new(p.named_params.clone()),
                    body: Arc::new(p.body.clone()),
                    env: captured_env,
                    upvalues: Arc::new(Vec::new()),
                    captures,
                    capture_specs: Arc::new(p.captures.clone()),
                    default_funcs: Arc::new(p.default_funcs.clone()),
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
                // Seed precompiled code when available for fast-path execution
                if let Val::Closure(closure_arc) = &clo
                    && let Some(inner) = &p.func
                {
                    let _ = closure_arc.code.set((**inner).clone());
                }
                assign_reg(frame_raw, regs, dst as usize, clo);
                pc = next_pc_default;
            }
            Op::LoadLocal(dst, idx) => {
                assign_reg(frame_raw, regs, dst as usize, regs[idx as usize].clone());
                pc = next_pc_default;
            }
            Op::StoreLocal(idx, src) => {
                let v = regs[src as usize].clone();
                assign_reg(frame_raw, regs, idx as usize, v);
                pc = next_pc_default;
            }
            Op::Call {
                f: rf,
                base,
                argc,
                retc,
            } => {
                let resume_pc = next_pc_default;
                let _current_vm_guard = VmCurrentGuard::new(self_ptr, ctx as *mut VmContext);
                let func = regs[rf as usize].clone();
                let start = base as usize;
                let n = argc as usize;
                let args_slice = &regs[start..start + n];
                let call_args = CallArgs::registers(RegisterSpan::current(start, n));
                let allocator = unsafe { &*region_allocator_ptr };
                let mut next_pc = resume_pc;
                match &func {
                    Val::Closure(closure_arc) => {
                        let closure_ptr = Arc::as_ptr(closure_arc) as usize;
                        let cached_fast = matches!(call_ic[pc as usize].as_ref(), Some(CallIc::ClosurePositional { closure_ptr: cached_ptr, argc: cached_argc, .. }) if *cached_ptr == closure_ptr && *cached_argc == argc);
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
                                caller_window: RegisterWindowRef::Current,
                            };
                            let closure = closure_arc.as_ref();
                            let mut cached_fun_ptr = None;
                            if let Some(CallIc::ClosurePositional {
                                closure_ptr: cached_ptr,
                                fun_ptr,
                                argc: cached_argc,
                                ..
                            }) = call_ic[pc as usize].as_ref()
                                && *cached_ptr == closure_ptr
                                && *cached_argc == argc
                            {
                                cached_fun_ptr = Some(*fun_ptr);
                            }
                            let fun: &Function = if let Some(ptr) = cached_fun_ptr {
                                unsafe { &*ptr }
                            } else {
                                closure.code.get_or_init(|| {
                                    let c = Compiler::new();
                                    c.compile_function_with_captures(
                                        closure.params.as_ref(),
                                        closure.named_params.as_ref(),
                                        closure.body.as_ref(),
                                        closure.capture_specs.as_ref(),
                                    )
                                })
                            };
                            let vm_mut = unsafe { &mut *self_ptr };
                            if let Some(CallIc::ClosurePositional {
                                closure_ptr: _,
                                fun_ptr: _,
                                argc: _,
                                cache,
                                frame_info,
                            }) = call_ic[pc as usize].as_mut()
                                && cached_fast
                            {
                                match vm_mut.exec_function_positional_fast(
                                    fun,
                                    args_slice,
                                    ctx,
                                    Some(&*frame_info),
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
                                match vm_mut.exec_function_positional_fast(
                                    fun,
                                    args_slice,
                                    ctx,
                                    Some(&frame_info),
                                    Some(&mut cache),
                                    Some(return_meta),
                                ) {
                                    Ok(val) => {
                                        if retc > 0 {
                                            assign_reg(frame_raw, regs, base as usize, val);
                                        }
                                        call_ic[pc as usize] = Some(CallIc::ClosurePositional {
                                            closure_ptr,
                                            fun_ptr: fun as *const Function,
                                            argc,
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
                                c.compile_function_with_captures(
                                    closure.params.as_ref(),
                                    closure.named_params.as_ref(),
                                    closure.body.as_ref(),
                                    closure.capture_specs.as_ref(),
                                )
                            });
                            let frame_info = closure.frame_info();
                            let captures_arc = Arc::clone(&closure.captures);
                            let capture_specs_arc = Arc::clone(&closure.capture_specs);
                            let call_result = if closure.named_params.is_empty() {
                                Vm::exec_function_with_args(
                                    fun,
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
                                            let default_val =
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
                                                })?;
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
                        caller_window: RegisterWindowRef::Current,
                    },
                );
                let _current_vm_guard = VmCurrentGuard::new(self_ptr, ctx as *mut VmContext);
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
                            c.compile_function_with_captures(
                                closure.params.as_ref(),
                                closure.named_params.as_ref(),
                                closure.body.as_ref(),
                                closure.capture_specs.as_ref(),
                            )
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
                                    let default_val = allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
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
                                    })?;
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
