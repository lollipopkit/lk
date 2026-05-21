//! 32-bit packed bytecode encoding scaffold.
//!
use super::bytecode::rk_index;
use super::bytecode::{ClosureProto, Function, NamedParamLayoutEntry, Op, PatternPlan, rk_is_const};
use crate::val::Val;
use std::sync::Arc;
use tracing::info;

mod compare;
mod decoded;
mod encode_support;
mod format;
mod function_decode;
mod metrics;
pub use decoded::*;
use encode_support::*;
pub(crate) use format::*;
pub use metrics::*;
#[derive(Debug, Clone)]
pub struct Bc32Function {
    pub consts: Vec<Val>,
    pub code32: Vec<u32>,
    pub decoded: Option<Arc<Bc32Decoded>>,
    pub n_regs: u16,
    pub protos: Vec<ClosureProto>,
    pub param_regs: Vec<u16>,
    pub named_param_regs: Vec<u16>,
    pub named_param_layout: Vec<NamedParamLayoutEntry>,
    pub pattern_plans: Vec<PatternPlan>,
}

const TRACE_TARGET: &str = "lk::vm::bc32";

#[inline]
fn pack_rk_binary(tag: Tag, d: u16, a: u16, b: u16) -> EncodedOp {
    let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
    let word = pack(tag, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
    let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
    EncodedOp::new(word, reg_ext)
}

#[inline]
fn pack_typed_arith_or_rk(tag: Tag, ext_op: u8, d: u16, a: u16, b: u16) -> Result<EncodedOp, Bc32Reject> {
    if rk_is_const(a) || rk_is_const(b) {
        return Ok(pack_rk_binary(tag, d, a, b));
    }
    pack_ext_op(ext_op, d, a, b)
}

fn encode_op(op: &Op) -> Result<EncodedOp, Bc32Reject> {
    if let Some(encoded) = compare::encode_compare_op(op) {
        return encoded;
    }
    match *op {
        Op::Nop => Ok(EncodedOp::new(pack(Tag::Move, 1, 0, 0, 0), None)),
        Op::AddRangeCountImm { .. } => Err(Bc32Reject::UnsupportedOpcode {
            opcode: "AddRangeCountImm",
            detail: "runtime range aggregate is currently opcode-only",
        }),
        Op::ListFoldAdd { .. } => Err(Bc32Reject::UnsupportedOpcode {
            opcode: "ListFoldAdd",
            detail: "list fold is currently opcode-only",
        }),
        Op::MapValuesFoldAdd { .. } => Err(Bc32Reject::UnsupportedOpcode {
            opcode: "MapValuesFoldAdd",
            detail: "map values fold is currently opcode-only",
        }),
        Op::Move(d, s) => {
            let word = pack(Tag::Move, 0, d as u8, s as u8, 0);
            let reg_ext = pack_reg_ext_bits(d, s, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::LoadK(d, k) => {
            ensure_u8("LoadK", "const", k)?;
            let word = pack(Tag::LoadK, 0, d as u8, k as u8, 0);
            let reg_ext = pack_reg_ext_bits(d, k, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::AddInt(d, a, b) => pack_typed_arith_or_rk(Tag::Add, EXT_OP_ADD_INT, d, a, b),
        Op::AddFloat(d, a, b) => pack_typed_arith_or_rk(Tag::Add, EXT_OP_ADD_FLOAT, d, a, b),
        Op::StrConcatKnownCap(d, a, b) => pack_ext_op(EXT_OP_STR_CONCAT_KNOWN_CAP, d, a, b),
        Op::StrConcatToStr(d, lhs, src) => pack_ext_op(EXT_OP_STR_CONCAT_TO_STR, d, lhs, src),
        Op::Add(d, a, b) => {
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Add, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::AddIntImm(d, a, imm) => {
            ensure_i8_range("AddIntImm", "imm", imm as i32)?;
            let word = pack(Tag::AddIntImm, 0, d as u8, a as u8, (imm as i8) as u8);
            let reg_ext = pack_reg_ext_bits(d, a, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::SubInt(d, a, b) => pack_typed_arith_or_rk(Tag::Sub, EXT_OP_SUB_INT, d, a, b),
        Op::SubFloat(d, a, b) => pack_typed_arith_or_rk(Tag::Sub, EXT_OP_SUB_FLOAT, d, a, b),
        Op::Sub(d, a, b) => {
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Sub, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::MulInt(d, a, b) => pack_typed_arith_or_rk(Tag::Mul, EXT_OP_MUL_INT, d, a, b),
        Op::MulFloat(d, a, b) => pack_typed_arith_or_rk(Tag::Mul, EXT_OP_MUL_FLOAT, d, a, b),
        Op::Mul(d, a, b) => {
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Mul, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::DivFloat(d, a, b) => pack_typed_arith_or_rk(Tag::Div, EXT_OP_DIV_FLOAT, d, a, b),
        Op::Div(d, a, b) => {
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Div, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ModInt(d, a, b) => pack_typed_arith_or_rk(Tag::Mod, EXT_OP_MOD_INT, d, a, b),
        Op::ModFloat(d, a, b) => pack_typed_arith_or_rk(Tag::Mod, EXT_OP_MOD_FLOAT, d, a, b),
        Op::Mod(d, a, b) => {
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Mod, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpI { dst, a, b, kind } => Ok(pack_cmp_i(dst, a, b, kind)),
        Op::CmpIntJmp { kind, a, b, ofs } => Ok(pack_cmp_i_jmp(a, b, kind, ofs)),
        Op::Jmp(ofs) => Ok(EncodedOp::new(
            ((encode_tag_with_flags(Tag::Jmp, 0) as u32) << 24) | (ofs as i32 as u32 & 0x00FF_FFFF),
            None,
        )),
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => {
            let (hi, lo) = encode_i16(ofs);
            let word = pack(Tag::JmpFalse, 0, r as u8, hi, lo);
            let reg_ext = pack_reg_ext_bits(r, 0, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ToBool(d, s) => {
            let word = pack(Tag::ToBool, 0, d as u8, s as u8, 0);
            let reg_ext = pack_reg_ext_bits(d, s, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ToStr(d, s) => {
            let word = pack(Tag::ToStr, 0, d as u8, s as u8, 0);
            let reg_ext = pack_reg_ext_bits(d, s, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Not(d, s) => {
            let word = pack(Tag::Not, 0, d as u8, s as u8, 0);
            let reg_ext = pack_reg_ext_bits(d, s, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Len { dst, src } => {
            let word = pack(Tag::Len, 0, dst as u8, src as u8, 0);
            let reg_ext = pack_reg_ext_bits(dst, src, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ListLen { dst, src } => pack_ext_op(EXT_OP_LIST_LEN, dst, src, 0),
        Op::MapLen { dst, src } => pack_ext_op(EXT_OP_MAP_LEN, dst, src, 0),
        Op::StrLen { dst, src } => pack_ext_op(EXT_OP_STR_LEN, dst, src, 0),
        Op::Floor { dst, src } => pack_ext_op(EXT_OP_FLOOR, dst, src, 0),
        Op::FloorDivImm { dst, src, imm } => pack_ext_op_i8(EXT_OP_FLOOR_DIV_IMM, "FloorDivImm", dst, src, imm),
        Op::StartsWithK(dst, src, kidx) => pack_ext_op(EXT_OP_STARTS_WITH_K, dst, src, kidx),
        Op::ContainsK(dst, src, kidx) => pack_ext_op(EXT_OP_CONTAINS_K, dst, src, kidx),
        Op::ToIter { dst, src } => pack_ext_op(EXT_OP_TO_ITER, dst, src, 0),
        Op::MapGetInterned(dst, map, kidx) => pack_ext_op(EXT_OP_MAP_GET_INTERNED, dst, map, kidx),
        Op::MapGetDynamic(dst, map, key) => pack_ext_op(EXT_OP_MAP_GET_DYNAMIC, dst, map, key),
        Op::MapSetInterned(map, kidx, val) => pack_ext_op(EXT_OP_MAP_SET_INTERNED, map, kidx, val),
        Op::MapSetInternedMove(map, kidx, val) => pack_ext_op(EXT_OP_MAP_SET_INTERNED_MOVE, map, kidx, val),
        Op::MapHas(dst, map, key) => pack_ext_op(EXT_OP_MAP_HAS, dst, map, key),
        Op::MapHasK(dst, map, kidx) => pack_ext_op(EXT_OP_MAP_HAS_K, dst, map, kidx),
        Op::ListSetI { dst, list, index, val } => pack_ext_op_i16_reg(EXT_OP_LIST_SET_I, dst, list, index, val),
        Op::Index { dst, base, idx } => {
            let word = pack(Tag::Index, 0, dst as u8, base as u8, idx as u8);
            let reg_ext = pack_reg_ext_bits(dst, base, idx);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::JmpIfNil(r, ofs) => {
            let (hi, lo) = encode_i16(ofs);
            let word = pack(Tag::JmpIfNil, 0, r as u8, hi, lo);
            let reg_ext = pack_reg_ext_bits(r, 0, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::JmpIfNotNil(r, ofs) => {
            let (hi, lo) = encode_i16(ofs);
            let word = pack(Tag::JmpIfNotNil, 0, r as u8, hi, lo);
            let reg_ext = pack_reg_ext_bits(r, 0, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::NullishPick { l, dst, ofs } => {
            ensure_regs_u8("NullishPick", l, dst, 0)?;
            ensure_i8_range("NullishPick", "ofs", ofs as i32)?;
            let word = pack(Tag::NullishPick, 0, l as u8, dst as u8, (ofs as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::Ret { base, retc } => {
            let word = pack(Tag::Ret, 0, base as u8, retc, 0);
            let reg_ext = pack_reg_ext_bits(base, 0, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::LoadGlobal(dst, k) => {
            ensure_u8("LoadGlobal", "const", k)?;
            let word = pack(Tag::LoadGlobal, 0, dst as u8, k as u8, 0);
            let reg_ext = pack_reg_ext_bits(dst, k, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::DefineGlobal(k, src) => {
            ensure_u8("DefineGlobal", "name", k)?;
            let word = pack(Tag::DefineGlobal, 0, k as u8, src as u8, 0);
            let reg_ext = pack_reg_ext_bits(k, src, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Access(d, b, f) => {
            let word = pack(Tag::Access, 0, d as u8, b as u8, f as u8);
            let reg_ext = pack_reg_ext_bits(d, b, f);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::AccessK(d, b, k) => {
            ensure_u8("AccessK", "key", k)?;
            let word = pack(Tag::AccessK, 0, d as u8, b as u8, k as u8);
            let reg_ext = pack_reg_ext_bits(d, b, k);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::IndexK(d, b, k) => {
            ensure_u8("IndexK", "key", k)?;
            let word = pack(Tag::IndexK, 0, d as u8, b as u8, k as u8);
            let reg_ext = pack_reg_ext_bits(d, b, k);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ListIndexI(dst, base, index) => pack_ext_op_i8(EXT_OP_LIST_INDEX_I, "ListIndexI", dst, base, index),
        Op::StrIndexI(dst, base, index) => pack_ext_op_i8(EXT_OP_STR_INDEX_I, "StrIndexI", dst, base, index),
        Op::LoadLocal(d, i) => {
            let word = pack(Tag::LoadLocal, 0, d as u8, i as u8, 0);
            let reg_ext = pack_reg_ext_bits(d, i, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::StoreLocal(i, s) => {
            let word = pack(Tag::StoreLocal, 0, i as u8, s as u8, 0);
            let reg_ext = pack_reg_ext_bits(i, s, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Call { f, base, argc, retc } => {
            if retc != 1 || pack_reg_ext_bits(f, base, 0).is_some() {
                return Err(Bc32Reject::UnsupportedOpcode {
                    opcode: "Call",
                    detail: "requires CallX",
                });
            }
            ensure_regs_u8("Call", f, base, 0)?;
            let word = pack(Tag::Call, 0, f as u8, base as u8, argc);
            Ok(EncodedOp::new(word, None))
        }
        Op::CallClosureExact { f, base, argc, retc } => {
            pack_call_ext(EXT_OP_CALL_CLOSURE_EXACT, "CallClosureExact", f, base, argc, retc)
        }
        Op::CallExact { f, base, argc, retc } => pack_call_ext(EXT_OP_CALL_EXACT, "CallExact", f, base, argc, retc),
        Op::CallNativeFast { f, base, argc, retc } => {
            pack_call_ext(EXT_OP_CALL_NATIVE_FAST, "CallNativeFast", f, base, argc, retc)
        }
        Op::CallMethod0 { dst, receiver, method } => pack_ext_op(EXT_OP_CALL_METHOD0, dst, receiver, method),
        Op::CallGlobalMethod0 { dst, receiver, method } => {
            pack_ext_op(EXT_OP_CALL_GLOBAL_METHOD0, dst, receiver, method)
        }
        Op::CallNamedFallback {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            retc,
        } => Ok(pack_call_named_fallback(f, base_pos, posc, base_named, namedc, retc)),
        Op::LoadCapture { dst, idx } => {
            ensure_regs_u8("LoadCapture", dst, 0, 0)?;
            ensure_u8("LoadCapture", "idx", idx)?;
            let word = pack(Tag::LoadCapture, 0, dst as u8, idx as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::JmpFalseSet { r, dst, ofs } => {
            ensure_i8_range("JmpFalseSet", "ofs", ofs as i32)?;
            ensure_regs_u8("JmpFalseSet", r, dst, 0)?;
            let word = pack(Tag::JmpFalseSet, 0, r as u8, dst as u8, (ofs as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::JmpTrueSet { r, dst, ofs } => {
            ensure_i8_range("JmpTrueSet", "ofs", ofs as i32)?;
            ensure_regs_u8("JmpTrueSet", r, dst, 0)?;
            let word = pack(Tag::JmpTrueSet, 0, r as u8, dst as u8, (ofs as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::ListSlice { dst, src, start } => {
            let word = pack(Tag::ListSlice, 0, dst as u8, src as u8, start as u8);
            let reg_ext = pack_reg_ext_bits(dst, src, start);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ListPush { list, val } => {
            let word = pack(Tag::ListPush, 0, list as u8, val as u8, 0);
            let reg_ext = pack_reg_ext_bits(list, val, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::ListPushMove { list, val } => {
            let word = pack(Tag::ListPush, 1, list as u8, val as u8, 0);
            let reg_ext = pack_reg_ext_bits(list, val, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::MapSet { map, key, val } => {
            let word = pack(Tag::MapSet, 0, map as u8, key as u8, val as u8);
            let reg_ext = pack_reg_ext_bits(map, key, val);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::MapSetMove { map, key, val } => {
            let word = pack(Tag::MapSet, 1, map as u8, key as u8, val as u8);
            let reg_ext = pack_reg_ext_bits(map, key, val);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::BuildList { dst, base, len } => {
            let word = pack(Tag::BuildList, 0, dst as u8, base as u8, len as u8);
            let reg_ext = pack_reg_ext_bits(dst, base, len);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::BuildMap { dst, base, len } => {
            let word = pack(Tag::BuildMap, 0, dst as u8, base as u8, len as u8);
            let reg_ext = pack_reg_ext_bits(dst, base, len);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::MakeClosure { dst, proto } => {
            ensure_u8("MakeClosure", "proto", proto)?;
            let word = pack(Tag::MakeClosure, 0, dst as u8, proto as u8, 0);
            let reg_ext = pack_reg_ext_bits(dst, proto, 0);
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Break(ofs) => Ok(EncodedOp::new(
            ((encode_tag_with_flags(Tag::Break, 0) as u32) << 24) | (ofs as i32 as u32 & 0x00FF_FFFF),
            None,
        )),
        Op::Continue(ofs) => Ok(EncodedOp::new(
            ((encode_tag_with_flags(Tag::Continue, 0) as u32) << 24) | (ofs as i32 as u32 & 0x00FF_FFFF),
            None,
        )),
        Op::PatternMatch { dst, src, plan } => {
            ensure_regs_u8("PatternMatch", dst, src, plan)?;
            let word = pack(Tag::PatternMatch, 0, dst as u8, src as u8, plan as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::PatternMatchOrFail {
            src,
            plan,
            err_kidx,
            is_const,
        } => {
            let tag = if is_const {
                Tag::PatternMatchOrFailConst
            } else {
                Tag::PatternMatchOrFail
            };
            ensure_regs_u8("PatternMatchOrFail", src, plan, err_kidx)?;
            let word = pack(tag, 0, src as u8, plan as u8, err_kidx as u8);
            Ok(EncodedOp::new(word, None))
        }
        _ => Err(Bc32Reject::UnsupportedOpcode {
            opcode: opcode_name(op),
            detail: "not_supported",
        }),
    }
}

#[inline]
fn sign_extend_24(x: u32) -> i32 {
    ((x as i32) << 8) >> 8
}

pub(crate) fn decode_word_with_hi(tag: Tag, flags: u8, w: u32, hi: (u16, u16, u16)) -> Op {
    let lo_a = ((w >> 16) & 0xFF) as u16;
    let lo_b = ((w >> 8) & 0xFF) as u16;
    let lo_c = (w & 0xFF) as u16;
    let (hi_a, hi_b, hi_c) = hi;
    let a = combine_reg(hi_a, lo_a);
    let b_reg = combine_reg(hi_b, lo_b);
    let c_reg = combine_reg(hi_c, lo_c);
    let b_rk = combine_rk(hi_b, lo_b, (flags & RK_FLAG_B) != 0);
    let c_rk = combine_rk(hi_c, lo_c, (flags & RK_FLAG_C) != 0);
    match tag {
        Tag::Move if (flags & TAG_FLAG_MASK) == 1 => Op::Nop,
        Tag::Move => Op::Move(a, b_reg),
        Tag::LoadK => Op::LoadK(a, b_reg),
        Tag::Add => Op::Add(a, b_rk, c_rk),
        Tag::Sub => Op::Sub(a, b_rk, c_rk),
        Tag::Mul => Op::Mul(a, b_rk, c_rk),
        Tag::Div => Op::Div(a, b_rk, c_rk),
        Tag::Mod => Op::Mod(a, b_rk, c_rk),
        Tag::AddIntImm => Op::AddIntImm(a, b_reg, (lo_c as i8) as i16),
        Tag::Eq => Op::CmpEq(a, b_rk, c_rk),
        Tag::Ne => Op::CmpNe(a, b_rk, c_rk),
        Tag::Lt => Op::CmpLt(a, b_rk, c_rk),
        Tag::Le => Op::CmpLe(a, b_rk, c_rk),
        Tag::Gt => Op::CmpGt(a, b_rk, c_rk),
        Tag::Ge => Op::CmpGe(a, b_rk, c_rk),
        Tag::CmpEqImm => Op::CmpEqImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpNeImm => Op::CmpNeImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpLtImm => Op::CmpLtImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpLeImm => Op::CmpLeImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpGtImm => Op::CmpGtImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpGeImm => Op::CmpGeImm(a, b_reg, (lo_c as i8) as i16),
        Tag::Jmp => Op::Jmp(sign_extend_24(w) as i16),
        Tag::JmpFalse => Op::BoolBranch(a, ((b_reg << 8) | c_reg) as i16),
        Tag::ToBool => Op::ToBool(a, b_reg),
        Tag::ToStr => Op::ToStr(a, b_reg),
        Tag::Not => Op::Not(a, b_reg),
        Tag::Len => Op::Len { dst: a, src: b_reg },
        Tag::Index => Op::Index {
            dst: a,
            base: b_reg,
            idx: c_reg,
        },
        Tag::JmpIfNil => Op::JmpIfNil(a, ((b_reg << 8) | c_reg) as i16),
        Tag::JmpIfNotNil => Op::JmpIfNotNil(a, ((b_reg << 8) | c_reg) as i16),
        Tag::NullishPick => Op::NullishPick {
            l: a,
            dst: b_reg,
            ofs: (c_reg as i8) as i16,
        },
        Tag::Ret => Op::Ret {
            base: a,
            retc: b_reg as u8,
        },
        Tag::LoadGlobal => Op::LoadGlobal(a, b_reg),
        Tag::DefineGlobal => Op::DefineGlobal(a, b_reg),
        Tag::Access => Op::Access(a, b_reg, c_reg),
        Tag::AccessK => Op::AccessK(a, b_reg, c_reg),
        Tag::IndexK => Op::IndexK(a, b_reg, c_reg),
        Tag::LoadLocal => Op::LoadLocal(a, b_reg),
        Tag::StoreLocal => Op::StoreLocal(a, b_reg),
        Tag::Call => Op::Call {
            f: a,
            base: b_reg,
            argc: c_reg as u8,
            retc: 1,
        },
        Tag::LoadCapture => Op::LoadCapture { dst: a, idx: b_reg },
        Tag::JmpFalseSet => Op::JmpFalseSet {
            r: a,
            dst: b_reg,
            ofs: (c_reg as i8) as i16,
        },
        Tag::JmpTrueSet => Op::JmpTrueSet {
            r: a,
            dst: b_reg,
            ofs: (c_reg as i8) as i16,
        },
        Tag::ListSlice => Op::ListSlice {
            dst: a,
            src: b_reg,
            start: c_reg,
        },
        Tag::ListPush if flags & 1 != 0 => Op::ListPushMove { list: a, val: b_reg },
        Tag::ListPush => Op::ListPush { list: a, val: b_reg },
        Tag::MapSet if flags & 1 != 0 => Op::MapSetMove {
            map: a,
            key: b_reg,
            val: c_reg,
        },
        Tag::MapSet => Op::MapSet {
            map: a,
            key: b_reg,
            val: c_reg,
        },
        Tag::BuildList => Op::BuildList {
            dst: a,
            base: b_reg,
            len: c_reg,
        },
        Tag::BuildMap => Op::BuildMap {
            dst: a,
            base: b_reg,
            len: c_reg,
        },
        Tag::MakeClosure => Op::MakeClosure { dst: a, proto: b_reg },
        Tag::Break => Op::Break(sign_extend_24(w) as i16),
        Tag::Continue => Op::Continue(sign_extend_24(w) as i16),
        Tag::PatternMatch => Op::PatternMatch {
            dst: a,
            src: b_reg,
            plan: c_reg,
        },
        Tag::PatternMatchOrFail => Op::PatternMatchOrFail {
            src: a,
            plan: b_reg,
            err_kidx: c_reg,
            is_const: false,
        },
        Tag::PatternMatchOrFailConst => Op::PatternMatchOrFail {
            src: a,
            plan: b_reg,
            err_kidx: c_reg,
            is_const: true,
        },
        _ => Op::Jmp(0),
    }
}

#[allow(dead_code)]
pub(crate) fn decode_word(w: u32) -> Op {
    match decode_tag_byte(tag_of(w)) {
        DecodedTag::Regular { tag, flags } => decode_word_with_hi(tag, flags, w, (0, 0, 0)),
        DecodedTag::RegExt | DecodedTag::Ext => Op::Jmp(0),
    }
}

impl Bc32Function {
    fn try_pack(f: &Function) -> Result<Self, PackIssue> {
        let n = f.code.len();
        if n == 0 {
            return Ok(Self {
                consts: f.consts.clone(),
                code32: vec![],
                decoded: None,
                n_regs: f.n_regs,
                protos: f.protos.clone(),
                param_regs: f.param_regs.clone(),
                named_param_regs: f.named_param_regs.clone(),
                named_param_layout: f.named_param_layout.clone(),
                pattern_plans: f.pattern_plans.clone(),
            });
        }
        let mut words_per_op: Vec<usize> = vec![1; n];
        for (i, op) in f.code.iter().enumerate() {
            words_per_op[i] = match op {
                Op::ForRangePrep { idx, limit, step, .. } => {
                    let extra = pack_reg_ext_bits(*idx, *limit, *step).is_some() as usize;
                    2 + extra
                }
                Op::ForRangeLoop { idx, limit, step, .. } | Op::RangeLoopI { idx, limit, step, .. } => {
                    let extra = pack_reg_ext_bits(*idx, *limit, *step).is_some() as usize;
                    2 + extra
                }
                Op::ForRangeStep { idx, step, .. } => {
                    let extra = pack_reg_ext_bits(*idx, *step, 0).is_some() as usize;
                    2 + extra
                }
                Op::CmpLtImmJmp { r, imm, .. } => {
                    ensure_i8_range("CmpLtImmJmp", "imm", *imm as i32).map_err(|err| PackIssue::new(err, i))?;
                    2 + pack_reg_ext_bits(*r, 0, 0).is_some() as usize
                }
                Op::CmpLeImmJmp { r, imm, .. } => {
                    ensure_i8_range("CmpLeImmJmp", "imm", *imm as i32).map_err(|err| PackIssue::new(err, i))?;
                    2 + pack_reg_ext_bits(*r, 0, 0).is_some() as usize
                }
                Op::CmpEqImmJmp { r, imm, .. }
                | Op::CmpGtImmJmp { r, imm, .. }
                | Op::CmpGeImmJmp { r, imm, .. }
                | Op::CmpNeImmJmp { r, imm, .. } => {
                    if (-128..=127).contains(imm) {
                        2 + pack_reg_ext_bits(*r, 0, 0).is_some() as usize
                    } else {
                        3 + pack_reg_ext_bits(*r, 0, 0).is_some() as usize
                    }
                }
                Op::AddIntImmJmp { r, imm, .. } => {
                    ensure_i8_range("AddIntImmJmp", "imm", *imm as i32).map_err(|err| PackIssue::new(err, i))?;
                    2 + pack_reg_ext_bits(*r, 0, 0).is_some() as usize
                }
                Op::CmpIntJmp { .. } => 3,
                Op::JmpFalseSet { .. } => 1,
                Op::JmpTrueSet { .. } => 1,
                Op::NullishPick { .. } => 1,
                Op::Call { f, base, retc, .. } if *retc != 1 || pack_reg_ext_bits(*f, *base, 0).is_some() => {
                    2 + pack_reg_ext_bits(*f, *base, 0).is_some() as usize
                }
                Op::CallNamed {
                    f,
                    base_pos,
                    base_named,
                    ..
                } => 2 + pack_reg_ext_bits(*f, *base_pos, *base_named).is_some() as usize,
                _ => encode_op(op)
                    .map(|encoded| encoded.len())
                    .map_err(|err| PackIssue::new(err, i))?,
            };
        }
        loop {
            let mut changed = false;
            let mut pref: Vec<usize> = vec![0; n + 1];
            for i in 0..n {
                pref[i + 1] = pref[i] + words_per_op[i];
            }
            for (i, op) in f.code.iter().enumerate() {
                match *op {
                    Op::JmpFalseSet { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                        let j = j as usize;
                        let wofs = (pref[j] as isize - pref[i] as isize) as i32;
                        let need_two = !(-128..=127).contains(&wofs);
                        let old = words_per_op[i];
                        let new = if need_two { 2 } else { 1 };
                        if new != old {
                            words_per_op[i] = new;
                            changed = true;
                        }
                    }
                    Op::JmpTrueSet { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                        let j = j as usize;
                        let wofs = (pref[j] as isize - pref[i] as isize) as i32;
                        let need_two = !(-128..=127).contains(&wofs);
                        let old = words_per_op[i];
                        let new = if need_two { 2 } else { 1 };
                        if new != old {
                            words_per_op[i] = new;
                            changed = true;
                        }
                    }
                    Op::NullishPick { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                        let j = j as usize;
                        let wofs = (pref[j] as isize - pref[i] as isize) as i32;
                        let need_two = !(-128..=127).contains(&wofs);
                        let old = words_per_op[i];
                        let new = if need_two { 2 } else { 1 };
                        if new != old {
                            words_per_op[i] = new;
                            changed = true;
                        }
                    }
                    Op::CmpLtImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::CmpLeImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::CmpEqImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::CmpGtImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::CmpGeImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::CmpNeImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::AddIntImmJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    Op::CmpIntJmp { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }
        let mut op_to_word: Vec<usize> = vec![0; n];
        let mut acc = 0usize;
        for (i, w) in words_per_op.iter().enumerate() {
            op_to_word[i] = acc;
            acc += *w;
        }
        let total_words = acc;
        let mut out: Vec<u32> = Vec::with_capacity(total_words);
        for (i, op) in f.code.iter().enumerate() {
            match op {
                Op::Jmp(ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    out.push(((encode_tag_with_flags(Tag::Jmp, 0) as u32) << 24) | ((wofs as u32) & 0x00FF_FFFF));
                }
                Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let (hi, lo) = ((wofs >> 8) as u8, (wofs & 0xFF) as u8);
                    out.push(pack(Tag::JmpFalse, 0, (*r & 0xFF) as u8, hi, lo));
                    if let Some(ext) = pack_reg_ext_bits(*r, 0, 0) {
                        out.push(ext);
                    }
                }
                Op::JmpIfNil(r, ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let (hi, lo) = ((wofs >> 8) as u8, (wofs & 0xFF) as u8);
                    out.push(pack(Tag::JmpIfNil, 0, (*r & 0xFF) as u8, hi, lo));
                    if let Some(ext) = pack_reg_ext_bits(*r, 0, 0) {
                        out.push(ext);
                    }
                }
                Op::JmpIfNotNil(r, ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let (hi, lo) = ((wofs >> 8) as u8, (wofs & 0xFF) as u8);
                    out.push(pack(Tag::JmpIfNotNil, 0, (*r & 0xFF) as u8, hi, lo));
                    if let Some(ext) = pack_reg_ext_bits(*r, 0, 0) {
                        out.push(ext);
                    }
                }
                Op::NullishPick { l, dst, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    if (-128..=127).contains(&wofs) {
                        out.push(pack(
                            Tag::NullishPick,
                            0,
                            (*l & 0xFF) as u8,
                            (*dst & 0xFF) as u8,
                            (wofs as i8) as u8,
                        ));
                    } else {
                        let wofs16 = wofs as i16;
                        out.push(pack(Tag::NullishPickX, 0, (*l & 0xFF) as u8, (*dst & 0xFF) as u8, 0));
                        out.push(pack_ext_word(0, (wofs16 >> 8) as u8, (wofs16 & 0xFF) as u8));
                    }
                }
                Op::JmpFalseSet { r, dst, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    if (-128..=127).contains(&wofs) && words_per_op[i] == 1 {
                        out.push(pack(
                            Tag::JmpFalseSet,
                            0,
                            (*r & 0xFF) as u8,
                            (*dst & 0xFF) as u8,
                            (wofs as i8) as u8,
                        ));
                    } else {
                        let wofs16 = wofs as i16;
                        out.push(pack(Tag::JmpFalseSetX, 0, (*r & 0xFF) as u8, (*dst & 0xFF) as u8, 0));
                        out.push(pack_ext_word(0, (wofs16 >> 8) as u8, (wofs16 & 0xFF) as u8));
                    }
                }
                Op::JmpTrueSet { r, dst, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    if (-128..=127).contains(&wofs) && words_per_op[i] == 1 {
                        out.push(pack(
                            Tag::JmpTrueSet,
                            0,
                            (*r & 0xFF) as u8,
                            (*dst & 0xFF) as u8,
                            (wofs as i8) as u8,
                        ));
                    } else {
                        let wofs16 = wofs as i16;
                        out.push(pack(Tag::JmpTrueSetX, 0, (*r & 0xFF) as u8, (*dst & 0xFF) as u8, 0));
                        out.push(pack_ext_word(0, (wofs16 >> 8) as u8, (wofs16 & 0xFF) as u8));
                    }
                }
                Op::Break(ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = op_to_word[tgt] as isize - op_to_word[i] as isize;
                    out.push(((encode_tag_with_flags(Tag::Break, 0) as u32) << 24) | ((wofs as u32) & 0x00FF_FFFF));
                }
                Op::Continue(ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = op_to_word[tgt] as isize - op_to_word[i] as isize;
                    out.push(((encode_tag_with_flags(Tag::Continue, 0) as u32) << 24) | ((wofs as u32) & 0x00FF_FFFF));
                }
                Op::ForRangePrep {
                    idx,
                    limit,
                    step,
                    inclusive,
                    explicit,
                } => {
                    let flags = (if *inclusive { 1 } else { 0 }) | (if *explicit { 2 } else { 0 });
                    out.push(pack(Tag::ForRangePrep, 0, *idx as u8, *limit as u8, *step as u8));
                    if let Some(ext) = pack_reg_ext_bits(*idx, *limit, *step) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(flags as u8, 0, 0));
                }
                Op::ForRangeLoop {
                    idx,
                    limit,
                    step,
                    inclusive,
                    write_idx,
                    ofs,
                }
                | Op::RangeLoopI {
                    idx,
                    limit,
                    step,
                    inclusive,
                    write_idx,
                    ofs,
                } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let flags = u8::from(*inclusive) | if *write_idx { 0 } else { 2 };
                    out.push(pack(Tag::ForRangeLoop, 0, *idx as u8, *limit as u8, *step as u8));
                    if let Some(ext) = pack_reg_ext_bits(*idx, *limit, *step) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(flags, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::ForRangeStep { idx, step, back_ofs } => {
                    let tgt = ((i as isize) + *back_ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    out.push(pack(Tag::ForRangeStep, 0, *idx as u8, *step as u8, 0));
                    if let Some(ext) = pack_reg_ext_bits(*idx, *step, 0) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(0, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::CmpLtImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    out.push(pack(Tag::CmpLtImmJmp, 0, *r as u8, (*imm as i8) as u8, 0));
                    if let Some(ext) = pack_reg_ext_bits(*r, 0, 0) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(0, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::CmpLeImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    out.push(pack(Tag::CmpLeImmJmp, 0, *r as u8, (*imm as i8) as u8, 0));
                    if let Some(ext) = pack_reg_ext_bits(*r, 0, 0) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(0, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::CmpNeImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    compare::pack_cmp_ne_imm_jmp(*r, *imm, wofs)
                        .map_err(|err| PackIssue::new(err, i))?
                        .emit(&mut out);
                }
                Op::CmpEqImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    compare::pack_cmp_eq_imm_jmp(*r, *imm, wofs)
                        .map_err(|err| PackIssue::new(err, i))?
                        .emit(&mut out);
                }
                Op::CmpGtImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    compare::pack_cmp_gt_imm_jmp(*r, *imm, wofs)
                        .map_err(|err| PackIssue::new(err, i))?
                        .emit(&mut out);
                }
                Op::CmpGeImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    compare::pack_cmp_ge_imm_jmp(*r, *imm, wofs)
                        .map_err(|err| PackIssue::new(err, i))?
                        .emit(&mut out);
                }
                Op::AddIntImmJmp { r, imm, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    out.push(pack(Tag::AddIntImmJmp, 0, *r as u8, (*imm as i8) as u8, 0));
                    if let Some(ext) = pack_reg_ext_bits(*r, 0, 0) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(0, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::CmpIntJmp { kind, a, b, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    pack_cmp_i_jmp(*a, *b, *kind, wofs).emit(&mut out);
                }
                Op::Call { f, base, argc, retc } => {
                    if *retc == 1 && pack_reg_ext_bits(*f, *base, 0).is_none() {
                        out.push(pack(Tag::Call, 0, *f as u8, *base as u8, *argc));
                    } else {
                        out.push(pack(Tag::CallX, 0, *f as u8, *base as u8, *retc));
                        if let Some(ext) = pack_reg_ext_bits(*f, *base, 0) {
                            out.push(ext);
                        }
                        out.push(pack_ext_word(*argc, 0, 0));
                    }
                }
                Op::CallNamed {
                    f,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
                } => {
                    out.push(pack(Tag::CallNamedX, 0, *f as u8, *base_pos as u8, *base_named as u8));
                    if let Some(ext) = pack_reg_ext_bits(*f, *base_pos, *base_named) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(*posc, *namedc, *retc));
                }
                _ => {
                    let encoded = encode_op(op).map_err(|err| PackIssue::new(err, i))?;
                    encoded.emit(&mut out);
                }
            }
        }
        let decoded = Bc32Decoded::from_words(&out).map(Arc::new);

        Ok(Self {
            consts: f.consts.clone(),
            code32: out,
            decoded,
            n_regs: f.n_regs,
            protos: f.protos.clone(),
            param_regs: f.param_regs.clone(),
            named_param_regs: f.named_param_regs.clone(),
            named_param_layout: f.named_param_layout.clone(),
            pattern_plans: f.pattern_plans.clone(),
        })
    }

    pub fn try_from_function(f: &Function) -> Option<Self> {
        record_attempt(f.code.len());
        match Self::try_pack(f) {
            Ok(packed) => {
                record_success(packed.code32.len());
                Some(packed)
            }
            Err(issue) => {
                let PackIssue { reason, op_index } = issue;
                let reason_key = reason.reason_key();
                let opcode = reason.opcode();
                let detail = reason.detail();
                record_failure(reason_key, opcode);
                let op_index_str = op_index.map(|idx| idx.to_string()).unwrap_or_else(|| "n/a".to_string());
                info!(
                    target: TRACE_TARGET,
                    reason = reason_key,
                    opcode = opcode,
                    detail = detail,
                    op_index = %op_index_str,
                    "bc32 packing fallback"
                );
                None
            }
        }
    }
}

/// Utility: expose tag and constants for VM bc32 fast-path
pub(crate) fn tag_of(w: u32) -> u8 {
    ((w >> 24) & 0xFF) as u8
}
pub(crate) const TAG_FOR_RANGE_PREP: u8 = encode_tag_raw(Tag::ForRangePrep);
pub(crate) const TAG_FOR_RANGE_LOOP: u8 = encode_tag_raw(Tag::ForRangeLoop);
pub(crate) const TAG_FOR_RANGE_STEP: u8 = encode_tag_raw(Tag::ForRangeStep);
pub(crate) const TAG_JMP_FALSE_SET_X: u8 = encode_tag_raw(Tag::JmpFalseSetX);
pub(crate) const TAG_JMP_TRUE_SET_X: u8 = encode_tag_raw(Tag::JmpTrueSetX);
pub(crate) const TAG_NULLISH_PICK_X: u8 = encode_tag_raw(Tag::NullishPickX);
pub(crate) const TAG_CALL_X: u8 = encode_tag_raw(Tag::CallX);
pub(crate) const TAG_CALL_NAMED_X: u8 = encode_tag_raw(Tag::CallNamedX);
pub(crate) const TAG_REG_EXT: u8 = RAW_TAG_REG_EXT;
pub(crate) const TAG_EXT: u8 = RAW_TAG_EXT;

#[cfg(test)]
mod tests;
