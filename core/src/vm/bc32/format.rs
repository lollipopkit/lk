use super::{Bc32Reject, EncodedOp, ensure_u8};
use crate::vm::bytecode::{Op, rk_make_const};

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
    ensure_u8("ExtOp", "arg2", c)?;
    let word = ((RAW_TAG_EXT as u32) << 24) | ((op as u32) << 16) | (((a as u8) as u32) << 8) | ((b as u8) as u32);
    Ok(EncodedOp::new(
        word,
        Some(pack_ext_word(c as u8, (a >> 8) as u8, (b >> 8) as u8)),
    ))
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
pub(crate) fn decode_ext_op(word: u32, ext: u32) -> Option<Op> {
    let op = ((word >> 16) & 0xFF) as u8;
    let a = combine_reg(((ext >> 8) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
    let b = combine_reg((ext & 0xFF) as u16, (word & 0xFF) as u16);
    let c = ((ext >> 16) & 0xFF) as u16;
    match op {
        EXT_OP_FLOOR => Some(Op::Floor { dst: a, src: b }),
        EXT_OP_STARTS_WITH_K => Some(Op::StartsWithK(a, b, c)),
        EXT_OP_CONTAINS_K => Some(Op::ContainsK(a, b, c)),
        EXT_OP_TO_ITER => Some(Op::ToIter { dst: a, src: b }),
        _ => None,
    }
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
