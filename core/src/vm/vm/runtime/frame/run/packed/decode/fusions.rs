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

pub(super) fn regs_dead_after_pc(decoded: &Bc32Decoded, pc: usize, regs: &[u16]) -> bool {
    regs.iter().all(|reg| reg_dead_after_pc(decoded, pc, *reg))
}

fn reg_dead_after_pc(decoded: &Bc32Decoded, pc: usize, reg: u16) -> bool {
    let Some(mut idx) = decoded.word_to_instr.get(pc).copied().map(|idx| idx as usize) else {
        return false;
    };
    while let Some(instr) = decoded.instrs.get(idx) {
        if op_reads_reg(&instr.op, reg) {
            return false;
        }
        if op_writes_reg(&instr.op, reg) {
            return true;
        }
        idx += 1;
    }
    true
}

fn op_reads_reg(op: &Op, reg: u16) -> bool {
    let is = |value: u16| value == reg;
    match *op {
        Op::Move(_, src)
        | Op::Not(_, src)
        | Op::ToStr(_, src)
        | Op::ToBool(_, src)
        | Op::StoreLocal(_, src)
        | Op::DefineGlobal(_, src)
        | Op::LoadLocal(_, src)
        | Op::JmpIfNil(src, _)
        | Op::JmpIfNotNil(src, _)
        | Op::JmpFalse(src, _)
        | Op::BoolBranch(src, _)
        | Op::AddIntImm(_, src, _)
        | Op::CmpEqImm(_, src, _)
        | Op::CmpNeImm(_, src, _)
        | Op::CmpLtImm(_, src, _)
        | Op::CmpLeImm(_, src, _)
        | Op::CmpGtImm(_, src, _)
        | Op::CmpGeImm(_, src, _)
        | Op::AccessK(_, src, _)
        | Op::IndexK(_, src, _)
        | Op::ListIndexI(_, src, _)
        | Op::StrIndexI(_, src, _)
        | Op::Len { src, .. }
        | Op::ListLen { src, .. }
        | Op::MapLen { src, .. }
        | Op::StrLen { src, .. }
        | Op::Floor { src, .. }
        | Op::FloorDivImm { src, .. }
        | Op::StartsWithK(_, src, _)
        | Op::ContainsK(_, src, _)
        | Op::MapGetInterned(_, src, _)
        | Op::MapHasK(_, src, _)
        | Op::PatternMatch { src, .. }
        | Op::PatternMatchOrFail { src, .. }
        | Op::ToIter { src, .. } => is(src),
        Op::CmpLtImmJmp { r, .. }
        | Op::CmpLeImmJmp { r, .. }
        | Op::CmpEqImmJmp { r, .. }
        | Op::CmpGtImmJmp { r, .. }
        | Op::CmpGeImmJmp { r, .. }
        | Op::CmpNeImmJmp { r, .. }
        | Op::JmpNilOrFalseJmp { r, .. } => is(r),
        Op::Ret { base, retc } => reg >= base && reg < base.saturating_add(retc as u16),
        Op::Add(_, a, b)
        | Op::StrConcatKnownCap(_, a, b)
        | Op::StrConcatToStr(_, a, b)
        | Op::Sub(_, a, b)
        | Op::Mul(_, a, b)
        | Op::Div(_, a, b)
        | Op::Mod(_, a, b)
        | Op::AddInt(_, a, b)
        | Op::AddFloat(_, a, b)
        | Op::SubInt(_, a, b)
        | Op::SubFloat(_, a, b)
        | Op::MulInt(_, a, b)
        | Op::MulFloat(_, a, b)
        | Op::DivFloat(_, a, b)
        | Op::ModInt(_, a, b)
        | Op::ModFloat(_, a, b)
        | Op::CmpEq(_, a, b)
        | Op::CmpNe(_, a, b)
        | Op::CmpLt(_, a, b)
        | Op::CmpLe(_, a, b)
        | Op::CmpGt(_, a, b)
        | Op::CmpGe(_, a, b)
        | Op::In(_, a, b)
        | Op::Access(_, a, b)
        | Op::MapHas(_, a, b)
        | Op::MapGetDynamic(_, a, b)
        | Op::Index { base: a, idx: b, .. }
        | Op::ListSlice { src: a, start: b, .. } => is(a) || is(b),
        Op::CmpI { a, b, .. } | Op::CmpIntJmp { a, b, .. } => is(a) || is(b),
        Op::NullishPick { l, .. } | Op::JmpFalseSet { r: l, .. } | Op::JmpTrueSet { r: l, .. } => is(l),
        Op::AddRangeCountImm { idx, limit, step, .. } => is(idx) || is(limit) || is(step),
        Op::ListSetI { list, val, .. } => is(list) || is(val),
        Op::BuildList { base, len, .. } | Op::BuildMap { base, len, .. } => {
            let end = base.saturating_add(len.saturating_mul(2));
            reg >= base && reg < end
        }
        Op::ListPush { list, val }
        | Op::ListPushMove { list, val }
        | Op::MapSetInterned(list, _, val)
        | Op::MapSetInternedMove(list, _, val) => is(list) || is(val),
        Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val } => is(map) || is(key) || is(val),
        Op::ListFoldAdd { acc, list } => is(acc) || is(list),
        Op::MapValuesFoldAdd { acc, map } => is(acc) || is(map),
        Op::Call { f, base, argc, .. }
        | Op::CallExact { f, base, argc, .. }
        | Op::CallClosureExact { f, base, argc, .. }
        | Op::CallNativeFast { f, base, argc, .. } => is(f) || (reg >= base && reg < base.saturating_add(argc as u16)),
        Op::CallMethod0 { receiver, .. } => is(receiver),
        Op::CallGlobalMethod0 { .. } => false,
        Op::CallNamed {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            ..
        }
        | Op::CallNamedFallback {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            ..
        } => {
            is(f)
                || (reg >= base_pos && reg < base_pos.saturating_add(posc as u16))
                || (reg >= base_named && reg < base_named.saturating_add((namedc as u16).saturating_mul(2)))
        }
        _ => false,
    }
}

