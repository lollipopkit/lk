use anyhow::{Result, bail};

use super::{read_i16, read_u8, read_u16, write_i16, write_u8, write_u16};
use crate::vm::bytecode::Op;

pub(super) fn encode_op(out: &mut Vec<u8>, op: &Op) -> Result<()> {
    match *op {
        Op::Nop => {
            write_u8(out, 106);
        }
        Op::LoadK(dst, kidx) => {
            write_u8(out, 0);
            write_u16(out, dst);
            write_u16(out, kidx);
        }
        Op::Move(dst, src) => {
            write_u8(out, 1);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::Not(dst, src) => {
            write_u8(out, 2);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::ToStr(dst, src) => {
            write_u8(out, 43);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::ToBool(dst, src) => {
            write_u8(out, 3);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::JmpIfNil(reg, ofs) => {
            write_u8(out, 4);
            write_u16(out, reg);
            write_i16(out, ofs);
        }
        Op::JmpIfNotNil(reg, ofs) => {
            write_u8(out, 5);
            write_u16(out, reg);
            write_i16(out, ofs);
        }
        Op::NullishPick { l, dst, ofs } => {
            write_u8(out, 6);
            write_u16(out, l);
            write_u16(out, dst);
            write_i16(out, ofs);
        }
        Op::JmpFalseSet { r, dst, ofs } => {
            write_u8(out, 7);
            write_u16(out, r);
            write_u16(out, dst);
            write_i16(out, ofs);
        }
        Op::JmpTrueSet { r, dst, ofs } => {
            write_u8(out, 8);
            write_u16(out, r);
            write_u16(out, dst);
            write_i16(out, ofs);
        }
        Op::Add(dst, a, b) => encode_op3(out, 9, dst, a, b),
        Op::AddInt(dst, a, b) => encode_op3(out, 90, dst, a, b),
        Op::AddFloat(dst, a, b) => encode_op3(out, 91, dst, a, b),
        Op::StrConcatKnownCap(dst, a, b) => encode_op3(out, 81, dst, a, b),
        Op::StrConcatToStr(dst, lhs, src) => encode_op3(out, 111, dst, lhs, src),
        Op::AddIntImm(dst, src, imm) => {
            write_u8(out, 50);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::Sub(dst, a, b) => encode_op3(out, 10, dst, a, b),
        Op::SubInt(dst, a, b) => encode_op3(out, 92, dst, a, b),
        Op::SubFloat(dst, a, b) => encode_op3(out, 93, dst, a, b),
        Op::Mul(dst, a, b) => encode_op3(out, 11, dst, a, b),
        Op::MulInt(dst, a, b) => encode_op3(out, 94, dst, a, b),
        Op::MulFloat(dst, a, b) => encode_op3(out, 95, dst, a, b),
        Op::Div(dst, a, b) => encode_op3(out, 12, dst, a, b),
        Op::DivFloat(dst, a, b) => encode_op3(out, 96, dst, a, b),
        Op::FloorDivImm { dst, src, imm } => {
            write_u8(out, 100);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::Mod(dst, a, b) => encode_op3(out, 13, dst, a, b),
        Op::ModInt(dst, a, b) => encode_op3(out, 97, dst, a, b),
        Op::ModFloat(dst, a, b) => encode_op3(out, 98, dst, a, b),
        Op::CmpEq(dst, a, b) => encode_op3(out, 14, dst, a, b),
        Op::CmpNe(dst, a, b) => encode_op3(out, 15, dst, a, b),
        Op::CmpLt(dst, a, b) => encode_op3(out, 16, dst, a, b),
        Op::CmpLe(dst, a, b) => encode_op3(out, 17, dst, a, b),
        Op::CmpGt(dst, a, b) => encode_op3(out, 18, dst, a, b),
        Op::CmpGe(dst, a, b) => encode_op3(out, 19, dst, a, b),
        Op::CmpI { dst, a, b, kind } => {
            write_u8(out, 84);
            write_u16(out, dst);
            write_u16(out, a);
            write_u16(out, b);
            write_u8(out, kind as u8);
        }
        Op::CmpIntJmp { kind, a, b, ofs } => {
            write_u8(out, 99);
            write_u8(out, kind as u8);
            write_u16(out, a);
            write_u16(out, b);
            write_i16(out, ofs);
        }
        Op::CmpEqImm(dst, src, imm) => {
            write_u8(out, 51);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::CmpNeImm(dst, src, imm) => {
            write_u8(out, 52);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::CmpLtImm(dst, src, imm) => {
            write_u8(out, 53);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::CmpLeImm(dst, src, imm) => {
            write_u8(out, 54);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::CmpGtImm(dst, src, imm) => {
            write_u8(out, 55);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::CmpGeImm(dst, src, imm) => {
            write_u8(out, 56);
            write_u16(out, dst);
            write_u16(out, src);
            write_i16(out, imm);
        }
        Op::In(dst, a, b) => encode_op3(out, 20, dst, a, b),
        Op::LoadLocal(dst, idx) => {
            write_u8(out, 21);
            write_u16(out, dst);
            write_u16(out, idx);
        }
        Op::StoreLocal(idx, src) => {
            write_u8(out, 22);
            write_u16(out, idx);
            write_u16(out, src);
        }
        Op::LoadGlobal(dst, kidx) => {
            write_u8(out, 23);
            write_u16(out, dst);
            write_u16(out, kidx);
        }
        Op::DefineGlobal(kidx, src) => {
            write_u8(out, 24);
            write_u16(out, kidx);
            write_u16(out, src);
        }
        Op::LoadCapture { dst, idx } => {
            write_u8(out, 25);
            write_u16(out, dst);
            write_u16(out, idx);
        }
        Op::Access(dst, base, field) => encode_op3(out, 26, dst, base, field),
        Op::AccessK(dst, base, kidx) => encode_op3(out, 27, dst, base, kidx),
        Op::IndexK(dst, base, kidx) => encode_op3(out, 28, dst, base, kidx),
        Op::ListSetI { dst, list, index, val } => {
            write_u8(out, 82);
            write_u16(out, dst);
            write_u16(out, list);
            write_i16(out, index);
            write_u16(out, val);
        }
        Op::ListIndexI(dst, base, index) => {
            write_u8(out, 76);
            write_u16(out, dst);
            write_u16(out, base);
            write_i16(out, index);
        }
        Op::StrIndexI(dst, base, index) => {
            write_u8(out, 77);
            write_u16(out, dst);
            write_u16(out, base);
            write_i16(out, index);
        }
        Op::Len { dst, src } => {
            write_u8(out, 29);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::ListLen { dst, src } => {
            write_u8(out, 73);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::MapLen { dst, src } => {
            write_u8(out, 74);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::StrLen { dst, src } => {
            write_u8(out, 75);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::Floor { dst, src } => {
            write_u8(out, 68);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::StartsWithK(dst, src, kidx) => encode_op3(out, 67, dst, src, kidx),
        Op::ContainsK(dst, src, kidx) => encode_op3(out, 70, dst, src, kidx),
        Op::MapHas(dst, map, key) => encode_op3(out, 71, dst, map, key),
        Op::MapGetInterned(dst, map, kidx) => encode_op3(out, 78, dst, map, kidx),
        Op::MapGetDynamic(dst, map, key) => encode_op3(out, 80, dst, map, key),
        Op::MapSetInterned(map, kidx, val) => encode_op3(out, 79, map, kidx, val),
        Op::MapSetInternedMove(map, kidx, val) => encode_op3(out, 105, map, kidx, val),
        Op::MapHasK(dst, map, kidx) => encode_op3(out, 72, dst, map, kidx),
        Op::Index { dst, base, idx } => encode_op3(out, 30, dst, base, idx),
        Op::ToIter { dst, src } => {
            write_u8(out, 31);
            write_u16(out, dst);
            write_u16(out, src);
        }
        Op::BuildList { dst, base, len } => {
            write_u8(out, 32);
            write_u16(out, dst);
            write_u16(out, base);
            write_u16(out, len);
        }
        Op::BuildMap { dst, base, len } => {
            write_u8(out, 33);
            write_u16(out, dst);
            write_u16(out, base);
            write_u16(out, len);
        }
        Op::ListSlice { dst, src, start } => encode_op3(out, 34, dst, src, start),
        Op::MakeClosure { dst, proto } => {
            write_u8(out, 35);
            write_u16(out, dst);
            write_u16(out, proto);
        }
        Op::Jmp(ofs) => {
            write_u8(out, 36);
            write_i16(out, ofs);
        }
        Op::JmpFalse(reg, ofs) | Op::BoolBranch(reg, ofs) => {
            write_u8(out, 37);
            write_u16(out, reg);
            write_i16(out, ofs);
        }
        Op::Call { f, base, argc, retc } => {
            write_u8(out, 38);
            write_u16(out, f);
            write_u16(out, base);
            write_u8(out, argc);
            write_u8(out, retc);
        }
        Op::CallExact { f, base, argc, retc } => {
            write_u8(out, 86);
            write_u16(out, f);
            write_u16(out, base);
            write_u8(out, argc);
            write_u8(out, retc);
        }
        Op::CallClosureExact { f, base, argc, retc } => {
            write_u8(out, 85);
            write_u16(out, f);
            write_u16(out, base);
            write_u8(out, argc);
            write_u8(out, retc);
        }
        Op::CallNativeFast { f, base, argc, retc } => {
            write_u8(out, 83);
            write_u16(out, f);
            write_u16(out, base);
            write_u8(out, argc);
            write_u8(out, retc);
        }
        Op::CallMethod0 { dst, receiver, method } => {
            write_u8(out, 88);
            write_u16(out, dst);
            write_u16(out, receiver);
            write_u16(out, method);
        }
        Op::CallGlobalMethod0 { dst, receiver, method } => {
            write_u8(out, 89);
            write_u16(out, dst);
            write_u16(out, receiver);
            write_u16(out, method);
        }
        Op::Ret { base, retc } => {
            write_u8(out, 39);
            write_u16(out, base);
            write_u8(out, retc);
        }
        Op::ForRangePrep {
            idx,
            limit,
            step,
            inclusive,
            explicit,
        } => {
            write_u8(out, 40);
            write_u16(out, idx);
            write_u16(out, limit);
            write_u16(out, step);
            write_u8(out, inclusive as u8);
            write_u8(out, explicit as u8);
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
            write_u8(out, 41);
            write_u16(out, idx);
            write_u16(out, limit);
            write_u16(out, step);
            write_u8(out, inclusive as u8);
            write_u8(out, write_idx as u8);
            write_i16(out, ofs);
        }
        Op::ForRangeStep { idx, step, back_ofs } => {
            write_u8(out, 42);
            write_u16(out, idx);
            write_u16(out, step);
            write_i16(out, back_ofs);
        }
        Op::CallNamed {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            retc,
        } => {
            write_u8(out, 44);
            write_u16(out, f);
            write_u16(out, base_pos);
            write_u8(out, posc);
            write_u16(out, base_named);
            write_u8(out, namedc);
            write_u8(out, retc);
        }
        Op::CallNamedFallback {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            retc,
        } => {
            write_u8(out, 87);
            write_u16(out, f);
            write_u16(out, base_pos);
            write_u8(out, posc);
            write_u16(out, base_named);
            write_u8(out, namedc);
            write_u8(out, retc);
        }
        Op::Break(ofs) => {
            write_u8(out, 45);
            write_i16(out, ofs);
        }
        Op::Continue(ofs) => {
            write_u8(out, 46);
            write_i16(out, ofs);
        }
        Op::CmpLtImmJmp { r, imm, ofs } => {
            write_u8(out, 57);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::JmpNilOrFalseJmp { r, ofs } => {
            write_u8(out, 58);
            write_u16(out, r);
            write_i16(out, ofs);
        }
        Op::AddIntImmJmp { r, imm, ofs } => {
            write_u8(out, 59);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::CmpLeImmJmp { r, imm, ofs } => {
            write_u8(out, 60);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::CmpEqImmJmp { r, imm, ofs } => {
            write_u8(out, 103);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::CmpGtImmJmp { r, imm, ofs } => {
            write_u8(out, 101);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::CmpGeImmJmp { r, imm, ofs } => {
            write_u8(out, 102);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::CmpNeImmJmp { r, imm, ofs } => {
            write_u8(out, 69);
            write_u16(out, r);
            write_i16(out, imm);
            write_i16(out, ofs);
        }
        Op::ListPush { list, val } => {
            write_u8(out, 61);
            write_u16(out, list);
            write_u16(out, val);
        }
        Op::ListPushMove { list, val } => {
            write_u8(out, 104);
            write_u16(out, list);
            write_u16(out, val);
        }
        Op::MapSet { map, key, val } => {
            write_u8(out, 62);
            write_u16(out, map);
            write_u16(out, key);
            write_u16(out, val);
        }
        Op::MapSetMove { map, key, val } => {
            write_u8(out, 66);
            write_u16(out, map);
            write_u16(out, key);
            write_u16(out, val);
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
            write_u8(out, 63);
            write_u16(out, target);
            write_u16(out, idx);
            write_u16(out, limit);
            write_u16(out, step);
            write_u8(out, inclusive as u8);
            write_u8(out, explicit as u8);
            write_i16(out, imm);
        }
        Op::ListFoldAdd { acc, list } => {
            write_u8(out, 64);
            write_u16(out, acc);
            write_u16(out, list);
        }
        Op::MapValuesFoldAdd { acc, map } => {
            write_u8(out, 65);
            write_u16(out, acc);
            write_u16(out, map);
        }
        Op::PatternMatch { dst, src, plan } => {
            write_u8(out, 47);
            write_u16(out, dst);
            write_u16(out, src);
            write_u16(out, plan);
        }
        Op::PatternMatchOrFail {
            src,
            plan,
            err_kidx,
            is_const,
        } => {
            write_u8(out, 48);
            write_u16(out, src);
            write_u16(out, plan);
            write_u16(out, err_kidx);
            write_u8(out, is_const as u8);
        }
        Op::Raise { err_kidx } => {
            write_u8(out, 49);
            write_u16(out, err_kidx);
        }
    }
    Ok(())
}

fn encode_op3(out: &mut Vec<u8>, tag: u8, a: u16, b: u16, c: u16) {
    write_u8(out, tag);
    write_u16(out, a);
    write_u16(out, b);
    write_u16(out, c);
}

pub(super) fn decode_op(bytes: &[u8], cursor: &mut usize) -> Result<Op> {
    let tag = read_u8(bytes, cursor)?;
    let op = match tag {
        106 => Op::Nop,
        0 => Op::LoadK(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        1 => Op::Move(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        2 => Op::Not(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        43 => Op::ToStr(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        3 => Op::ToBool(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        4 => Op::JmpIfNil(read_u16(bytes, cursor)?, read_i16(bytes, cursor)?),
        5 => Op::JmpIfNotNil(read_u16(bytes, cursor)?, read_i16(bytes, cursor)?),
        6 => Op::NullishPick {
            l: read_u16(bytes, cursor)?,
            dst: read_u16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        7 => Op::JmpFalseSet {
            r: read_u16(bytes, cursor)?,
            dst: read_u16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        8 => Op::JmpTrueSet {
            r: read_u16(bytes, cursor)?,
            dst: read_u16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        9 => decode_op3(Op::Add, bytes, cursor)?,
        90 => decode_op3(Op::AddInt, bytes, cursor)?,
        91 => decode_op3(Op::AddFloat, bytes, cursor)?,
        81 => decode_op3(Op::StrConcatKnownCap, bytes, cursor)?,
        111 => decode_op3(Op::StrConcatToStr, bytes, cursor)?,
        10 => decode_op3(Op::Sub, bytes, cursor)?,
        92 => decode_op3(Op::SubInt, bytes, cursor)?,
        93 => decode_op3(Op::SubFloat, bytes, cursor)?,
        11 => decode_op3(Op::Mul, bytes, cursor)?,
        94 => decode_op3(Op::MulInt, bytes, cursor)?,
        95 => decode_op3(Op::MulFloat, bytes, cursor)?,
        12 => decode_op3(Op::Div, bytes, cursor)?,
        96 => decode_op3(Op::DivFloat, bytes, cursor)?,
        100 => Op::FloorDivImm {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
        },
        13 => decode_op3(Op::Mod, bytes, cursor)?,
        97 => decode_op3(Op::ModInt, bytes, cursor)?,
        98 => decode_op3(Op::ModFloat, bytes, cursor)?,
        14 => decode_op3(Op::CmpEq, bytes, cursor)?,
        15 => decode_op3(Op::CmpNe, bytes, cursor)?,
        16 => decode_op3(Op::CmpLt, bytes, cursor)?,
        17 => decode_op3(Op::CmpLe, bytes, cursor)?,
        18 => decode_op3(Op::CmpGt, bytes, cursor)?,
        19 => decode_op3(Op::CmpGe, bytes, cursor)?,
        84 => Op::CmpI {
            dst: read_u16(bytes, cursor)?,
            a: read_u16(bytes, cursor)?,
            b: read_u16(bytes, cursor)?,
            kind: crate::vm::bytecode::IntCmpKind::from_u8(read_u8(bytes, cursor)?)
                .ok_or_else(|| anyhow::anyhow!("invalid CmpI kind"))?,
        },
        99 => Op::CmpIntJmp {
            kind: crate::vm::bytecode::IntCmpKind::from_u8(read_u8(bytes, cursor)?)
                .ok_or_else(|| anyhow::anyhow!("invalid CmpIntJmp kind"))?,
            a: read_u16(bytes, cursor)?,
            b: read_u16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        20 => decode_op3(Op::In, bytes, cursor)?,
        21 => Op::LoadLocal(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        22 => Op::StoreLocal(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        23 => Op::LoadGlobal(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        24 => Op::DefineGlobal(read_u16(bytes, cursor)?, read_u16(bytes, cursor)?),
        25 => Op::LoadCapture {
            dst: read_u16(bytes, cursor)?,
            idx: read_u16(bytes, cursor)?,
        },
        26 => decode_op3(Op::Access, bytes, cursor)?,
        27 => decode_op3(Op::AccessK, bytes, cursor)?,
        28 => decode_op3(Op::IndexK, bytes, cursor)?,
        82 => Op::ListSetI {
            dst: read_u16(bytes, cursor)?,
            list: read_u16(bytes, cursor)?,
            index: read_i16(bytes, cursor)?,
            val: read_u16(bytes, cursor)?,
        },
        76 => Op::ListIndexI(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        77 => Op::StrIndexI(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        29 => Op::Len {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
        },
        73 => Op::ListLen {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
        },
        74 => Op::MapLen {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
        },
        75 => Op::StrLen {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
        },
        68 => Op::Floor {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
        },
        30 => Op::Index {
            dst: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            idx: read_u16(bytes, cursor)?,
        },
        31 => Op::ToIter {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
        },
        32 => Op::BuildList {
            dst: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            len: read_u16(bytes, cursor)?,
        },
        33 => Op::BuildMap {
            dst: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            len: read_u16(bytes, cursor)?,
        },
        34 => Op::ListSlice {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
            start: read_u16(bytes, cursor)?,
        },
        35 => Op::MakeClosure {
            dst: read_u16(bytes, cursor)?,
            proto: read_u16(bytes, cursor)?,
        },
        50 => Op::AddIntImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        51 => Op::CmpEqImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        52 => Op::CmpNeImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        53 => Op::CmpLtImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        54 => Op::CmpLeImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        55 => Op::CmpGtImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        56 => Op::CmpGeImm(
            read_u16(bytes, cursor)?,
            read_u16(bytes, cursor)?,
            read_i16(bytes, cursor)?,
        ),
        57 => Op::CmpLtImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        58 => Op::JmpNilOrFalseJmp {
            r: read_u16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        59 => Op::AddIntImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        60 => Op::CmpLeImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        103 => Op::CmpEqImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        69 => Op::CmpNeImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        101 => Op::CmpGtImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        102 => Op::CmpGeImmJmp {
            r: read_u16(bytes, cursor)?,
            imm: read_i16(bytes, cursor)?,
            ofs: read_i16(bytes, cursor)?,
        },
        61 => Op::ListPush {
            list: read_u16(bytes, cursor)?,
            val: read_u16(bytes, cursor)?,
        },
        104 => Op::ListPushMove {
            list: read_u16(bytes, cursor)?,
            val: read_u16(bytes, cursor)?,
        },
        62 => Op::MapSet {
            map: read_u16(bytes, cursor)?,
            key: read_u16(bytes, cursor)?,
            val: read_u16(bytes, cursor)?,
        },
        63 => Op::AddRangeCountImm {
            target: read_u16(bytes, cursor)?,
            idx: read_u16(bytes, cursor)?,
            limit: read_u16(bytes, cursor)?,
            step: read_u16(bytes, cursor)?,
            inclusive: read_u8(bytes, cursor)? != 0,
            explicit: read_u8(bytes, cursor)? != 0,
            imm: read_i16(bytes, cursor)?,
        },
        64 => Op::ListFoldAdd {
            acc: read_u16(bytes, cursor)?,
            list: read_u16(bytes, cursor)?,
        },
        65 => Op::MapValuesFoldAdd {
            acc: read_u16(bytes, cursor)?,
            map: read_u16(bytes, cursor)?,
        },
        66 => Op::MapSetMove {
            map: read_u16(bytes, cursor)?,
            key: read_u16(bytes, cursor)?,
            val: read_u16(bytes, cursor)?,
        },
        67 => decode_op3(Op::StartsWithK, bytes, cursor)?,
        70 => decode_op3(Op::ContainsK, bytes, cursor)?,
        71 => decode_op3(Op::MapHas, bytes, cursor)?,
        78 => decode_op3(Op::MapGetInterned, bytes, cursor)?,
        80 => decode_op3(Op::MapGetDynamic, bytes, cursor)?,
        79 => decode_op3(Op::MapSetInterned, bytes, cursor)?,
        105 => decode_op3(Op::MapSetInternedMove, bytes, cursor)?,
        72 => decode_op3(Op::MapHasK, bytes, cursor)?,
        36 => Op::Jmp(read_i16(bytes, cursor)?),
        37 => Op::BoolBranch(read_u16(bytes, cursor)?, read_i16(bytes, cursor)?),
        38 => Op::Call {
            f: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            argc: read_u8(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        86 => Op::CallExact {
            f: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            argc: read_u8(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        85 => Op::CallClosureExact {
            f: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            argc: read_u8(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        83 => Op::CallNativeFast {
            f: read_u16(bytes, cursor)?,
            base: read_u16(bytes, cursor)?,
            argc: read_u8(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        88 => Op::CallMethod0 {
            dst: read_u16(bytes, cursor)?,
            receiver: read_u16(bytes, cursor)?,
            method: read_u16(bytes, cursor)?,
        },
        89 => Op::CallGlobalMethod0 {
            dst: read_u16(bytes, cursor)?,
            receiver: read_u16(bytes, cursor)?,
            method: read_u16(bytes, cursor)?,
        },
        39 => Op::Ret {
            base: read_u16(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        40 => Op::ForRangePrep {
            idx: read_u16(bytes, cursor)?,
            limit: read_u16(bytes, cursor)?,
            step: read_u16(bytes, cursor)?,
            inclusive: read_u8(bytes, cursor)? != 0,
            explicit: read_u8(bytes, cursor)? != 0,
        },
        41 => Op::RangeLoopI {
            idx: read_u16(bytes, cursor)?,
            limit: read_u16(bytes, cursor)?,
            step: read_u16(bytes, cursor)?,
            inclusive: read_u8(bytes, cursor)? != 0,
            write_idx: read_u8(bytes, cursor)? != 0,
            ofs: read_i16(bytes, cursor)?,
        },
        42 => Op::ForRangeStep {
            idx: read_u16(bytes, cursor)?,
            step: read_u16(bytes, cursor)?,
            back_ofs: read_i16(bytes, cursor)?,
        },
        44 => Op::CallNamed {
            f: read_u16(bytes, cursor)?,
            base_pos: read_u16(bytes, cursor)?,
            posc: read_u8(bytes, cursor)?,
            base_named: read_u16(bytes, cursor)?,
            namedc: read_u8(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        87 => Op::CallNamedFallback {
            f: read_u16(bytes, cursor)?,
            base_pos: read_u16(bytes, cursor)?,
            posc: read_u8(bytes, cursor)?,
            base_named: read_u16(bytes, cursor)?,
            namedc: read_u8(bytes, cursor)?,
            retc: read_u8(bytes, cursor)?,
        },
        45 => Op::Break(read_i16(bytes, cursor)?),
        46 => Op::Continue(read_i16(bytes, cursor)?),
        47 => Op::PatternMatch {
            dst: read_u16(bytes, cursor)?,
            src: read_u16(bytes, cursor)?,
            plan: read_u16(bytes, cursor)?,
        },
        48 => Op::PatternMatchOrFail {
            src: read_u16(bytes, cursor)?,
            plan: read_u16(bytes, cursor)?,
            err_kidx: read_u16(bytes, cursor)?,
            is_const: read_u8(bytes, cursor)? != 0,
        },
        49 => Op::Raise {
            err_kidx: read_u16(bytes, cursor)?,
        },
        _ => bail!("unknown opcode tag {}", tag),
    };
    Ok(op)
}

fn decode_op3<F>(ctor: F, bytes: &[u8], cursor: &mut usize) -> Result<Op>
where
    F: Fn(u16, u16, u16) -> Op,
{
    Ok(ctor(
        read_u16(bytes, cursor)?,
        read_u16(bytes, cursor)?,
        read_u16(bytes, cursor)?,
    ))
}
