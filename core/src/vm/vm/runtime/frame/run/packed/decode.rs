use super::*;

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
pub(super) fn build_hot_slot(code32: &[u32], pc: usize, word: u32, raw_tag: u8) -> Option<PackedHotSlot> {
    if raw_tag == bc32::TAG_EXT {
        let ext_op = ((word >> 16) & 0xFF) as u8;
        let ext = *code32.get(pc + 1)?;
        if bc32::tag_of(ext) != bc32::TAG_EXT {
            return None;
        }
        let reg_ext = code32
            .get(pc + 2)
            .copied()
            .filter(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
        let f = bc32::combine_reg(((ext >> 8) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
        let b = bc32::combine_reg((ext & 0xFF) as u16, (word & 0xFF) as u16);
        let c = bc32::combine_reg(
            reg_ext.map(|word| (word & 0xFF) as u16).unwrap_or(0),
            ((ext >> 16) & 0xFF) as u16,
        );
        let next_pc = if reg_ext.is_some() { pc + 3 } else { pc + 2 };
        let kind = match ext_op {
            bc32::EXT_OP_ADD_INT => PackedHotKind::IntArith {
                op: PackedArithOp::Add,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_ADD_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Add,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_SUB_INT => PackedHotKind::IntArith {
                op: PackedArithOp::Sub,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_SUB_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Sub,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_MUL_INT => PackedHotKind::IntArith {
                op: PackedArithOp::Mul,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_MUL_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Mul,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_DIV_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Div,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_MOD_INT => PackedHotKind::IntArith {
                op: PackedArithOp::Mod,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_MOD_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Mod,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_FLOOR => PackedHotKind::Floor { dst: f, src: b },
            bc32::EXT_OP_STARTS_WITH_K => PackedHotKind::StartsWithK { dst: f, src: b, key: c },
            bc32::EXT_OP_TO_ITER => PackedHotKind::ToIter { dst: f, src: b },
            bc32::EXT_OP_MAP_SET_INTERNED => PackedHotKind::MapSetInterned { map: f, key: b, val: c },
            bc32::EXT_OP_CALL_NATIVE_FAST => PackedHotKind::CallNativeFast {
                f,
                base: b,
                argc: c as u8,
                retc: 1,
            },
            bc32::EXT_OP_LIST_LEN => PackedHotKind::ListLen { dst: f, src: b },
            bc32::EXT_OP_MAP_LEN => PackedHotKind::MapLen { dst: f, src: b },
            bc32::EXT_OP_STR_LEN => PackedHotKind::StrLen { dst: f, src: b },
            bc32::EXT_OP_MAP_GET_INTERNED => PackedHotKind::MapGetInterned { dst: f, map: b, key: c },
            bc32::EXT_OP_MAP_GET_DYNAMIC => PackedHotKind::MapGetDynamic { dst: f, map: b, key: c },
            bc32::EXT_OP_STR_CONCAT_KNOWN_CAP => PackedHotKind::StrConcatKnownCap { dst: f, a: b, b: c },
            _ => return None,
        };
        return Some(PackedHotSlot { word, next_pc, kind });
    }
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
            Tag::Access => {
                let (dst, base, field) = decode_abc(word, reg_ext);
                PackedHotKind::Access { dst, base, field }
            }
            Tag::AccessK => {
                let (dst, base, key) = decode_abc(word, reg_ext);
                PackedHotKind::AccessK { dst, base, key }
            }
            Tag::Index => {
                let (dst, base, idx) = decode_abc(word, reg_ext);
                PackedHotKind::Index { dst, base, idx }
            }
            Tag::Len => {
                let (dst, src, _) = decode_abc(word, reg_ext);
                PackedHotKind::Len { dst, src }
            }
            Tag::BuildList => {
                let (dst, base, len) = decode_abc(word, reg_ext);
                PackedHotKind::BuildList { dst, base, len }
            }
            Tag::BuildMap => {
                let (dst, base, len) = decode_abc(word, reg_ext);
                PackedHotKind::BuildMap { dst, base, len }
            }
            Tag::ForRangePrep => {
                let (idx, limit, step) = decode_abc(word, reg_ext);
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let flags = ((ext_word >> 16) & 0xFF) as u8;
                let inclusive = (flags & 1) != 0;
                let explicit = (flags & 2) != 0;
                next_pc += 1;
                PackedHotKind::ForRangePrep {
                    idx,
                    limit,
                    step,
                    inclusive,
                    explicit,
                }
            }
            Tag::ForRangeLoop => {
                let (idx, _, _) = decode_abc(word, reg_ext);
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                let flags = ((ext_word >> 16) & 0xFF) as u8;
                let write_idx = (flags & 2) == 0;
                next_pc += 1;
                PackedHotKind::ForRangeLoop { idx, write_idx, ofs }
            }
            Tag::ForRangeStep => {
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let back_ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                next_pc += 1;
                let tail = detect_for_range_tail(code32, pc, back_ofs);
                PackedHotKind::ForRangeStep { back_ofs, tail }
            }
            Tag::ToStr => {
                let (dst, src, _) = decode_abc(word, reg_ext);
                if let Some((out, lhs, fused_next_pc)) = decode_tostr_add_rhs(code32, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::ToStrAddRhs {
                        tmp: dst,
                        src,
                        out,
                        lhs,
                        add_pc: next_pc,
                    }
                } else {
                    PackedHotKind::ToStr { dst, src }
                }
            }
            Tag::MakeClosure => {
                let (dst, proto, _) = decode_abc(word, reg_ext);
                PackedHotKind::MakeClosure { dst, proto }
            }
            Tag::Call => {
                let (f, base, argc) = decode_abc(word, reg_ext);
                PackedHotKind::Call {
                    f,
                    base,
                    argc: argc as u8,
                    retc: 1,
                }
            }
            Tag::CallX => {
                let (f, base, _) = decode_abc(word, reg_ext);
                let retc = (word & 0xFF) as u8;
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let argc = ((ext_word >> 16) & 0xFF) as u8;
                next_pc += 1;
                PackedHotKind::Call { f, base, argc, retc }
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
            Tag::Jmp => {
                let ofs = (((word & 0x00FF_FFFF) as i32) << 8 >> 8) as i16;
                PackedHotKind::Jmp { ofs }
            }
            Tag::JmpFalse => {
                let (r, hi, lo) = decode_abc(word, reg_ext);
                let ofs = ((hi << 8) | lo) as i16;
                PackedHotKind::JmpFalse { r, ofs }
            }
            Tag::Ret => {
                let (base, retc, _) = decode_abc(word, reg_ext);
                PackedHotKind::Ret { base, retc: retc as u8 }
            }
            Tag::ListPush => {
                let (list, val, _) = decode_abc(word, reg_ext);
                PackedHotKind::ListPush { list, val }
            }
            Tag::MapSet => {
                let (map, key, val) = decode_abc(word, reg_ext);
                if flags & 1 != 0 {
                    PackedHotKind::MapSetMove { map, key, val }
                } else {
                    PackedHotKind::MapSet { map, key, val }
                }
            }
            Tag::Eq => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::CmpJmp {
                        op: PackedCmpOp::Eq,
                        a,
                        b,
                        ofs,
                    }
                } else {
                    PackedHotKind::Cmp {
                        op: PackedCmpOp::Eq,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Ne => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::CmpJmp {
                        op: PackedCmpOp::Ne,
                        a,
                        b,
                        ofs,
                    }
                } else {
                    PackedHotKind::Cmp {
                        op: PackedCmpOp::Ne,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Lt => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::CmpJmp {
                        op: PackedCmpOp::Lt,
                        a,
                        b,
                        ofs,
                    }
                } else {
                    PackedHotKind::Cmp {
                        op: PackedCmpOp::Lt,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Le => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::CmpJmp {
                        op: PackedCmpOp::Le,
                        a,
                        b,
                        ofs,
                    }
                } else {
                    PackedHotKind::Cmp {
                        op: PackedCmpOp::Le,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Gt => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::CmpJmp {
                        op: PackedCmpOp::Gt,
                        a,
                        b,
                        ofs,
                    }
                } else {
                    PackedHotKind::Cmp {
                        op: PackedCmpOp::Gt,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Ge => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                    next_pc = fused_next_pc;
                    PackedHotKind::CmpJmp {
                        op: PackedCmpOp::Ge,
                        a,
                        b,
                        ofs,
                    }
                } else {
                    PackedHotKind::Cmp {
                        op: PackedCmpOp::Ge,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::CmpLtImmJmp => {
                let (hi_a, _, _) = bc32::unpack_reg_ext(reg_ext);
                let r = bc32::combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                let imm = (((word >> 8) & 0xFF) as i8) as i16;
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                next_pc += 1;
                PackedHotKind::CmpLtImmJmp { r, imm, ofs }
            }
            Tag::CmpLeImmJmp => {
                let (hi_a, _, _) = bc32::unpack_reg_ext(reg_ext);
                let r = bc32::combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                let imm = (((word >> 8) & 0xFF) as i8) as i16;
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                next_pc += 1;
                PackedHotKind::CmpLeImmJmp { r, imm, ofs }
            }
            Tag::AddIntImmJmp => {
                let (hi_a, _, _) = bc32::unpack_reg_ext(reg_ext);
                let r = bc32::combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                let imm = (((word >> 8) & 0xFF) as i8) as i16;
                let ext_word = *code32.get(next_pc)?;
                if bc32::tag_of(ext_word) != bc32::TAG_EXT {
                    return None;
                }
                let ofs = (((((ext_word >> 8) & 0xFF) as u16) << 8) | ((ext_word & 0xFF) as u16)) as i16;
                next_pc += 1;
                PackedHotKind::AddIntImmJmp { r, imm, ofs }
            }
            _ => return None,
        };
        Some(PackedHotSlot { word, next_pc, kind })
    } else {
        None
    }
}

#[inline(always)]
fn decode_cmp_jmp(code32: &[u32], cmp_pc: usize, jmp_pc: usize, dst: u16) -> Option<(i16, usize)> {
    let (op, next_pc) = fetch_packed_op(None, code32, jmp_pc).ok()?;
    let (r, ofs) = match op {
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => (r, ofs),
        _ => return None,
    };
    if r != dst {
        return None;
    }
    let target = (jmp_pc as isize) + (ofs as isize);
    let fused_ofs = target - (cmp_pc as isize);
    if !(i16::MIN as isize..=i16::MAX as isize).contains(&fused_ofs) {
        return None;
    }
    Some((fused_ofs as i16, next_pc))
}

#[inline(always)]
fn decode_tostr_add_rhs(code32: &[u32], add_pc: usize, tmp: u16) -> Option<(u16, u16, usize)> {
    let add_word = *code32.get(add_pc)?;
    let bc32::DecodedTag::Regular { tag: Tag::Add, flags } = bc32::decode_tag_byte(bc32::tag_of(add_word)) else {
        return None;
    };

    let mut next_pc = add_pc + 1;
    let mut reg_ext = None;
    if next_pc < code32.len() && bc32::tag_of(code32[next_pc]) == bc32::TAG_REG_EXT {
        reg_ext = Some(code32[next_pc]);
        next_pc += 1;
    }

    let (out, lhs, rhs) = decode_rk_pair(add_word, reg_ext, flags);
    if rk_is_const(rhs) || rhs != tmp {
        return None;
    }
    Some((out, lhs, next_pc))
}

fn detect_for_range_tail(code32: &[u32], step_pc: usize, back_ofs: i16) -> Option<PackedRangeTail> {
    let guard_pc = ((step_pc as isize) + (back_ofs as isize)) as usize;
    let (guard_op, body_pc) = fetch_packed_op(None, code32, guard_pc).ok()?;
    let (idx, write_idx, ofs) = match guard_op {
        Op::ForRangeLoop {
            idx, write_idx, ofs, ..
        }
        | Op::RangeLoopI {
            idx, write_idx, ofs, ..
        } => (idx, write_idx, ofs),
        _ => return None,
    };
    Some(PackedRangeTail {
        guard_pc,
        body_pc,
        exit_pc: ((guard_pc as isize) + (ofs as isize)) as usize,
        idx,
        write_idx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::vm::bc32::Bc32Function;

    fn function_with(code: Vec<Op>) -> Function {
        Function {
            consts: vec![Val::Int(1)],
            code,
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    #[test]
    fn packed_hot_slot_fuses_dynamic_compare_followed_by_jmp_false() {
        let function = function_with(vec![
            Op::CmpLt(2, 0, 1),
            Op::JmpFalse(2, 2),
            Op::LoadK(3, 0),
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp+jmp must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(&bc.code32, 0, word, bc32::tag_of(word)).expect("cmp hot slot");

        match slot.kind {
            PackedHotKind::CmpJmp {
                op: PackedCmpOp::Lt,
                a: 0,
                b: 1,
                ofs,
            } => assert_eq!(ofs, 3),
            _ => panic!("expected CmpJmp hot slot"),
        }
        assert_eq!(slot.next_pc, 2, "fused slot must skip the following JmpFalse word");
    }

    #[test]
    fn packed_for_range_step_carries_generic_tail_guard() {
        let function = function_with(vec![
            Op::ForRangePrep {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: false,
                explicit: false,
            },
            Op::ForRangeLoop {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: false,
                write_idx: true,
                ofs: 3,
            },
            Op::AddIntImm(3, 3, 1),
            Op::ForRangeStep {
                idx: 0,
                step: 2,
                back_ofs: -2,
            },
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("range loop must be BC32 encodable");
        let step_pc = 5;
        let word = bc.code32[step_pc];
        let slot = build_hot_slot(&bc.code32, step_pc, word, bc32::tag_of(word)).expect("range step hot slot");

        match slot.kind {
            PackedHotKind::ForRangeStep { tail: Some(tail), .. } => {
                assert_eq!(tail.guard_pc, 2);
                assert_eq!(tail.body_pc, 4);
                assert_eq!(tail.exit_pc, 7);
                assert_eq!(tail.idx, 0);
                assert!(tail.write_idx);
            }
            _ => panic!("expected generic range tail guard metadata"),
        }
    }
}
