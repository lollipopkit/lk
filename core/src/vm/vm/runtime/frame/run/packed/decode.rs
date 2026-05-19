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
                let fusion = detect_for_range_fusion(code32, next_pc, idx);
                PackedHotKind::ForRangeLoop {
                    idx,
                    write_idx,
                    ofs,
                    fusion,
                }
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

fn detect_for_range_fusion(code32: &[u32], body_pc: usize, idx: u16) -> Option<PackedRangeFusion> {
    let (mod_op, add_pc) = fetch_packed_op(None, code32, body_pc).ok()?;
    if let Op::Mod(mod_dst, value, modulo) = mod_op
        && value == idx
        && rk_is_const(modulo)
    {
        let (add_op, step_pc) = fetch_packed_op(None, code32, add_pc).ok()?;
        if let Op::Add(acc_dst, acc_src, add_rhs) = add_op
            && acc_dst == acc_src
            && add_rhs == mod_dst
        {
            let (step_op, _) = fetch_packed_op(None, code32, step_pc).ok()?;
            if let Op::ForRangeStep { back_ofs, .. } = step_op {
                return Some(PackedRangeFusion::AddModulo {
                    acc: acc_dst,
                    modulo,
                    step_pc,
                    back_ofs,
                });
            }
        }
    }

    detect_tiny_add_mod_call_fusion(code32, body_pc, idx).or_else(|| detect_tiny_int_call3_fusion(code32, body_pc, idx))
}

fn detect_tiny_int_call3_fusion(code32: &[u32], body_pc: usize, idx: u16) -> Option<PackedRangeFusion> {
    let (move_acc_op, move_idx_pc) = fetch_packed_op(None, code32, body_pc).ok()?;
    let Op::Move(call_base, acc_local) = move_acc_op else {
        return None;
    };

    let (move_idx_op, mod_pc) = fetch_packed_op(None, code32, move_idx_pc).ok()?;
    let Op::Move(call_arg_1, move_idx_src) = move_idx_op else {
        return None;
    };
    if call_arg_1 != call_base + 1 || move_idx_src != idx {
        return None;
    }

    let (mod_op, call_pc) = fetch_packed_op(None, code32, mod_pc).ok()?;
    let (mod_dst, mod_value, modulo) = match mod_op {
        Op::Mod(dst, value, modulo) | Op::ModInt(dst, value, modulo) => (dst, value, modulo),
        _ => return None,
    };
    if mod_dst != call_base + 2 || mod_value != idx || !rk_is_const(modulo) {
        return None;
    }

    let (call_op, store_pc) = fetch_packed_op(None, code32, call_pc).ok()?;
    let Op::Call {
        f,
        base,
        argc: 3,
        retc: 1,
    } = call_op
    else {
        return None;
    };
    if base != call_base {
        return None;
    }

    let (store_op, step_pc) = fetch_packed_op(None, code32, store_pc).ok()?;
    let Op::StoreLocal(store_local, store_src) = store_op else {
        return None;
    };
    if store_local != acc_local || store_src != call_base {
        return None;
    }

    let (step_op, _) = fetch_packed_op(None, code32, step_pc).ok()?;
    let Op::ForRangeStep { back_ofs, .. } = step_op else {
        return None;
    };

    Some(PackedRangeFusion::TinyIntCall3 {
        func: f,
        acc: acc_local,
        modulo,
        call_pc,
        step_pc,
        back_ofs,
    })
}

fn detect_tiny_add_mod_call_fusion(code32: &[u32], body_pc: usize, idx: u16) -> Option<PackedRangeFusion> {
    let (load_op, move_acc_pc) = fetch_packed_op(None, code32, body_pc).ok()?;
    if let Op::Move(call_base, acc_local) = load_op {
        let (mod_op, call_pc) = fetch_packed_op(None, code32, move_acc_pc).ok()?;
        let (mod_dst, mod_value, modulo) = match mod_op {
            Op::Mod(dst, value, modulo) | Op::ModInt(dst, value, modulo) => (dst, value, modulo),
            _ => return None,
        };
        if mod_dst != call_base + 1 || mod_value != idx || !rk_is_const(modulo) {
            return None;
        }

        let (call_op, store_pc) = fetch_packed_op(None, code32, call_pc).ok()?;
        let Op::Call {
            f,
            base,
            argc: 2,
            retc: 1,
        } = call_op
        else {
            return None;
        };
        if base != call_base {
            return None;
        }

        let (store_op, step_pc) = fetch_packed_op(None, code32, store_pc).ok()?;
        let Op::StoreLocal(store_local, store_src) = store_op else {
            return None;
        };
        if store_local != acc_local || store_src != call_base {
            return None;
        }

        let (step_op, _) = fetch_packed_op(None, code32, step_pc).ok()?;
        let Op::ForRangeStep { back_ofs, .. } = step_op else {
            return None;
        };

        return Some(PackedRangeFusion::TinyAddModCall {
            func: f,
            acc: acc_local,
            modulo,
            call_pc,
            step_pc,
            back_ofs,
        });
    }

    let Op::LoadLocal(acc_arg, acc_local) = load_op else {
        return None;
    };

    let (move_acc_op, mod_pc) = fetch_packed_op(None, code32, move_acc_pc).ok()?;
    let Op::Move(call_base, move_acc_src) = move_acc_op else {
        return None;
    };
    if move_acc_src != acc_arg {
        return None;
    }

    let (mod_op, move_mod_pc) = fetch_packed_op(None, code32, mod_pc).ok()?;
    let (mod_dst, mod_value, modulo) = match mod_op {
        Op::Mod(dst, value, modulo) | Op::ModInt(dst, value, modulo) => (dst, value, modulo),
        _ => return None,
    };
    if mod_value != idx || !rk_is_const(modulo) {
        return None;
    }

    let (move_mod_op, call_pc) = fetch_packed_op(None, code32, move_mod_pc).ok()?;
    let Op::Move(call_arg_1, move_mod_src) = move_mod_op else {
        return None;
    };
    if call_arg_1 != call_base + 1 || move_mod_src != mod_dst {
        return None;
    }

    let (call_op, store_pc) = fetch_packed_op(None, code32, call_pc).ok()?;
    let Op::Call {
        f,
        base,
        argc: 2,
        retc: 1,
    } = call_op
    else {
        return None;
    };
    if base != call_base {
        return None;
    }

    let (store_op, step_pc) = fetch_packed_op(None, code32, store_pc).ok()?;
    let Op::StoreLocal(store_local, store_src) = store_op else {
        return None;
    };
    if store_local != acc_local || store_src != call_base {
        return None;
    }

    let (step_op, _) = fetch_packed_op(None, code32, step_pc).ok()?;
    let Op::ForRangeStep { back_ofs, .. } = step_op else {
        return None;
    };

    Some(PackedRangeFusion::TinyAddModCall {
        func: f,
        acc: acc_local,
        modulo,
        call_pc,
        step_pc,
        back_ofs,
    })
}
