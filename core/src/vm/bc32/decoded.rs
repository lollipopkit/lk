use super::*;

#[derive(Debug, Clone)]
pub struct Bc32Decoded {
    pub instrs: Vec<Bc32DecodedInstr>,
    pub word_to_instr: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Bc32DecodedInstr {
    pub op: Op,
    pub next_pc: usize,
}

impl Bc32Decoded {
    pub fn from_words(code32: &[u32]) -> Option<Self> {
        let mut instrs = Vec::with_capacity(code32.len());
        let mut word_to_instr = vec![u32::MAX; code32.len()];
        let mut pc = 0usize;

        while pc < code32.len() {
            let word = code32[pc];
            let tag = tag_of(word);

            if tag == TAG_REG_EXT {
                pc += 1;
                continue;
            }

            if tag == TAG_EXT {
                let ext = *code32.get(pc + 1)?;
                if tag_of(ext) != TAG_EXT {
                    return None;
                }
                let op = decode_ext_op(word, ext)?;
                let instr_idx = instrs.len() as u32;
                word_to_instr[pc] = instr_idx;
                instrs.push(Bc32DecodedInstr { op, next_pc: pc + 2 });
                pc += 2;
                continue;
            }

            let mut next = pc + 1;
            let reg_ext_word = if next < code32.len() && tag_of(code32[next]) == TAG_REG_EXT {
                let ext = code32[next];
                next += 1;
                Some(ext)
            } else {
                None
            };
            let (hi_a, hi_b, hi_c) = unpack_reg_ext(reg_ext_word);

            let op = match tag {
                x if x == TAG_FOR_RANGE_PREP => {
                    let idx = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let limit = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let step = combine_reg(hi_c, (word & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
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
                x if x == TAG_FOR_RANGE_LOOP => {
                    let idx = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let limit = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let step = combine_reg(hi_c, (word & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let flags = ((w2 >> 16) & 0xFF) as u8;
                    let ofs_raw = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    let inclusive = (flags & 1) != 0;
                    let write_idx = (flags & 2) == 0;
                    Op::ForRangeLoop {
                        idx,
                        limit,
                        step,
                        inclusive,
                        write_idx,
                        ofs: ofs_raw,
                    }
                }
                x if x == TAG_FOR_RANGE_STEP => {
                    let idx = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let step = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let back_ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::ForRangeStep { idx, step, back_ofs }
                }
                x if x == TAG_JMP_FALSE_SET_X => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let dst = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::JmpFalseSet { r, dst, ofs }
                }
                x if x == TAG_JMP_TRUE_SET_X => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let dst = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::JmpTrueSet { r, dst, ofs }
                }
                x if x == TAG_NULLISH_PICK_X => {
                    let left = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let dst = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::NullishPick { l: left, dst, ofs }
                }
                x if x == TAG_CALL_X => {
                    let f = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let base = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let retc = (word & 0xFF) as u8;
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let argc = ((w2 >> 16) & 0xFF) as u8;
                    Op::Call { f, base, argc, retc }
                }
                x if x == TAG_CALL_NAMED_X => {
                    let f = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let base_pos = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let base_named = combine_reg(hi_c, (word & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
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
                x if x == encode_tag_raw(Tag::CmpLtImmJmp) => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let imm = (((word >> 8) & 0xFF) as i8) as i16;
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::CmpLtImmJmp { r, imm, ofs }
                }
                x if x == encode_tag_raw(Tag::CmpLeImmJmp) => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let imm = (((word >> 8) & 0xFF) as i8) as i16;
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::CmpLeImmJmp { r, imm, ofs }
                }
                x if x == encode_tag_raw(Tag::AddIntImmJmp) => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let imm = (((word >> 8) & 0xFF) as i8) as i16;
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::AddIntImmJmp { r, imm, ofs }
                }
                _ => match decode_tag_byte(tag) {
                    DecodedTag::Regular { tag: base, flags } => {
                        decode_word_with_hi(base, flags, word, (hi_a, hi_b, hi_c))
                    }
                    _ => Op::Jmp(0),
                },
            };

            let instr_idx = instrs.len() as u32;
            if pc < word_to_instr.len() {
                word_to_instr[pc] = instr_idx;
            }
            instrs.push(Bc32DecodedInstr { op, next_pc: next });
            pc = next;
        }

        Some(Self { instrs, word_to_instr })
    }
}
