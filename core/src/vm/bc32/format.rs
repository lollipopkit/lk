use super::{Bc32Reject, EncodedOp, ensure_i8_range};
use crate::vm::bytecode::{IntCmpKind, Op, rk_make_const};

// Common 8-bit tags for encodable ops. Layout: [tag:8 | a:8 | b:8 | c:8]
pub(crate) const RAW_TAG_EXT: u8 = 0xFF;
pub(crate) const RAW_TAG_REG_EXT: u8 = 0xFE;
pub(crate) const TAG_FLAG_MASK: u8 = 0x03;
pub(crate) const TAG_FLAG_SHIFT: u8 = 2;
pub(crate) const RK_FLAG_B: u8 = 0x01;
pub(crate) const RK_FLAG_C: u8 = 0x02;
pub(crate) const EXT_OP_FLOOR: u8 = 1;
pub(crate) const EXT_OP_STARTS_WITH_K: u8 = 2;
pub(crate) const EXT_OP_CONTAINS_K: u8 = 3;
pub(crate) const EXT_OP_TO_ITER: u8 = 4;
pub(crate) const EXT_OP_MAP_HAS_K: u8 = 5;
pub(crate) const EXT_OP_ADD_INT: u8 = 6;
pub(crate) const EXT_OP_ADD_FLOAT: u8 = 7;
pub(crate) const EXT_OP_SUB_INT: u8 = 8;
pub(crate) const EXT_OP_SUB_FLOAT: u8 = 9;
pub(crate) const EXT_OP_MUL_INT: u8 = 10;
pub(crate) const EXT_OP_MUL_FLOAT: u8 = 11;
pub(crate) const EXT_OP_DIV_FLOAT: u8 = 12;
pub(crate) const EXT_OP_MOD_INT: u8 = 13;
pub(crate) const EXT_OP_MOD_FLOAT: u8 = 14;
pub(crate) const EXT_OP_LIST_LEN: u8 = 15;
pub(crate) const EXT_OP_MAP_LEN: u8 = 16;
pub(crate) const EXT_OP_STR_LEN: u8 = 17;
pub(crate) const EXT_OP_LIST_INDEX_I: u8 = 18;
pub(crate) const EXT_OP_STR_INDEX_I: u8 = 19;
pub(crate) const EXT_OP_MAP_GET_INTERNED: u8 = 20;
pub(crate) const EXT_OP_MAP_SET_INTERNED: u8 = 21;
pub(crate) const EXT_OP_MAP_GET_DYNAMIC: u8 = 22;
pub(crate) const EXT_OP_STR_CONCAT_KNOWN_CAP: u8 = 23;
pub(crate) const EXT_OP_LIST_SET_I: u8 = 24;
pub(crate) const EXT_OP_CALL_NATIVE_FAST: u8 = 25;
pub(crate) const EXT_OP_CMP_I: u8 = 26;
pub(crate) const EXT_OP_CALL_CLOSURE_EXACT: u8 = 27;
pub(crate) const EXT_OP_CALL_EXACT: u8 = 28;
pub(crate) const EXT_OP_CALL_NAMED_FALLBACK: u8 = 29;
pub(crate) const EXT_OP_CALL_METHOD0: u8 = 30;
pub(crate) const EXT_OP_CALL_GLOBAL_METHOD0: u8 = 31;
pub(crate) const EXT_OP_STR_CONCAT_TO_STR: u8 = 32;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tag {
    Move = 0,
    LoadK,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    AddIntImm,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    CmpEqImm,
    CmpNeImm,
    CmpLtImm,
    CmpLeImm,
    CmpGtImm,
    CmpGeImm,
    Jmp,
    JmpFalse,
    ToBool,
    Not,
    Len,
    Index,
    ToStr,
    JmpIfNil,
    JmpIfNotNil,
    NullishPick,
    Ret,
    LoadGlobal,
    DefineGlobal,
    Access,
    AccessK,
    IndexK,
    LoadLocal,
    StoreLocal,
    Call,
    LoadCapture,
    JmpFalseSet,
    JmpTrueSet,
    ListSlice,
    JmpFalseSetX,
    JmpTrueSetX,
    NullishPickX,
    ForRangePrep,
    ForRangeLoop,
    ForRangeStep,
    Break,
    Continue,
    CallX,
    PatternMatch,
    PatternMatchOrFail,
    PatternMatchOrFailConst,
    BuildList,
    BuildMap,
    MakeClosure,
    CallNamedX,
    ListPush,
    MapSet,
    CmpLtImmJmp,
    CmpLeImmJmp,
    AddIntImmJmp,
}

