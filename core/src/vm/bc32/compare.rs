use super::{Bc32Reject, EncodedOp, Op, RK_FLAG_B, RK_FLAG_C, Tag, pack, pack_ext_word, pack_reg_ext_bits};
use super::{
    EXT_OP_CMP_EQ_IMM16, EXT_OP_CMP_GE_IMM16, EXT_OP_CMP_GT_IMM16, EXT_OP_CMP_LE_IMM16, EXT_OP_CMP_LT_IMM16,
    EXT_OP_CMP_NE_IMM16, RAW_TAG_EXT,
};
use crate::vm::bytecode::{rk_index, rk_is_const};

#[inline]
fn pack_rk_compare(tag: Tag, d: u16, a: u16, b: u16) -> EncodedOp {
    let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
    let word = pack(tag, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
    let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
    EncodedOp::new(word, reg_ext)
}

#[inline]
fn pack_cmp_imm16(ext_op: u8, d: u16, a: u16, imm: i16) -> EncodedOp {
    let word = ((RAW_TAG_EXT as u32) << 24) | ((ext_op as u32) << 16) | (((d as u8) as u32) << 8) | (a as u8 as u32);
    let imm = imm as u16;
    let ext = pack_ext_word((imm >> 8) as u8, imm as u8, 0);
    if let Some(reg_ext) = pack_reg_ext_bits(d, a, 0) {
        EncodedOp::with_extra(word, [ext, reg_ext])
    } else {
        EncodedOp::new(word, Some(ext))
    }
}

#[inline]
fn pack_cmp_imm(tag: Tag, ext_op: u8, d: u16, a: u16, imm: i16) -> Result<EncodedOp, Bc32Reject> {
    if (-128..=127).contains(&imm) {
        let word = pack(tag, 0, d as u8, a as u8, (imm as i8) as u8);
        let reg_ext = pack_reg_ext_bits(d, a, 0);
        Ok(EncodedOp::new(word, reg_ext))
    } else {
        Ok(pack_cmp_imm16(ext_op, d, a, imm))
    }
}

pub(super) fn encode_compare_op(op: &Op) -> Option<Result<EncodedOp, Bc32Reject>> {
    Some(match *op {
        Op::CmpEq(d, a, b) => Ok(pack_rk_compare(Tag::Eq, d, a, b)),
        Op::CmpNe(d, a, b) => Ok(pack_rk_compare(Tag::Ne, d, a, b)),
        Op::CmpLt(d, a, b) => Ok(pack_rk_compare(Tag::Lt, d, a, b)),
        Op::CmpLe(d, a, b) => Ok(pack_rk_compare(Tag::Le, d, a, b)),
        Op::CmpGt(d, a, b) => Ok(pack_rk_compare(Tag::Gt, d, a, b)),
        Op::CmpGe(d, a, b) => Ok(pack_rk_compare(Tag::Ge, d, a, b)),
        Op::CmpEqImm(d, a, imm) => pack_cmp_imm(Tag::CmpEqImm, EXT_OP_CMP_EQ_IMM16, d, a, imm),
        Op::CmpNeImm(d, a, imm) => pack_cmp_imm(Tag::CmpNeImm, EXT_OP_CMP_NE_IMM16, d, a, imm),
        Op::CmpLtImm(d, a, imm) => pack_cmp_imm(Tag::CmpLtImm, EXT_OP_CMP_LT_IMM16, d, a, imm),
        Op::CmpLeImm(d, a, imm) => pack_cmp_imm(Tag::CmpLeImm, EXT_OP_CMP_LE_IMM16, d, a, imm),
        Op::CmpGtImm(d, a, imm) => pack_cmp_imm(Tag::CmpGtImm, EXT_OP_CMP_GT_IMM16, d, a, imm),
        Op::CmpGeImm(d, a, imm) => pack_cmp_imm(Tag::CmpGeImm, EXT_OP_CMP_GE_IMM16, d, a, imm),
        _ => return None,
    })
}

pub(crate) fn is_cmp_imm16_op(ext_op: u8) -> bool {
    matches!(
        ext_op,
        EXT_OP_CMP_EQ_IMM16
            | EXT_OP_CMP_NE_IMM16
            | EXT_OP_CMP_LT_IMM16
            | EXT_OP_CMP_LE_IMM16
            | EXT_OP_CMP_GT_IMM16
            | EXT_OP_CMP_GE_IMM16
    )
}

pub(crate) fn decode_cmp_imm16_op(ext_op: u8, dst: u16, src: u16, imm: i16) -> Option<Op> {
    Some(match ext_op {
        EXT_OP_CMP_EQ_IMM16 => Op::CmpEqImm(dst, src, imm),
        EXT_OP_CMP_NE_IMM16 => Op::CmpNeImm(dst, src, imm),
        EXT_OP_CMP_LT_IMM16 => Op::CmpLtImm(dst, src, imm),
        EXT_OP_CMP_LE_IMM16 => Op::CmpLeImm(dst, src, imm),
        EXT_OP_CMP_GT_IMM16 => Op::CmpGtImm(dst, src, imm),
        EXT_OP_CMP_GE_IMM16 => Op::CmpGeImm(dst, src, imm),
        _ => return None,
    })
}
