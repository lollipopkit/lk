use super::*;

mod fusions;
use fusions::*;

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

type MoveCallDecode = (Vec<(u16, u16)>, u16, u16, u8, u8, PackedHotCallKind, usize);

fn decode_move_call(decoded: Option<&Bc32Decoded>, pc: usize) -> Option<MoveCallDecode> {
    let decoded = decoded?;
    let mut instr_idx = decoded.word_to_instr.get(pc).copied()? as usize;
    if instr_idx >= decoded.instrs.len() {
        return None;
    }
    let mut moves = Vec::new();
    while let Some(instr) = decoded.instrs.get(instr_idx) {
        match instr.op {
            Op::Move(dst, src) => {
                moves.push((dst, src));
                instr_idx += 1;
            }
            Op::Call { f, base, argc, retc } => {
                if moves_target_call_window(&moves, base, argc) {
                    return Some((moves, f, base, argc, retc, PackedHotCallKind::Generic, instr.next_pc));
                }
                return None;
            }
            Op::CallClosureExact { f, base, argc, retc } => {
                if moves_target_call_window(&moves, base, argc) {
                    return Some((
                        moves,
                        f,
                        base,
                        argc,
                        retc,
                        PackedHotCallKind::ClosureExact,
                        instr.next_pc,
                    ));
                }
                return None;
            }
            Op::CallExact { f, base, argc, retc } => {
                if moves_target_call_window(&moves, base, argc) {
                    return Some((moves, f, base, argc, retc, PackedHotCallKind::Exact, instr.next_pc));
                }
                return None;
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

struct PackedMapUpsertAdd {
    cmp_dst: u16,
    default: PackedValueOperand,
    default_load: Option<(u16, u16)>,
    add_dst: u16,
    add_rhs: PackedAddOperand,
    next_pc: usize,
}

fn instr_pc(decoded: &Bc32Decoded, instr_idx: usize) -> Option<usize> {
    decoded.word_to_instr.iter().position(|idx| *idx == instr_idx as u32)
}

fn decoded_branch_target(decoded: &Bc32Decoded, instr_idx: usize, ofs: i16) -> Option<usize> {
    let pc = instr_pc(decoded, instr_idx)?;
    let target = pc as isize + ofs as isize;
    (target >= 0).then_some(target as usize)
}

fn nil_cmp_dst(op: &Op, consts: &[Val], value_dst: u16) -> Option<u16> {
    let Op::CmpEq(dst, a, b) = *op else {
        return None;
    };
    let a_is_value = a == value_dst;
    let b_is_value = b == value_dst;
    let a_is_nil = rk_is_const(a) && matches!(consts.get(rk_index(a) as usize), Some(Val::Nil));
    let b_is_nil = rk_is_const(b) && matches!(consts.get(rk_index(b) as usize), Some(Val::Nil));
    if a_is_value && b_is_nil {
        return Some(dst);
    }
    if b_is_value && a_is_nil {
        return Some(dst);
    }
    None
}

fn same_map_set(op: &Op, expected_map: u16, expected_key: u16, interned_key: bool) -> Option<u16> {
    match *op {
        Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val }
            if !interned_key && map == expected_map && key == expected_key =>
        {
            Some(val)
        }
        Op::MapSetInterned(map, key, val) | Op::MapSetInternedMove(map, key, val)
            if interned_key && map == expected_map && key == expected_key =>
        {
            Some(val)
        }
        _ => None,
    }
}

fn add_from_map_value(op: &Op, expected_value: u16) -> Option<(u16, PackedAddOperand)> {
    match *op {
        Op::AddIntImm(dst, src, imm) if src == expected_value => Some((dst, PackedAddOperand::Imm(imm))),
        Op::Add(dst, a, b) | Op::AddInt(dst, a, b) if a == expected_value => Some((dst, PackedAddOperand::Reg(b))),
        Op::Add(dst, a, b) | Op::AddInt(dst, a, b) if b == expected_value => Some((dst, PackedAddOperand::Reg(a))),
        _ => None,
    }
}

fn decode_map_get_upsert_add(
    decoded: Option<&Bc32Decoded>,
    consts: &[Val],
    pc: usize,
    get_dst: u16,
    map: u16,
    key: u16,
    interned_key: bool,
) -> Option<PackedMapUpsertAdd> {
    let decoded = decoded?;
    let get_idx = decoded.word_to_instr.get(pc).copied()? as usize;
    let cmp_idx = get_idx + 1;
    let branch_idx = get_idx + 2;
    let cmp_dst = nil_cmp_dst(&decoded.instrs.get(cmp_idx)?.op, consts, get_dst)?;
    let branch = &decoded.instrs.get(branch_idx)?.op;
    let (branch_reg, branch_ofs) = match *branch {
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => (r, ofs),
        _ => return None,
    };
    if branch_reg != cmp_dst {
        return None;
    }

    let else_pc = decoded_branch_target(decoded, branch_idx, branch_ofs)?;
    let else_idx = *decoded.word_to_instr.get(else_pc)? as usize;
    if else_idx <= branch_idx + 1 {
        return None;
    }

    let mut nil_set_idx = branch_idx + 1;
    let mut default_load = None;
    let default = match &decoded.instrs.get(nil_set_idx)?.op {
        Op::LoadK(reg, kidx) => {
            default_load = Some((*reg, *kidx));
            nil_set_idx += 1;
            PackedValueOperand::Const(*kidx)
        }
        _ => PackedValueOperand::Reg(same_map_set(
            &decoded.instrs.get(nil_set_idx)?.op,
            map,
            key,
            interned_key,
        )?),
    };
    let nil_val = same_map_set(&decoded.instrs.get(nil_set_idx)?.op, map, key, interned_key)?;
    if let PackedValueOperand::Reg(default_reg) = default
        && nil_val != default_reg
    {
        return None;
    }

    let jmp_idx = nil_set_idx + 1;
    let Op::Jmp(jmp_ofs) = decoded.instrs.get(jmp_idx)?.op else {
        return None;
    };
    let after_pc = decoded_branch_target(decoded, jmp_idx, jmp_ofs)?;
    let (add_dst, add_rhs) = add_from_map_value(&decoded.instrs.get(else_idx)?.op, get_dst)?;
    let else_set_idx = else_idx + 1;
    let else_val = same_map_set(&decoded.instrs.get(else_set_idx)?.op, map, key, interned_key)?;
    if else_val != add_dst || decoded.instrs.get(else_set_idx)?.next_pc != after_pc {
        return None;
    }

    Some(PackedMapUpsertAdd {
        cmp_dst,
        default,
        default_load,
        add_dst,
        add_rhs,
        next_pc: after_pc,
    })
}

#[inline(always)]
pub(super) fn build_hot_slot(
    code32: &[u32],
    decoded: Option<&Bc32Decoded>,
    consts: &[Val],
    pc: usize,
    word: u32,
    raw_tag: u8,
) -> Option<PackedHotSlot> {
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
        if let Some(op) = cmp_imm16_hot_op(ext_op) {
            let (hi_dst, hi_src, _) = bc32::unpack_reg_ext(reg_ext);
            let dst = bc32::combine_reg(hi_dst, ((word >> 8) & 0xFF) as u16);
            let src = bc32::combine_reg(hi_src, (word & 0xFF) as u16);
            let imm = (((((ext >> 16) & 0xFF) as u16) << 8) | (((ext >> 8) & 0xFF) as u16)) as i16;
            let next_pc = if reg_ext.is_some() { pc + 3 } else { pc + 2 };
            if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, next_pc, dst) {
                return Some(PackedHotSlot {
                    word,
                    next_pc: fused_next_pc,
                    kind: PackedHotKind::CmpImmJmp { op, src, imm, ofs },
                });
            }
            return Some(PackedHotSlot {
                word,
                next_pc,
                kind: PackedHotKind::CmpImm { op, dst, src, imm },
            });
        }
        let f = bc32::combine_reg(((ext >> 8) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
        let b = bc32::combine_reg((ext & 0xFF) as u16, (word & 0xFF) as u16);
        let c = bc32::combine_reg(
            reg_ext.map(|word| (word & 0xFF) as u16).unwrap_or(0),
            ((ext >> 16) & 0xFF) as u16,
        );
        let next_pc = if reg_ext.is_some() { pc + 3 } else { pc + 2 };
        let kind = match ext_op {
            bc32::EXT_OP_ADD_INT => {
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == f
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::IntArithAddIntImm {
                            arith_op: PackedArithOp::Add,
                            arith_dst: f,
                            arith_a: b,
                            arith_b: c,
                            add_dst,
                            add_imm,
                        },
                    });
                }
                if let Some((div_dst, imm, fused_next_pc)) = decode_following_floor_div_imm(code32, next_pc, f) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::AddIntFloorDivImm {
                            add_dst: f,
                            a: b,
                            b: c,
                            div_dst,
                            imm,
                        },
                    });
                }
                PackedHotKind::IntArith {
                    op: PackedArithOp::Add,
                    dst: f,
                    a: b,
                    b: c,
                }
            }
            bc32::EXT_OP_ADD_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Add,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_SUB_INT => {
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == f
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::IntArithAddIntImm {
                            arith_op: PackedArithOp::Sub,
                            arith_dst: f,
                            arith_a: b,
                            arith_b: c,
                            add_dst,
                            add_imm,
                        },
                    });
                }
                if let Some((cmp_op, cmp_a, cmp_b, ofs, fused_next_pc)) = decode_following_cmp_int_jmp(code32, next_pc)
                    && (cmp_a == f || cmp_b == f)
                {
                    let jump_pc = ((next_pc as isize) + (ofs as isize)) as usize;
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::IntArithCmpIntJmp {
                            arith_op: PackedArithOp::Sub,
                            arith_dst: f,
                            arith_a: b,
                            arith_b: c,
                            cmp_op,
                            cmp_a,
                            cmp_b,
                            jump_pc,
                        },
                    });
                }
                PackedHotKind::IntArith {
                    op: PackedArithOp::Sub,
                    dst: f,
                    a: b,
                    b: c,
                }
            }
            bc32::EXT_OP_SUB_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Sub,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_MUL_INT => {
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == f
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::IntArithAddIntImm {
                            arith_op: PackedArithOp::Mul,
                            arith_dst: f,
                            arith_a: b,
                            arith_b: c,
                            add_dst,
                            add_imm,
                        },
                    });
                }
                if let Some((cmp_op, cmp_a, cmp_b, ofs, fused_next_pc)) = decode_following_cmp_int_jmp(code32, next_pc)
                    && (cmp_a == f || cmp_b == f)
                {
                    let jump_pc = ((next_pc as isize) + (ofs as isize)) as usize;
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::IntArithCmpIntJmp {
                            arith_op: PackedArithOp::Mul,
                            arith_dst: f,
                            arith_a: b,
                            arith_b: c,
                            cmp_op,
                            cmp_a,
                            cmp_b,
                            jump_pc,
                        },
                    });
                }
                if let Some((div_dst, imm, fused_next_pc)) = decode_following_floor_div_imm(code32, next_pc, f) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MulIntFloorDivImm {
                            mul_dst: f,
                            a: b,
                            b: c,
                            div_dst,
                            imm,
                        },
                    });
                }
                if let Some((second_dst, second_a, second_b, add_dst, add_a, add_b, fused_next_pc)) =
                    decode_following_mul_int_mul_int_add_int(decoded, code32, next_pc, f)
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MulIntMulIntAddInt {
                            first_dst: f,
                            first_a: b,
                            first_b: c,
                            second_dst,
                            second_a,
                            second_b,
                            add_dst,
                            add_a,
                            add_b,
                        },
                    });
                }
                if let Some((add_dst, add_a, add_b, fused_next_pc)) =
                    decode_following_add_int_consuming(decoded, code32, next_pc, f)
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MulIntAddInt {
                            mul_dst: f,
                            mul_a: b,
                            mul_b: c,
                            add_dst,
                            add_a,
                            add_b,
                        },
                    });
                }
                PackedHotKind::IntArith {
                    op: PackedArithOp::Mul,
                    dst: f,
                    a: b,
                    b: c,
                }
            }
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
            bc32::EXT_OP_MOD_INT => {
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == f
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::IntArithAddIntImm {
                            arith_op: PackedArithOp::Mod,
                            arith_dst: f,
                            arith_a: b,
                            arith_b: c,
                            add_dst,
                            add_imm,
                        },
                    });
                }
                PackedHotKind::IntArith {
                    op: PackedArithOp::Mod,
                    dst: f,
                    a: b,
                    b: c,
                }
            }
            bc32::EXT_OP_MOD_FLOAT => PackedHotKind::FloatArith {
                op: PackedArithOp::Mod,
                dst: f,
                a: b,
                b: c,
            },
            bc32::EXT_OP_FLOOR => PackedHotKind::Floor { dst: f, src: b },
            bc32::EXT_OP_STARTS_WITH_K => PackedHotKind::StartsWithK { dst: f, src: b, key: c },
            bc32::EXT_OP_CONTAINS_K => PackedHotKind::ContainsK { dst: f, src: b, key: c },
            bc32::EXT_OP_TO_ITER => PackedHotKind::ToIter { dst: f, src: b },
            bc32::EXT_OP_MAP_SET_INTERNED => PackedHotKind::MapSetInterned { map: f, key: b, val: c },
            bc32::EXT_OP_CALL_NATIVE_FAST => PackedHotKind::CallNativeFast {
                f,
                base: b,
                argc: c as u8,
                retc: 1,
            },
            bc32::EXT_OP_CALL_CLOSURE_EXACT => PackedHotKind::CallClosureExact {
                f,
                base: b,
                argc: c as u8,
                retc: 1,
            },
            bc32::EXT_OP_CALL_EXACT => PackedHotKind::CallExact {
                f,
                base: b,
                argc: c as u8,
                retc: 1,
            },
            bc32::EXT_OP_CALL_METHOD0 => PackedHotKind::CallMethod0 {
                dst: f,
                receiver: b,
                method: c,
            },
            bc32::EXT_OP_CALL_GLOBAL_METHOD0 => PackedHotKind::CallGlobalMethod0 {
                dst: f,
                receiver: b,
                method: c,
            },
            bc32::EXT_OP_LIST_LEN => PackedHotKind::ListLen { dst: f, src: b },
            bc32::EXT_OP_MAP_LEN => PackedHotKind::MapLen { dst: f, src: b },
            bc32::EXT_OP_STR_LEN => PackedHotKind::StrLen { dst: f, src: b },
            bc32::EXT_OP_FLOOR_DIV_IMM => PackedHotKind::FloorDivImm {
                dst: f,
                src: b,
                imm: c as u8 as i8 as i16,
            },
            bc32::EXT_OP_MAP_GET_INTERNED => {
                if let Some(fused) = decode_map_get_upsert_add(decoded, consts, pc, f, b, c, true) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused.next_pc,
                        kind: PackedHotKind::MapGetInternedUpsertAdd {
                            get_dst: f,
                            cmp_dst: fused.cmp_dst,
                            map: b,
                            key: c,
                            default: fused.default,
                            default_load: fused.default_load,
                            add_dst: fused.add_dst,
                            add_rhs: fused.add_rhs,
                        },
                    });
                }
                if let Some((op, rhs, jump_pc, fused_next_pc)) = decode_map_get_cmp_jmp(code32, next_pc, f) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MapGetInternedCmpJmp {
                            dst: f,
                            map: b,
                            key: c,
                            op,
                            rhs,
                            jump_pc,
                        },
                    });
                }
                PackedHotKind::MapGetInterned { dst: f, map: b, key: c }
            }
            bc32::EXT_OP_MAP_SET_INTERNED_MOVE => PackedHotKind::MapSetInternedMove { map: f, key: b, val: c },
            bc32::EXT_OP_MAP_GET_DYNAMIC => {
                if let Some(fused) = decode_map_get_upsert_add(decoded, consts, pc, f, b, c, false) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused.next_pc,
                        kind: PackedHotKind::MapGetDynamicUpsertAdd {
                            get_dst: f,
                            cmp_dst: fused.cmp_dst,
                            map: b,
                            key: c,
                            default: fused.default,
                            default_load: fused.default_load,
                            add_dst: fused.add_dst,
                            add_rhs: fused.add_rhs,
                        },
                    });
                }
                if let Some((op, rhs, jump_pc, fused_next_pc)) = decode_map_get_cmp_jmp(code32, next_pc, f) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MapGetDynamicCmpJmp {
                            dst: f,
                            map: b,
                            key: c,
                            op,
                            rhs,
                            jump_pc,
                        },
                    });
                }
                PackedHotKind::MapGetDynamic { dst: f, map: b, key: c }
            }
            bc32::EXT_OP_MAP_HAS => {
                if let Some((inc_r, inc_imm, true_pc, false_pc, fused_next_pc)) =
                    decode_map_has_inc_jmp(code32, next_pc, f)
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MapHasIncJmp {
                            dst: f,
                            map: b,
                            key: c,
                            inc_r,
                            inc_imm,
                            true_pc,
                            false_pc,
                        },
                    });
                }
                PackedHotKind::MapHas { dst: f, map: b, key: c }
            }
            bc32::EXT_OP_MAP_HAS_K => {
                if let Some((inc_r, inc_imm, true_pc, false_pc, fused_next_pc)) =
                    decode_map_has_inc_jmp(code32, next_pc, f)
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::MapHasKIncJmp {
                            dst: f,
                            map: b,
                            key: c,
                            inc_r,
                            inc_imm,
                            true_pc,
                            false_pc,
                        },
                    });
                }
                PackedHotKind::MapHasK { dst: f, map: b, key: c }
            }
            bc32::EXT_OP_STR_CONCAT_KNOWN_CAP => PackedHotKind::StrConcatKnownCap { dst: f, a: b, b: c },
            bc32::EXT_OP_STR_CONCAT_TO_STR => PackedHotKind::StrConcatToStr { dst: f, lhs: b, src: c },
            bc32::EXT_OP_CMP_I => {
                let ext2 = *code32.get(pc + 2)?;
                if bc32::tag_of(ext2) != bc32::TAG_EXT {
                    return None;
                }
                let dst = bc32::combine_reg(((ext2 >> 16) & 0xFF) as u16, ((word >> 8) & 0xFF) as u16);
                let a = bc32::combine_reg(((ext2 >> 8) & 0xFF) as u16, (word & 0xFF) as u16);
                let b = bc32::combine_reg((ext2 & 0xFF) as u16, (ext & 0xFF) as u16);
                let op = match crate::vm::IntCmpKind::from_u8(((ext >> 16) & 0xFF) as u8)? {
                    crate::vm::IntCmpKind::Eq => PackedCmpOp::Eq,
                    crate::vm::IntCmpKind::Ne => PackedCmpOp::Ne,
                    crate::vm::IntCmpKind::Lt => PackedCmpOp::Lt,
                    crate::vm::IntCmpKind::Le => PackedCmpOp::Le,
                    crate::vm::IntCmpKind::Gt => PackedCmpOp::Gt,
                    crate::vm::IntCmpKind::Ge => PackedCmpOp::Ge,
                };
                if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, pc, pc + 3, dst) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::CmpIntJmp { op, a, b, ofs },
                    });
                }
                return Some(PackedHotSlot {
                    word,
                    next_pc: pc + 3,
                    kind: PackedHotKind::CmpInt { op, dst, a, b },
                });
            }
            bc32::EXT_OP_CMP_I_JMP => {
                let ext2 = *code32.get(pc + 2)?;
                if bc32::tag_of(ext2) != bc32::TAG_EXT {
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
                let next_pc = pc + 3;
                return Some(decode_cmp_int_jmp_hot_slot(
                    decoded, code32, pc, word, op, a, b, ofs, next_pc,
                ));
            }
            bc32::EXT_OP_CMP_EQ_IMM_JMP
            | bc32::EXT_OP_CMP_NE_IMM_JMP
            | bc32::EXT_OP_CMP_GT_IMM_JMP
            | bc32::EXT_OP_CMP_GE_IMM_JMP => {
                let reg_ext = code32
                    .get(pc + 2)
                    .copied()
                    .filter(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
                let (hi_r, _, _) = bc32::unpack_reg_ext(reg_ext);
                let src = bc32::combine_reg(hi_r, ((word >> 8) & 0xFF) as u16);
                let imm = (word & 0xFF) as u8 as i8 as i16;
                let ofs = (((((ext >> 8) & 0xFF) as u16) << 8) | ((ext & 0xFF) as u16)) as i16;
                let op = match ext_op {
                    bc32::EXT_OP_CMP_EQ_IMM_JMP => PackedCmpImmOp::Eq,
                    bc32::EXT_OP_CMP_NE_IMM_JMP => PackedCmpImmOp::Ne,
                    bc32::EXT_OP_CMP_GT_IMM_JMP => PackedCmpImmOp::Gt,
                    bc32::EXT_OP_CMP_GE_IMM_JMP => PackedCmpImmOp::Ge,
                    _ => unreachable!("guarded by match arm"),
                };
                let next_pc = if reg_ext.is_some() { pc + 3 } else { pc + 2 };
                if let Some((mul_dst, mul_a, mul_b, add_dst, add_a, add_b, fused_next_pc)) =
                    decode_following_mul_int_add_int(decoded, code32, next_pc, src)
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::CmpImmMulIntAddInt {
                            op,
                            src,
                            imm,
                            mul_dst,
                            mul_a,
                            mul_b,
                            add_dst,
                            add_a,
                            add_b,
                            ofs,
                        },
                    });
                }
                return Some(PackedHotSlot {
                    word,
                    next_pc,
                    kind: PackedHotKind::CmpImmJmp { op, src, imm, ofs },
                });
            }
            bc32::EXT_OP_CMP_EQ_IMM16_JMP
            | bc32::EXT_OP_CMP_NE_IMM16_JMP
            | bc32::EXT_OP_CMP_GT_IMM16_JMP
            | bc32::EXT_OP_CMP_GE_IMM16_JMP => {
                let ofs_ext = *code32.get(pc + 2)?;
                if bc32::tag_of(ofs_ext) != bc32::TAG_EXT {
                    return None;
                }
                let reg_ext = code32
                    .get(pc + 3)
                    .copied()
                    .filter(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT);
                let (hi_r, _, _) = bc32::unpack_reg_ext(reg_ext);
                let src = bc32::combine_reg(hi_r, ((word >> 8) & 0xFF) as u16);
                let imm = (((((ext >> 16) & 0xFF) as u16) << 8) | (((ext >> 8) & 0xFF) as u16)) as i16;
                let ofs = (((((ofs_ext >> 8) & 0xFF) as u16) << 8) | ((ofs_ext & 0xFF) as u16)) as i16;
                let op = match ext_op {
                    bc32::EXT_OP_CMP_EQ_IMM16_JMP => PackedCmpImmOp::Eq,
                    bc32::EXT_OP_CMP_NE_IMM16_JMP => PackedCmpImmOp::Ne,
                    bc32::EXT_OP_CMP_GT_IMM16_JMP => PackedCmpImmOp::Gt,
                    bc32::EXT_OP_CMP_GE_IMM16_JMP => PackedCmpImmOp::Ge,
                    _ => unreachable!("guarded by match arm"),
                };
                let next_pc = if reg_ext.is_some() { pc + 4 } else { pc + 3 };
                if let Some((mul_dst, mul_a, mul_b, add_dst, add_a, add_b, fused_next_pc)) =
                    decode_following_mul_int_add_int(decoded, code32, next_pc, src)
                {
                    return Some(PackedHotSlot {
                        word,
                        next_pc: fused_next_pc,
                        kind: PackedHotKind::CmpImmMulIntAddInt {
                            op,
                            src,
                            imm,
                            mul_dst,
                            mul_a,
                            mul_b,
                            add_dst,
                            add_a,
                            add_b,
                            ofs,
                        },
                    });
                }
                return Some(PackedHotSlot {
                    word,
                    next_pc,
                    kind: PackedHotKind::CmpImmJmp { op, src, imm, ofs },
                });
            }
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
                if let Some((moves, f, base, argc, retc, call_kind, next_pc)) = decode_move_call(decoded, pc) {
                    return Some(PackedHotSlot {
                        word,
                        next_pc,
                        kind: PackedHotKind::MoveCall {
                            moves,
                            f,
                            base,
                            argc,
                            retc,
                            call_kind,
                        },
                    });
                }
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
            Tag::LoadCapture => {
                let (dst, idx, _) = decode_abc(word, reg_ext);
                PackedHotKind::LoadCapture { dst, idx }
            }
            Tag::Access => {
                let (dst, base, field) = decode_abc(word, reg_ext);
                if let Some((arith_op, arith_dst, arith_a, arith_b, fused_next_pc)) =
                    decode_following_int_arith(decoded, code32, next_pc, dst)
                {
                    next_pc = fused_next_pc;
                    PackedHotKind::AccessIntArith {
                        access_dst: dst,
                        base,
                        field,
                        arith_op,
                        arith_dst,
                        arith_a,
                        arith_b,
                    }
                } else {
                    PackedHotKind::Access { dst, base, field }
                }
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
            Tag::ToBool => {
                let (dst, src, _) = decode_abc(word, reg_ext);
                PackedHotKind::ToBool { dst, src }
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
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == dst
                {
                    next_pc = fused_next_pc;
                    PackedHotKind::ArithAddIntImm {
                        op: PackedArithOp::Add,
                        arith_dst: dst,
                        a,
                        b,
                        add_dst,
                        add_imm,
                    }
                } else {
                    PackedHotKind::Arith {
                        op: PackedArithOp::Add,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Sub => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == dst
                {
                    next_pc = fused_next_pc;
                    PackedHotKind::ArithAddIntImm {
                        op: PackedArithOp::Sub,
                        arith_dst: dst,
                        a,
                        b,
                        add_dst,
                        add_imm,
                    }
                } else if let Some((cmp_op, cmp_a, cmp_b, ofs, fused_next_pc)) =
                    decode_following_cmp_int_jmp(code32, next_pc)
                    && (cmp_a == dst || cmp_b == dst)
                {
                    let jump_pc = ((next_pc as isize) + (ofs as isize)) as usize;
                    next_pc = fused_next_pc;
                    PackedHotKind::IntArithCmpIntJmp {
                        arith_op: PackedArithOp::Sub,
                        arith_dst: dst,
                        arith_a: a,
                        arith_b: b,
                        cmp_op,
                        cmp_a,
                        cmp_b,
                        jump_pc,
                    }
                } else {
                    PackedHotKind::Arith {
                        op: PackedArithOp::Sub,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::Mul => {
                let (dst, a, b) = decode_rk_pair(word, reg_ext, flags);
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == dst
                {
                    next_pc = fused_next_pc;
                    PackedHotKind::ArithAddIntImm {
                        op: PackedArithOp::Mul,
                        arith_dst: dst,
                        a,
                        b,
                        add_dst,
                        add_imm,
                    }
                } else if let Some((cmp_op, cmp_a, cmp_b, ofs, fused_next_pc)) =
                    decode_following_cmp_int_jmp(code32, next_pc)
                    && (cmp_a == dst || cmp_b == dst)
                {
                    let jump_pc = ((next_pc as isize) + (ofs as isize)) as usize;
                    next_pc = fused_next_pc;
                    PackedHotKind::IntArithCmpIntJmp {
                        arith_op: PackedArithOp::Mul,
                        arith_dst: dst,
                        arith_a: a,
                        arith_b: b,
                        cmp_op,
                        cmp_a,
                        cmp_b,
                        jump_pc,
                    }
                } else {
                    PackedHotKind::Arith {
                        op: PackedArithOp::Mul,
                        dst,
                        a,
                        b,
                    }
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
                if let Some((add_dst, add_src, add_imm, fused_next_pc)) = decode_following_add_int_imm(code32, next_pc)
                    && add_src == dst
                {
                    next_pc = fused_next_pc;
                    PackedHotKind::ArithAddIntImm {
                        op: PackedArithOp::Mod,
                        arith_dst: dst,
                        a,
                        b,
                        add_dst,
                        add_imm,
                    }
                } else {
                    PackedHotKind::Arith {
                        op: PackedArithOp::Mod,
                        dst,
                        a,
                        b,
                    }
                }
            }
            Tag::AddIntImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                PackedHotKind::AddIntImm { dst, src, imm }
            }
            Tag::CmpEqImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                decode_cmp_imm_hot(code32, pc, next_pc, PackedCmpImmOp::Eq, dst, src, imm, &mut next_pc)
            }
            Tag::CmpNeImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                decode_cmp_imm_hot(code32, pc, next_pc, PackedCmpImmOp::Ne, dst, src, imm, &mut next_pc)
            }
            Tag::CmpLtImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                decode_cmp_imm_hot(code32, pc, next_pc, PackedCmpImmOp::Lt, dst, src, imm, &mut next_pc)
            }
            Tag::CmpLeImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                decode_cmp_imm_hot(code32, pc, next_pc, PackedCmpImmOp::Le, dst, src, imm, &mut next_pc)
            }
            Tag::CmpGtImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                decode_cmp_imm_hot(code32, pc, next_pc, PackedCmpImmOp::Gt, dst, src, imm, &mut next_pc)
            }
            Tag::CmpGeImm => {
                let (dst, src, imm) = decode_ab_imm(word, reg_ext);
                decode_cmp_imm_hot(code32, pc, next_pc, PackedCmpImmOp::Ge, dst, src, imm, &mut next_pc)
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
            Tag::JmpFalseSet => {
                let (r, dst, lo) = decode_abc(word, reg_ext);
                PackedHotKind::JmpFalseSet {
                    r,
                    dst,
                    ofs: lo as u8 as i8 as i16,
                }
            }
            Tag::JmpTrueSet => {
                let (r, dst, lo) = decode_abc(word, reg_ext);
                PackedHotKind::JmpTrueSet {
                    r,
                    dst,
                    ofs: lo as u8 as i8 as i16,
                }
            }
            Tag::Ret => {
                let (base, retc, _) = decode_abc(word, reg_ext);
                PackedHotKind::Ret { base, retc: retc as u8 }
            }
            Tag::ListPush => {
                let (list, val, _) = decode_abc(word, reg_ext);
                if flags & 1 != 0 {
                    PackedHotKind::ListPushMove { list, val }
                } else {
                    PackedHotKind::ListPush { list, val }
                }
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
fn decode_cmp_imm_hot(
    code32: &[u32],
    cmp_pc: usize,
    next_pc: usize,
    op: PackedCmpImmOp,
    dst: u16,
    src: u16,
    imm: i16,
    slot_next_pc: &mut usize,
) -> PackedHotKind {
    if let Some((ofs, fused_next_pc)) = decode_cmp_jmp(code32, cmp_pc, next_pc, dst) {
        *slot_next_pc = fused_next_pc;
        PackedHotKind::CmpImmJmp { op, src, imm, ofs }
    } else {
        PackedHotKind::CmpImm { op, dst, src, imm }
    }
}

#[inline(always)]
fn cmp_imm16_hot_op(ext_op: u8) -> Option<PackedCmpImmOp> {
    match ext_op {
        bc32::EXT_OP_CMP_EQ_IMM16 => Some(PackedCmpImmOp::Eq),
        bc32::EXT_OP_CMP_NE_IMM16 => Some(PackedCmpImmOp::Ne),
        bc32::EXT_OP_CMP_LT_IMM16 => Some(PackedCmpImmOp::Lt),
        bc32::EXT_OP_CMP_LE_IMM16 => Some(PackedCmpImmOp::Le),
        bc32::EXT_OP_CMP_GT_IMM16 => Some(PackedCmpImmOp::Gt),
        bc32::EXT_OP_CMP_GE_IMM16 => Some(PackedCmpImmOp::Ge),
        _ => None,
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
fn decode_map_has_inc_jmp(code32: &[u32], branch_pc: usize, dst: u16) -> Option<(u16, i16, usize, usize, usize)> {
    let (branch_op, add_pc) = fetch_packed_op(None, code32, branch_pc).ok()?;
    let (branch_r, branch_ofs) = match branch_op {
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => (r, ofs),
        _ => return None,
    };
    if branch_r != dst {
        return None;
    }

    let (add_op, fused_next_pc) = fetch_packed_op(None, code32, add_pc).ok()?;
    let Op::AddIntImmJmp { r, imm, ofs } = add_op else {
        return None;
    };
    let false_pc = ((branch_pc as isize) + (branch_ofs as isize)) as usize;
    let true_pc = ((add_pc as isize) + (ofs as isize)) as usize;
    Some((r, imm, true_pc, false_pc, fused_next_pc))
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
#[path = "decode_tests.rs"]
mod decode_tests;