impl Tag {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Tag::Move,
            1 => Tag::LoadK,
            2 => Tag::Add,
            3 => Tag::Sub,
            4 => Tag::Mul,
            5 => Tag::Div,
            6 => Tag::Mod,
            7 => Tag::AddIntImm,
            8 => Tag::Eq,
            9 => Tag::Ne,
            10 => Tag::Lt,
            11 => Tag::Le,
            12 => Tag::Gt,
            13 => Tag::Ge,
            14 => Tag::CmpEqImm,
            15 => Tag::CmpNeImm,
            16 => Tag::CmpLtImm,
            17 => Tag::CmpLeImm,
            18 => Tag::CmpGtImm,
            19 => Tag::CmpGeImm,
            20 => Tag::Jmp,
            21 => Tag::JmpFalse,
            22 => Tag::ToBool,
            23 => Tag::Not,
            24 => Tag::Len,
            25 => Tag::Index,
            26 => Tag::ToStr,
            27 => Tag::JmpIfNil,
            28 => Tag::JmpIfNotNil,
            29 => Tag::NullishPick,
            30 => Tag::Ret,
            31 => Tag::LoadGlobal,
            32 => Tag::DefineGlobal,
            33 => Tag::Access,
            34 => Tag::AccessK,
            35 => Tag::IndexK,
            36 => Tag::LoadLocal,
            37 => Tag::StoreLocal,
            38 => Tag::Call,
            39 => Tag::LoadCapture,
            40 => Tag::JmpFalseSet,
            41 => Tag::JmpTrueSet,
            42 => Tag::ListSlice,
            43 => Tag::JmpFalseSetX,
            44 => Tag::JmpTrueSetX,
            45 => Tag::NullishPickX,
            46 => Tag::ForRangePrep,
            47 => Tag::ForRangeLoop,
            48 => Tag::ForRangeStep,
            49 => Tag::Break,
            50 => Tag::Continue,
            51 => Tag::CallX,
            52 => Tag::PatternMatch,
            53 => Tag::PatternMatchOrFail,
            54 => Tag::PatternMatchOrFailConst,
            55 => Tag::BuildList,
            56 => Tag::BuildMap,
            57 => Tag::MakeClosure,
            58 => Tag::CallNamedX,
            59 => Tag::ListPush,
            60 => Tag::MapSet,
            61 => Tag::CmpLtImmJmp,
            62 => Tag::CmpLeImmJmp,
            63 => Tag::AddIntImmJmp,
            _ => return None,
        })
    }
}

#[inline]
pub(crate) const fn encode_tag_raw(tag: Tag) -> u8 {
    (tag as u8) << TAG_FLAG_SHIFT
}

#[inline]
pub(crate) const fn encode_tag_with_flags(tag: Tag, flags: u8) -> u8 {
    encode_tag_raw(tag) | (flags & TAG_FLAG_MASK)
}

pub(crate) enum DecodedTag {
    Regular { tag: Tag, flags: u8 },
    RegExt,
    Ext,
}

#[inline]
pub(crate) fn decode_tag_byte(byte: u8) -> DecodedTag {
    if byte == RAW_TAG_REG_EXT {
        DecodedTag::RegExt
    } else if byte == RAW_TAG_EXT {
        DecodedTag::Ext
    } else {
        let base = byte >> TAG_FLAG_SHIFT;
        let flags = byte & TAG_FLAG_MASK;
        if let Some(tag) = Tag::from_u8(base) {
            DecodedTag::Regular { tag, flags }
        } else {
            DecodedTag::Ext
        }
    }
}

