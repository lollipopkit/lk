use super::*;

type MoveCallDecode = (Vec<(u16, u16)>, u16, u16, u8, u8, PackedHotCallKind, usize);

pub(super) fn decode_move_call(decoded: Option<&Bc32Decoded>, pc: usize) -> Option<MoveCallDecode> {
    let decoded = decoded?;
    let mut instr_idx = decoded.word_to_instr.get(pc).copied()? as usize;
    let mut moves = Vec::new();
    while let Some(instr) = decoded.instrs.get(instr_idx) {
        match instr.op {
            Op::Move(dst, src) => {
                moves.push((dst, src));
                instr_idx += 1;
            }
            Op::Call { f, base, argc, retc } => {
                return moves_target_call_window(&moves, base, argc).then_some((
                    moves,
                    f,
                    base,
                    argc,
                    retc,
                    PackedHotCallKind::Generic,
                    instr.next_pc,
                ));
            }
            Op::CallClosureExact { f, base, argc, retc } => {
                return moves_target_call_window(&moves, base, argc).then_some((
                    moves,
                    f,
                    base,
                    argc,
                    retc,
                    PackedHotCallKind::ClosureExact,
                    instr.next_pc,
                ));
            }
            Op::CallExact { f, base, argc, retc } => {
                return moves_target_call_window(&moves, base, argc).then_some((
                    moves,
                    f,
                    base,
                    argc,
                    retc,
                    PackedHotCallKind::Exact,
                    instr.next_pc,
                ));
            }
            _ => return None,
        }
    }
    None
}

fn moves_target_call_window(moves: &[(u16, u16)], base: u16, argc: u8) -> bool {
    let Some(end) = base.checked_add(argc as u16) else {
        return false;
    };
    !moves.is_empty() && moves.iter().all(|(dst, _)| *dst >= base && *dst < end)
}

