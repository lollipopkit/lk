use super::*;

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
                let write_idx = (flags & 2) == 0;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::ForRangeLoop {
                    idx,
                    limit,
                    step,
                    inclusive,
                    write_idx,
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
            Tag::CmpLtImmJmp => {
                let r = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let imm = (((w >> 8) & 0xFF) as i8) as i16;
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for CmpLtImmJmp"))?;
                next += 1;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::CmpLtImmJmp { r, imm, ofs }
            }
            Tag::CmpLeImmJmp => {
                let r = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let imm = (((w >> 8) & 0xFF) as i8) as i16;
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for CmpLeImmJmp"))?;
                next += 1;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::CmpLeImmJmp { r, imm, ofs }
            }
            Tag::AddIntImmJmp => {
                let r = bc32::combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                let imm = (((w >> 8) & 0xFF) as i8) as i16;
                let w2 = *code32
                    .get(next)
                    .ok_or_else(|| anyhow!("bc32: missing Ext for AddIntImmJmp"))?;
                next += 1;
                let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                Op::AddIntImmJmp { r, imm, ofs }
            }
            _ => bc32::decode_word_with_hi(tag, flags, w, (hi_a, hi_b, hi_c)),
        },
    };

    Ok((op, next))
}

#[inline(always)]
pub(super) fn fetch_packed_op(decoded: Option<&Bc32Decoded>, code32: &[u32], pc: usize) -> anyhow::Result<(Op, usize)> {
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