#[inline]
pub(crate) fn pack(tag: Tag, flags: u8, a: u8, b: u8, c: u8) -> u32 {
    ((encode_tag_with_flags(tag, flags) as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

#[inline]
pub(super) fn pack_ext_op(op: u8, a: u16, b: u16, c: u16) -> Result<EncodedOp, Bc32Reject> {
    let word = ((RAW_TAG_EXT as u32) << 24) | ((op as u32) << 16) | (((a as u8) as u32) << 8) | ((b as u8) as u32);
    let ext = pack_ext_word(c as u8, (a >> 8) as u8, (b >> 8) as u8);
    let c_hi = c >> 8;
    if c_hi == 0 {
        Ok(EncodedOp::new(word, Some(ext)))
    } else {
        Ok(EncodedOp::with_extra(
            word,
            [ext, pack_reg_ext_bits(0, 0, c).expect("c_hi was non-zero")],
        ))
    }
}

#[inline]
pub(super) fn pack_ext_op_i8(op: u8, opcode: &'static str, a: u16, b: u16, c: i16) -> Result<EncodedOp, Bc32Reject> {
    ensure_i8_range(opcode, "index", c as i32)?;
    let word = ((RAW_TAG_EXT as u32) << 24) | ((op as u32) << 16) | (((a as u8) as u32) << 8) | ((b as u8) as u32);
    Ok(EncodedOp::new(
        word,
        Some(pack_ext_word(c as i8 as u8, (a >> 8) as u8, (b >> 8) as u8)),
    ))
}

#[inline]
pub(super) fn pack_ext_op_i16_reg(op: u8, a: u16, b: u16, index: i16, d: u16) -> Result<EncodedOp, Bc32Reject> {
    let word = ((RAW_TAG_EXT as u32) << 24) | ((op as u32) << 16) | (((a as u8) as u32) << 8) | ((b as u8) as u32);
    let index_raw = index as u16;
    Ok(EncodedOp::with_extra(
        word,
        [
            pack_ext_word((index_raw >> 8) as u8, index_raw as u8, d as u8),
            pack_ext_word((a >> 8) as u8, (b >> 8) as u8, (d >> 8) as u8),
        ],
    ))
}

#[inline]
pub(super) fn pack_cmp_i(dst: u16, a: u16, b: u16, kind: IntCmpKind) -> EncodedOp {
    let word =
        ((RAW_TAG_EXT as u32) << 24) | ((EXT_OP_CMP_I as u32) << 16) | (((dst as u8) as u32) << 8) | (a as u8 as u32);
    EncodedOp::with_extra(
        word,
        [
            pack_ext_word(kind as u8, 0, b as u8),
            pack_ext_word((dst >> 8) as u8, (a >> 8) as u8, (b >> 8) as u8),
        ],
    )
}

#[inline]
pub(super) fn pack_call_ext(
    ext_op: u8,
    opcode: &'static str,
    f: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<EncodedOp, Bc32Reject> {
    if retc != 1 {
        return Err(Bc32Reject::UnsupportedOpcode {
            opcode,
            detail: "BC32 typed call currently supports retc=1",
        });
    }
    pack_ext_op(ext_op, f, base, argc as u16)
}

#[inline]
pub(super) fn pack_call_named_fallback(
    f: u16,
    base_pos: u16,
    posc: u8,
    base_named: u16,
    namedc: u8,
    retc: u8,
) -> EncodedOp {
    let word = ((RAW_TAG_EXT as u32) << 24)
        | ((EXT_OP_CALL_NAMED_FALLBACK as u32) << 16)
        | (((f as u8) as u32) << 8)
        | (base_pos as u8 as u32);
    EncodedOp::with_extra3(
        word,
        [
            pack_ext_word(base_named as u8, posc, namedc),
            pack_ext_word(retc, (f >> 8) as u8, (base_pos >> 8) as u8),
            pack_ext_word((base_named >> 8) as u8, 0, 0),
        ],
    )
}

#[inline]
pub(crate) fn pack_reg_ext_bits(a: u16, b: u16, c: u16) -> Option<u32> {
    let hi_a = (a >> 8) as u8;
    let hi_b = (b >> 8) as u8;
    let hi_c = (c >> 8) as u8;
    if hi_a == 0 && hi_b == 0 && hi_c == 0 {
        None
    } else {
        Some(((RAW_TAG_REG_EXT as u32) << 24) | ((hi_a as u32) << 16) | ((hi_b as u32) << 8) | (hi_c as u32))
    }
}

#[inline]
pub(crate) fn pack_ext_word(a: u8, b: u8, c: u8) -> u32 {
    ((RAW_TAG_EXT as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

#[inline]
pub(crate) fn decode_ext_op(word: u32, ext: u32, c_hi: u16) -> Option<Op> {
    let op = ((word >> 16) & 0xFF) as u8;
    let a = combine_reg(((ext >> 8) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
    let b = combine_reg((ext & 0xFF) as u16, (word & 0xFF) as u16);
    let c = combine_reg(c_hi, ((ext >> 16) & 0xFF) as u16);
    match op {
        EXT_OP_FLOOR => Some(Op::Floor { dst: a, src: b }),
        EXT_OP_STARTS_WITH_K => Some(Op::StartsWithK(a, b, c)),
        EXT_OP_CONTAINS_K => Some(Op::ContainsK(a, b, c)),
        EXT_OP_TO_ITER => Some(Op::ToIter { dst: a, src: b }),
        EXT_OP_MAP_HAS_K => Some(Op::MapHasK(a, b, c)),
        EXT_OP_ADD_INT => Some(Op::AddInt(a, b, c)),
        EXT_OP_ADD_FLOAT => Some(Op::AddFloat(a, b, c)),
        EXT_OP_SUB_INT => Some(Op::SubInt(a, b, c)),
        EXT_OP_SUB_FLOAT => Some(Op::SubFloat(a, b, c)),
        EXT_OP_MUL_INT => Some(Op::MulInt(a, b, c)),
        EXT_OP_MUL_FLOAT => Some(Op::MulFloat(a, b, c)),
        EXT_OP_DIV_FLOAT => Some(Op::DivFloat(a, b, c)),
        EXT_OP_MOD_INT => Some(Op::ModInt(a, b, c)),
        EXT_OP_MOD_FLOAT => Some(Op::ModFloat(a, b, c)),
        EXT_OP_LIST_LEN => Some(Op::ListLen { dst: a, src: b }),
        EXT_OP_MAP_LEN => Some(Op::MapLen { dst: a, src: b }),
        EXT_OP_STR_LEN => Some(Op::StrLen { dst: a, src: b }),
        EXT_OP_LIST_INDEX_I => Some(Op::ListIndexI(a, b, c as u8 as i8 as i16)),
        EXT_OP_STR_INDEX_I => Some(Op::StrIndexI(a, b, c as u8 as i8 as i16)),
        EXT_OP_MAP_GET_INTERNED => Some(Op::MapGetInterned(a, b, c)),
        EXT_OP_MAP_SET_INTERNED => Some(Op::MapSetInterned(a, b, c)),
        EXT_OP_MAP_GET_DYNAMIC => Some(Op::MapGetDynamic(a, b, c)),
        EXT_OP_STR_CONCAT_KNOWN_CAP => Some(Op::StrConcatKnownCap(a, b, c)),
        EXT_OP_STR_CONCAT_TO_STR => Some(Op::StrConcatToStr(a, b, c)),
        EXT_OP_CALL_NATIVE_FAST => Some(Op::CallNativeFast {
            f: a,
            base: b,
            argc: c as u8,
            retc: 1,
        }),
        EXT_OP_CALL_CLOSURE_EXACT => Some(Op::CallClosureExact {
            f: a,
            base: b,
            argc: c as u8,
            retc: 1,
        }),
        EXT_OP_CALL_EXACT => Some(Op::CallExact {
            f: a,
            base: b,
            argc: c as u8,
            retc: 1,
        }),
        EXT_OP_CALL_METHOD0 => Some(Op::CallMethod0 {
            dst: a,
            receiver: b,
            method: c,
        }),
        EXT_OP_CALL_GLOBAL_METHOD0 => Some(Op::CallGlobalMethod0 {
            dst: a,
            receiver: b,
            method: c,
        }),
        _ => None,
    }
}

#[inline]
pub(crate) fn decode_ext_op_at(code32: &[u32], pc: usize) -> Option<(Op, usize)> {
    let word = *code32.get(pc)?;
    let ext = *code32.get(pc + 1)?;
    if ((ext >> 24) & 0xFF) as u8 != RAW_TAG_EXT {
        return None;
    }
    let op = ((word >> 16) & 0xFF) as u8;
    if op != EXT_OP_LIST_SET_I && op != EXT_OP_CMP_I && op != EXT_OP_CALL_NAMED_FALLBACK {
        let reg_ext = code32
            .get(pc + 2)
            .copied()
            .filter(|word| ((word >> 24) & 0xFF) as u8 == RAW_TAG_REG_EXT);
        let c_hi = reg_ext.map(|word| (word & 0xFF) as u16).unwrap_or(0);
        let next_pc = if reg_ext.is_some() { pc + 3 } else { pc + 2 };
        return decode_ext_op(word, ext, c_hi).map(|op| (op, next_pc));
    }
    let ext2 = *code32.get(pc + 2)?;
    if ((ext2 >> 24) & 0xFF) as u8 != RAW_TAG_EXT {
        return None;
    }
    if op == EXT_OP_CMP_I {
        let dst = combine_reg(((ext2 >> 16) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
        let a = combine_reg(((ext2 >> 8) & 0xFF) as u16, (word & 0xFF) as u16);
        let b = combine_reg((ext2 & 0xFF) as u16, (ext & 0xFF) as u16);
        let kind = IntCmpKind::from_u8(((ext >> 16) & 0xFF) as u8)?;
        return Some((Op::CmpI { dst, a, b, kind }, pc + 3));
    }
    if op == EXT_OP_CALL_NAMED_FALLBACK {
        let ext3 = *code32.get(pc + 3)?;
        if ((ext3 >> 24) & 0xFF) as u8 != RAW_TAG_EXT {
            return None;
        }
        let f = combine_reg(((ext2 >> 8) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
        let base_pos = combine_reg((ext2 & 0xFF) as u16, (word & 0xFF) as u16);
        let base_named = combine_reg(((ext3 >> 16) & 0xFF) as u16, ((ext >> 16) & 0xFF) as u16);
        return Some((
            Op::CallNamedFallback {
                f,
                base_pos,
                posc: ((ext >> 8) & 0xFF) as u8,
                base_named,
                namedc: (ext & 0xFF) as u8,
                retc: ((ext2 >> 16) & 0xFF) as u8,
            },
            pc + 4,
        ));
    }
    let dst = combine_reg(((ext2 >> 16) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
    let list = combine_reg(((ext2 >> 8) & 0xFF) as u16, (word & 0xFF) as u16);
    let index = (((((ext >> 16) & 0xFF) as u16) << 8) | (((ext >> 8) & 0xFF) as u16)) as i16;
    let val = combine_reg((ext2 & 0xFF) as u16, (ext & 0xFF) as u16);
    Some((Op::ListSetI { dst, list, index, val }, pc + 3))
}

#[inline]
pub(crate) fn unpack_reg_ext(word: Option<u32>) -> (u16, u16, u16) {
    if let Some(ext) = word {
        let hi_a = ((ext >> 16) & 0xFF) as u16;
        let hi_b = ((ext >> 8) & 0xFF) as u16;
        let hi_c = (ext & 0xFF) as u16;
        (hi_a, hi_b, hi_c)
    } else {
        (0, 0, 0)
    }
}

#[inline]
pub(crate) fn combine_reg(hi: u16, lo: u16) -> u16 {
    (hi << 8) | (lo & 0xFF)
}

#[inline]
pub(crate) fn combine_rk(hi: u16, lo: u16, is_const: bool) -> u16 {
    let value = combine_reg(hi, lo);
    if is_const { rk_make_const(value) } else { value }
}

#[inline]
pub(crate) fn encode_i16(x: i16) -> (u8, u8) {
    (((x as u16) >> 8) as u8, (x as u8))
}
