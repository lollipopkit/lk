use super::*;

impl Bc32Function {
    /// Decode back to the standard Function format for execution.
    pub fn decode(&self) -> Function {
        let mut code = Vec::with_capacity(self.code32.len());
        let mut pc = 0usize;
        while pc < self.code32.len() {
            let word = self.code32[pc];
            match decode_tag_byte(tag_of(word)) {
                DecodedTag::RegExt => {
                    pc += 1;
                    continue;
                }
                DecodedTag::Ext => {
                    let Some((op, next_pc)) = decode_ext_op_at(&self.code32, pc) else {
                        break;
                    };
                    code.push(op);
                    pc = next_pc;
                }
                DecodedTag::Regular { tag, flags } => {
                    let mut next = pc + 1;
                    let reg_ext_word = if next < self.code32.len() && tag_of(self.code32[next]) == TAG_REG_EXT {
                        let ext = Some(self.code32[next]);
                        next += 1;
                        ext
                    } else {
                        None
                    };
                    let (hi_a, hi_b, hi_c) = unpack_reg_ext(reg_ext_word);
                    match tag {
                        Tag::ForRangePrep => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let a = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let b = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                            let c = combine_reg(hi_c, (word & 0xFF) as u16);
                            let word2 = self.code32[next];
                            next += 1;
                            let flag_word = ((word2 >> 16) & 0xFF) as u8;
                            let inclusive = (flag_word & 1) != 0;
                            let explicit = (flag_word & 2) != 0;
                            code.push(Op::ForRangePrep {
                                idx: a,
                                limit: b,
                                step: c,
                                inclusive,
                                explicit,
                            });
                            pc = next;
                        }
                        Tag::ForRangeLoop => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let a = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let b = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                            let c = combine_reg(hi_c, (word & 0xFF) as u16);
                            let word2 = self.code32[next];
                            next += 1;
                            let flags = ((word2 >> 16) & 0xFF) as u8;
                            let inclusive = (flags & 1) != 0;
                            let write_idx = (flags & 2) == 0;
                            let ofs = (((((word2 >> 8) & 0xFF) as u16) << 8) | ((word2 & 0xFF) as u16)) as i16;
                            code.push(Op::RangeLoopI {
                                idx: a,
                                limit: b,
                                step: c,
                                inclusive,
                                write_idx,
                                ofs,
                            });
                            pc = next;
                        }
                        Tag::ForRangeStep => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let a = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let b = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                            let word2 = self.code32[next];
                            next += 1;
                            let back_ofs = (((((word2 >> 8) & 0xFF) as u16) << 8) | ((word2 & 0xFF) as u16)) as i16;
                            code.push(Op::ForRangeStep {
                                idx: a,
                                step: b,
                                back_ofs,
                            });
                            pc = next;
                        }
                        Tag::JmpFalseSetX | Tag::JmpTrueSetX | Tag::NullishPickX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let first = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let second = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                            let word2 = self.code32[next];
                            next += 1;
                            let ofs = (((((word2 >> 8) & 0xFF) as u16) << 8) | ((word2 & 0xFF) as u16)) as i16;
                            code.push(match tag {
                                Tag::JmpFalseSetX => Op::JmpFalseSet {
                                    r: first,
                                    dst: second,
                                    ofs,
                                },
                                Tag::JmpTrueSetX => Op::JmpTrueSet {
                                    r: first,
                                    dst: second,
                                    ofs,
                                },
                                Tag::NullishPickX => Op::NullishPick {
                                    l: first,
                                    dst: second,
                                    ofs,
                                },
                                _ => unreachable!(),
                            });
                            pc = next;
                        }
                        Tag::CallX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let f_reg = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let base = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                            let retc = (word & 0xFF) as u8;
                            let word2 = self.code32[next];
                            next += 1;
                            let argc = ((word2 >> 16) & 0xFF) as u8;
                            code.push(Op::Call {
                                f: f_reg,
                                base,
                                argc,
                                retc,
                            });
                            pc = next;
                        }
                        Tag::CallNamedX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let f_reg = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let base_pos = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                            let base_named = combine_reg(hi_c, (word & 0xFF) as u16);
                            let word2 = self.code32[next];
                            next += 1;
                            let posc = ((word2 >> 16) & 0xFF) as u8;
                            let namedc = ((word2 >> 8) & 0xFF) as u8;
                            let retc = (word2 & 0xFF) as u8;
                            code.push(Op::CallNamed {
                                f: f_reg,
                                base_pos,
                                posc,
                                base_named,
                                namedc,
                                retc,
                            });
                            pc = next;
                        }
                        Tag::CmpLtImmJmp | Tag::CmpLeImmJmp | Tag::AddIntImmJmp => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                            let imm = (((word >> 8) & 0xFF) as i8) as i16;
                            let word2 = self.code32[next];
                            next += 1;
                            let ofs = (((((word2 >> 8) & 0xFF) as u16) << 8) | ((word2 & 0xFF) as u16)) as i16;
                            code.push(match tag {
                                Tag::CmpLtImmJmp => Op::CmpLtImmJmp { r, imm, ofs },
                                Tag::CmpLeImmJmp => Op::CmpLeImmJmp { r, imm, ofs },
                                Tag::AddIntImmJmp => Op::AddIntImmJmp { r, imm, ofs },
                                _ => unreachable!(),
                            });
                            pc = next;
                        }
                        _ => {
                            let op = decode_word_with_hi(tag, flags, word, (hi_a, hi_b, hi_c));
                            code.push(op);
                            pc = next;
                        }
                    }
                }
            }
        }
        Function {
            consts: self.consts.clone(),
            code,
            n_regs: self.n_regs,
            protos: self.protos.clone(),
            param_regs: self.param_regs.clone(),
            named_param_regs: self.named_param_regs.clone(),
            named_param_layout: self.named_param_layout.clone(),
            pattern_plans: self.pattern_plans.clone(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }
}