fn op_writes_reg(op: &Op, reg: u16) -> bool {
    let is = |value: u16| value == reg;
    match *op {
        Op::LoadK(dst, _)
        | Op::Move(dst, _)
        | Op::Not(dst, _)
        | Op::ToStr(dst, _)
        | Op::ToBool(dst, _)
        | Op::Add(dst, _, _)
        | Op::StrConcatKnownCap(dst, _, _)
        | Op::StrConcatToStr(dst, _, _)
        | Op::Sub(dst, _, _)
        | Op::Mul(dst, _, _)
        | Op::Div(dst, _, _)
        | Op::Mod(dst, _, _)
        | Op::AddInt(dst, _, _)
        | Op::AddFloat(dst, _, _)
        | Op::AddIntImm(dst, _, _)
        | Op::SubInt(dst, _, _)
        | Op::SubFloat(dst, _, _)
        | Op::MulInt(dst, _, _)
        | Op::MulFloat(dst, _, _)
        | Op::DivFloat(dst, _, _)
        | Op::ModInt(dst, _, _)
        | Op::ModFloat(dst, _, _)
        | Op::CmpEq(dst, _, _)
        | Op::CmpNe(dst, _, _)
        | Op::CmpLt(dst, _, _)
        | Op::CmpLe(dst, _, _)
        | Op::CmpGt(dst, _, _)
        | Op::CmpGe(dst, _, _)
        | Op::CmpEqImm(dst, _, _)
        | Op::CmpNeImm(dst, _, _)
        | Op::CmpLtImm(dst, _, _)
        | Op::CmpLeImm(dst, _, _)
        | Op::CmpGtImm(dst, _, _)
        | Op::CmpGeImm(dst, _, _)
        | Op::In(dst, _, _)
        | Op::LoadLocal(dst, _)
        | Op::LoadGlobal(dst, _)
        | Op::Access(dst, _, _)
        | Op::AccessK(dst, _, _)
        | Op::IndexK(dst, _, _)
        | Op::ListIndexI(dst, _, _)
        | Op::StrIndexI(dst, _, _)
        | Op::StartsWithK(dst, _, _)
        | Op::ContainsK(dst, _, _)
        | Op::MapHas(dst, _, _)
        | Op::MapGetInterned(dst, _, _)
        | Op::MapGetDynamic(dst, _, _)
        | Op::MapHasK(dst, _, _)
        | Op::MakeClosure { dst, .. }
        | Op::PatternMatch { dst, .. }
        | Op::ToIter { dst, .. }
        | Op::BuildList { dst, .. }
        | Op::BuildMap { dst, .. }
        | Op::ListSlice { dst, .. }
        | Op::NullishPick { dst, .. }
        | Op::JmpFalseSet { dst, .. }
        | Op::JmpTrueSet { dst, .. }
        | Op::Len { dst, .. }
        | Op::ListLen { dst, .. }
        | Op::MapLen { dst, .. }
        | Op::StrLen { dst, .. }
        | Op::Floor { dst, .. }
        | Op::FloorDivImm { dst, .. } => is(dst),
        Op::LoadCapture { dst, .. } | Op::CmpI { dst, .. } | Op::ListSetI { dst, .. } => is(dst),
        Op::StoreLocal(idx, _) => is(idx),
        Op::AddIntImmJmp { r, .. } => is(r),
        Op::AddRangeCountImm { target, .. } => is(target),
        Op::ListPush { list, .. }
        | Op::ListPushMove { list, .. }
        | Op::MapSetInterned(list, _, _)
        | Op::MapSetInternedMove(list, _, _)
        | Op::ListFoldAdd { acc: list, .. }
        | Op::MapValuesFoldAdd { acc: list, .. } => is(list),
        Op::MapSet { map, .. } | Op::MapSetMove { map, .. } => is(map),
        Op::Call { base, retc, .. }
        | Op::CallExact { base, retc, .. }
        | Op::CallClosureExact { base, retc, .. }
        | Op::CallNativeFast { base, retc, .. } => reg >= base && reg < base.saturating_add(retc as u16),
        Op::CallMethod0 { dst, .. } | Op::CallGlobalMethod0 { dst, .. } => is(dst),
        Op::CallNamed { base_pos, retc, .. } | Op::CallNamedFallback { base_pos, retc, .. } => {
            reg >= base_pos && reg < base_pos.saturating_add(retc as u16)
        }
        _ => false,
    }
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