pub(super) fn decode_following_move(code32: &[u32], pc: usize) -> Option<(u16, u16, usize)> {
    let word = *code32.get(pc)?;
    let bc32::DecodedTag::Regular {
        tag: Tag::Move,
        flags: 0,
    } = bc32::decode_tag_byte(bc32::tag_of(word))
    else {
        return None;
    };
    let reg_ext = code32
        .get(pc + 1)
        .copied()
        .filter(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
    let (dst, src, _) = decode_abc(word, reg_ext);
    let next_pc = if reg_ext.is_some() { pc + 2 } else { pc + 1 };
    Some((dst, src, next_pc))
}

pub(super) fn decode_following_bool_branch(
    code32: &[u32],
    origin_pc: usize,
    branch_pc: usize,
    expected_reg: u16,
) -> Option<(i16, usize)> {
    let (op, next_pc) = fetch_packed_op(None, code32, branch_pc).ok()?;
    let (reg, ofs) = match op {
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => (r, ofs),
        _ => return None,
    };
    if reg != expected_reg {
        return None;
    }
    let target = branch_pc as isize + ofs as isize;
    let fused_ofs = i16::try_from(target - origin_pc as isize).ok()?;
    Some((fused_ofs, next_pc))
}

pub(super) fn decode_map_get_cmp_jmp(
    code32: &[u32],
    cmp_pc: usize,
    value_dst: u16,
) -> Option<(PackedCmpOp, u16, usize, usize)> {
    let (cmp_op, branch_pc) = fetch_packed_op(None, code32, cmp_pc).ok()?;
    let (op, cmp_dst, a, b) = match cmp_op {
        Op::CmpEq(dst, a, b) => (PackedCmpOp::Eq, dst, a, b),
        Op::CmpNe(dst, a, b) => (PackedCmpOp::Ne, dst, a, b),
        _ => return None,
    };
    let rhs = if a == value_dst {
        b
    } else if b == value_dst {
        a
    } else {
        return None;
    };
    let (branch_op, next_pc) = fetch_packed_op(None, code32, branch_pc).ok()?;
    let (branch_reg, ofs) = match branch_op {
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => (r, ofs),
        _ => return None,
    };
    if branch_reg != cmp_dst {
        return None;
    }
    let target = (branch_pc as isize) + (ofs as isize);
    if target < 0 {
        return None;
    }
    Some((op, rhs, target as usize, next_pc))
}

pub(super) fn decode_following_cmp_int_jmp(code32: &[u32], pc: usize) -> Option<(PackedCmpOp, u16, u16, i16, usize)> {
    let word = *code32.get(pc)?;
    if bc32::tag_of(word) != bc32::TAG_EXT {
        return None;
    }
    let ext_op = ((word >> 16) & 0xFF) as u8;
    if ext_op != bc32::EXT_OP_CMP_I_JMP {
        return None;
    }
    let ext = *code32.get(pc + 1)?;
    let ext2 = *code32.get(pc + 2)?;
    if bc32::tag_of(ext) != bc32::TAG_EXT || bc32::tag_of(ext2) != bc32::TAG_EXT {
        return None;
    }
    let a = bc32::combine_reg(((ext2 >> 16) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
    let b = bc32::combine_reg(((ext2 >> 8) & 0xFF) as u16, (word & 0xFF) as u16);
    let ofs = (((((ext >> 8) & 0xFF) as u16) << 8) | ((ext & 0xFF) as u16)) as i16;
    let op = match crate::vm::IntCmpKind::from_u8(((ext >> 16) & 0xFF) as u8)? {
        crate::vm::IntCmpKind::Eq => PackedCmpOp::Eq,
        crate::vm::IntCmpKind::Ne => PackedCmpOp::Ne,
        crate::vm::IntCmpKind::Lt => PackedCmpOp::Lt,
        crate::vm::IntCmpKind::Le => PackedCmpOp::Le,
        crate::vm::IntCmpKind::Gt => PackedCmpOp::Gt,
        crate::vm::IntCmpKind::Ge => PackedCmpOp::Ge,
    };
    Some((op, a, b, ofs, pc + 3))
}

pub(super) fn decode_following_cmp_int_jmp_move(
    code32: &[u32],
    cmp_pc: usize,
    expected_arith_dst: u16,
) -> Option<(PackedCmpOp, u16, u16, u16, u16, usize)> {
    let (cmp_op, cmp_a, cmp_b, ofs, move_pc) = decode_following_cmp_int_jmp(code32, cmp_pc)?;
    if cmp_a != expected_arith_dst && cmp_b != expected_arith_dst {
        return None;
    }
    let (move_dst, move_src, move_next_pc) = decode_following_move(code32, move_pc)?;
    if move_src != expected_arith_dst {
        return None;
    }
    let target_pc = ((cmp_pc as isize) + (ofs as isize)) as usize;
    if target_pc != move_next_pc {
        return None;
    }
    Some((cmp_op, cmp_a, cmp_b, move_dst, move_src, move_next_pc))
}

pub(super) fn decode_following_int_arith(
    decoded: Option<&Bc32Decoded>,
    code32: &[u32],
    pc: usize,
    expected_operand: u16,
) -> Option<(PackedArithOp, u16, u16, u16, usize)> {
    let (op, next_pc) = fetch_packed_op(decoded, code32, pc).ok()?;
    let (arith_op, dst, a, b) = match op {
        Op::AddInt(dst, a, b) => (PackedArithOp::Add, dst, a, b),
        Op::SubInt(dst, a, b) => (PackedArithOp::Sub, dst, a, b),
        Op::MulInt(dst, a, b) => (PackedArithOp::Mul, dst, a, b),
        Op::ModInt(dst, a, b) => (PackedArithOp::Mod, dst, a, b),
        _ => return None,
    };
    if a == expected_operand || b == expected_operand {
        Some((arith_op, dst, a, b, next_pc))
    } else {
        None
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn decode_following_mul_int_add_int(
    decoded: Option<&Bc32Decoded>,
    code32: &[u32],
    pc: usize,
    expected_src: u16,
) -> Option<(u16, u16, u16, u16, u16, u16, usize)> {
    let (mul_op, add_pc) = fetch_packed_op(decoded, code32, pc).ok()?;
    let (mul_dst, mul_a, mul_b) = match mul_op {
        Op::Mul(dst, a, b) | Op::MulInt(dst, a, b) => (dst, a, b),
        _ => return None,
    };
    if mul_a != expected_src && mul_b != expected_src {
        return None;
    }

    let (add_op, next_pc) = fetch_packed_op(decoded, code32, add_pc).ok()?;
    let Op::AddInt(add_dst, add_a, add_b) = add_op else {
        return None;
    };
    if add_a != mul_dst && add_b != mul_dst {
        return None;
    }

    Some((mul_dst, mul_a, mul_b, add_dst, add_a, add_b, next_pc))
}

#[allow(clippy::type_complexity)]
pub(super) fn decode_following_mul_int_mul_int_add_int(
    decoded: Option<&Bc32Decoded>,
    code32: &[u32],
    pc: usize,
    first_dst: u16,
) -> Option<(u16, u16, u16, u16, u16, u16, usize)> {
    let (second_mul_op, add_pc) = fetch_packed_op(decoded, code32, pc).ok()?;
    let Op::MulInt(second_dst, second_a, second_b) = second_mul_op else {
        return None;
    };
    let (add_op, next_pc) = fetch_packed_op(decoded, code32, add_pc).ok()?;
    let Op::AddInt(add_dst, add_a, add_b) = add_op else {
        return None;
    };
    let consumes_both_mul_results =
        (add_a == first_dst && add_b == second_dst) || (add_a == second_dst && add_b == first_dst);
    if !consumes_both_mul_results {
        return None;
    }
    Some((second_dst, second_a, second_b, add_dst, add_a, add_b, next_pc))
}

pub(super) fn decode_following_add_int_consuming(
    decoded: Option<&Bc32Decoded>,
    code32: &[u32],
    pc: usize,
    expected_operand: u16,
) -> Option<(u16, u16, u16, usize)> {
    let (op, next_pc) = fetch_packed_op(decoded, code32, pc).ok()?;
    let Op::AddInt(dst, a, b) = op else {
        return None;
    };
    if a == expected_operand || b == expected_operand {
        Some((dst, a, b, next_pc))
    } else {
        None
    }
}

pub(super) fn decode_following_floor_div_imm(
    code32: &[u32],
    pc: usize,
    expected_src: u16,
) -> Option<(u16, i16, usize)> {
    let word = *code32.get(pc)?;
    if bc32::tag_of(word) != bc32::TAG_EXT {
        return None;
    }
    let ext_op = ((word >> 16) & 0xFF) as u8;
    if ext_op != bc32::EXT_OP_FLOOR_DIV_IMM {
        return None;
    }
    let ext = *code32.get(pc + 1)?;
    if bc32::tag_of(ext) != bc32::TAG_EXT {
        return None;
    }
    let reg_ext = code32
        .get(pc + 2)
        .copied()
        .filter(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
    let dst = bc32::combine_reg(((ext >> 8) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
    let src = bc32::combine_reg((ext & 0xFF) as u16, (word & 0xFF) as u16);
    if src != expected_src {
        return None;
    }
    let imm = ((ext >> 16) & 0xFF) as u8 as i8 as i16;
    let next_pc = if reg_ext.is_some() { pc + 3 } else { pc + 2 };
    Some((dst, imm, next_pc))
}

pub(super) fn decode_following_add_int_imm(code32: &[u32], pc: usize) -> Option<(u16, u16, i16, usize)> {
    let word = *code32.get(pc)?;
    let bc32::DecodedTag::Regular {
        tag: Tag::AddIntImm,
        flags: 0,
    } = bc32::decode_tag_byte(bc32::tag_of(word))
    else {
        return None;
    };
    let reg_ext = code32
        .get(pc + 1)
        .copied()
        .filter(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
    let (dst, src, imm) = decode_ab_imm(word, reg_ext);
    let next_pc = if reg_ext.is_some() { pc + 2 } else { pc + 1 };
    Some((dst, src, imm, next_pc))
}

#[allow(clippy::type_complexity)]
pub(super) fn decode_following_sub_access_sub(
    decoded: Option<&Bc32Decoded>,
    code32: &[u32],
    pc: usize,
) -> Option<(u16, u16, u16, usize, u16, u16, u16, u16, u16, u16, usize)> {
    let (first_op, access_pc) = fetch_packed_op(decoded, code32, pc).ok()?;
    let Op::SubInt(first_dst, first_a, first_b) = first_op else {
        return None;
    };

    let (access_op, final_pc) = fetch_packed_op(decoded, code32, access_pc).ok()?;
    let Op::Access(access_dst, access_base, access_field) = access_op else {
        return None;
    };
    if access_field != first_dst {
        return None;
    }

    let (final_op, next_pc) = fetch_packed_op(decoded, code32, final_pc).ok()?;
    let Op::SubInt(final_dst, final_a, final_b) = final_op else {
        return None;
    };
    if final_a != access_dst && final_b != access_dst {
        return None;
    }

    Some((
        first_dst,
        first_a,
        first_b,
        access_pc,
        access_dst,
        access_base,
        access_field,
        final_dst,
        final_a,
        final_b,
        next_pc,
    ))
}

pub(super) fn decode_cmp_int_jmp_hot_slot(
    decoded: Option<&Bc32Decoded>,
    code32: &[u32],
    pc: usize,
    word: u32,
    op: PackedCmpOp,
    a: u16,
    b: u16,
    ofs: i16,
    next_pc: usize,
) -> PackedHotSlot {
    if let Some((dst, src, move_next_pc)) = decode_following_move(code32, next_pc) {
        let target_pc = ((pc as isize) + (ofs as isize)) as usize;
        if target_pc == move_next_pc {
            return PackedHotSlot {
                word,
                next_pc: move_next_pc,
                kind: PackedHotKind::CmpIntMove {
                    op,
                    a,
                    b,
                    dst,
                    src,
                    ofs,
                },
            };
        }
    }
    if let Some((dst, src, imm, add_next_pc)) = decode_following_add_int_imm(code32, next_pc) {
        return PackedHotSlot {
            word,
            next_pc: add_next_pc,
            kind: PackedHotKind::CmpIntAddIntImm {
                op,
                a,
                b,
                dst,
                src,
                imm,
                ofs,
            },
        };
    }
    if let Some((
        first_dst,
        first_a,
        first_b,
        access_pc,
        access_dst,
        access_base,
        access_field,
        final_dst,
        final_a,
        final_b,
        fused_next_pc,
    )) = decode_following_sub_access_sub(decoded, code32, next_pc)
    {
        let target_pc = ((pc as isize) + (ofs as isize)) as usize;
        if target_pc == fused_next_pc {
            return PackedHotSlot {
                word,
                next_pc: fused_next_pc,
                kind: PackedHotKind::CmpIntSubAccessSub {
                    op,
                    a,
                    b,
                    first_dst,
                    first_a,
                    first_b,
                    access_pc,
                    access_dst,
                    access_base,
                    access_field,
                    final_dst,
                    final_a,
                    final_b,
                    ofs,
                },
            };
        }
    }
    PackedHotSlot {
        word,
        next_pc,
        kind: PackedHotKind::CmpIntJmp { op, a, b, ofs },
    }
}
