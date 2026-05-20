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

fn decode_following_move(code32: &[u32], pc: usize) -> Option<(u16, u16, usize)> {
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

fn decode_following_floor_div_imm(code32: &[u32], pc: usize, expected_src: u16) -> Option<(u16, i16, usize)> {
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

fn decode_following_add_int_imm(code32: &[u32], pc: usize) -> Option<(u16, u16, i16, usize)> {
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

fn decode_map_get_cmp_jmp(code32: &[u32], cmp_pc: usize, value_dst: u16) -> Option<(PackedCmpOp, u16, usize, usize)> {
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

#[inline(always)]
pub(super) fn build_hot_slot(
    code32: &[u32],
    decoded: Option<&Bc32Decoded>,
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
            bc32::EXT_OP_MAP_HAS => PackedHotKind::MapHas { dst: f, map: b, key: c },
            bc32::EXT_OP_MAP_HAS_K => PackedHotKind::MapHasK { dst: f, map: b, key: c },
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
                if let Some((dst, src, move_next_pc)) = decode_following_move(code32, next_pc) {
                    let target_pc = ((pc as isize) + (ofs as isize)) as usize;
                    if target_pc == move_next_pc {
                        return Some(PackedHotSlot {
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
                        });
                    }
                }
                if let Some((dst, src, imm, add_next_pc)) = decode_following_add_int_imm(code32, next_pc) {
                    return Some(PackedHotSlot {
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
                    });
                }
                return Some(PackedHotSlot {
                    word,
                    next_pc,
                    kind: PackedHotKind::CmpIntJmp { op, a, b, ofs },
                });
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
                return Some(PackedHotSlot {
                    word,
                    next_pc: if reg_ext.is_some() { pc + 3 } else { pc + 2 },
                    kind: PackedHotKind::CmpImmJmp {
                        op: match ext_op {
                            bc32::EXT_OP_CMP_EQ_IMM_JMP => PackedCmpImmOp::Eq,
                            bc32::EXT_OP_CMP_NE_IMM_JMP => PackedCmpImmOp::Ne,
                            bc32::EXT_OP_CMP_GT_IMM_JMP => PackedCmpImmOp::Gt,
                            bc32::EXT_OP_CMP_GE_IMM_JMP => PackedCmpImmOp::Ge,
                            _ => unreachable!("guarded by match arm"),
                        },
                        src,
                        imm,
                        ofs,
                    },
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
                return Some(PackedHotSlot {
                    word,
                    next_pc: if reg_ext.is_some() { pc + 4 } else { pc + 3 },
                    kind: PackedHotKind::CmpImmJmp {
                        op: match ext_op {
                            bc32::EXT_OP_CMP_EQ_IMM16_JMP => PackedCmpImmOp::Eq,
                            bc32::EXT_OP_CMP_NE_IMM16_JMP => PackedCmpImmOp::Ne,
                            bc32::EXT_OP_CMP_GT_IMM16_JMP => PackedCmpImmOp::Gt,
                            bc32::EXT_OP_CMP_GE_IMM16_JMP => PackedCmpImmOp::Ge,
                            _ => unreachable!("guarded by match arm"),
                        },
                        src,
                        imm,
                        ofs,
                    },
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
            n_regs: 8,
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
        let slot =
            build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, word, bc32::tag_of(word)).expect("cmp hot slot");

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
    fn packed_hot_slot_fuses_cmp_int_jmp_followed_by_move() {
        let function = function_with(vec![
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Lt,
                a: 0,
                b: 1,
                ofs: 2,
            },
            Op::Move(2, 0),
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp-int-jmp+move must be BC32 encodable");
        let word = bc.code32[0];
        let slot =
            build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, word, bc32::tag_of(word)).expect("cmp hot slot");
        let next_pc = slot.next_pc;

        match slot.kind {
            PackedHotKind::CmpIntMove {
                op: PackedCmpOp::Lt,
                a: 0,
                b: 1,
                dst: 2,
                src: 0,
                ofs,
            } => assert_eq!(ofs as usize, next_pc),
            _ => panic!("expected CmpIntMove hot slot"),
        }
        assert_eq!(next_pc, 4, "fused slot must skip compare words and following Move word");
    }

    #[test]
    fn packed_hot_slot_fuses_cmp_int_jmp_followed_by_add_int_imm() {
        let function = function_with(vec![
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Lt,
                a: 0,
                b: 1,
                ofs: 3,
            },
            Op::AddIntImm(2, 3, 1),
            Op::Jmp(2),
            Op::AddIntImm(4, 5, -1),
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp-int-jmp+add-imm must be BC32 encodable");
        let word = bc.code32[0];
        let slot =
            build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, word, bc32::tag_of(word)).expect("cmp hot slot");

        match slot.kind {
            PackedHotKind::CmpIntAddIntImm {
                op: PackedCmpOp::Lt,
                a: 0,
                b: 1,
                dst: 2,
                src: 3,
                imm: 1,
                ofs: 5,
            } => {}
            PackedHotKind::CmpIntJmp { .. } => panic!("expected CmpIntAddIntImm hot slot, got CmpIntJmp"),
            PackedHotKind::CmpIntMove { .. } => panic!("expected CmpIntAddIntImm hot slot, got CmpIntMove"),
            PackedHotKind::AddIntImm { .. } => panic!("expected CmpIntAddIntImm hot slot, got AddIntImm"),
            PackedHotKind::AddIntImmJmp { .. } => panic!("expected CmpIntAddIntImm hot slot, got AddIntImmJmp"),
            PackedHotKind::Jmp { .. } => panic!("expected CmpIntAddIntImm hot slot, got Jmp"),
            _ => panic!("expected CmpIntAddIntImm hot slot"),
        }
        assert_eq!(
            slot.next_pc,
            bc.decoded.as_ref().unwrap().instrs[1].next_pc,
            "fused slot must skip compare words and following AddIntImm word"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_moves_into_closure_exact_call_window() {
        let function = function_with(vec![
            Op::Move(2, 0),
            Op::Move(3, 1),
            Op::CallClosureExact {
                f: 4,
                base: 2,
                argc: 2,
                retc: 1,
            },
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("move+call must be BC32 encodable");
        let word = bc.code32[0];
        let slot =
            build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, word, bc32::tag_of(word)).expect("move+call hot slot");

        match slot.kind {
            PackedHotKind::MoveCall {
                moves,
                f: 4,
                base: 2,
                argc: 2,
                retc: 1,
                call_kind,
            } => {
                assert_eq!(moves, vec![(2, 0), (3, 1)]);
                assert!(matches!(call_kind, PackedHotCallKind::ClosureExact));
            }
            _ => panic!("expected MoveCall hot slot"),
        }
        assert_eq!(slot.next_pc, 4, "fused slot must skip argument moves and call words");
    }

    #[test]
    fn packed_hot_slot_decodes_map_has_k() {
        let function = Function {
            consts: vec![Val::Nil, Val::from_str("needle")],
            code: vec![Op::LoadK(0, 0), Op::MapHasK(1, 0, 1), Op::Ret { base: 1, retc: 1 }],
            n_regs: 2,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("MapHasK must be BC32 encodable");
        let pc = 1;
        let word = bc.code32[pc];
        let slot =
            build_hot_slot(&bc.code32, bc.decoded.as_deref(), pc, word, bc32::tag_of(word)).expect("MapHasK hot slot");

        assert!(matches!(slot.kind, PackedHotKind::MapHasK { dst: 1, map: 0, key: 1 }));
    }

    #[test]
    fn packed_hot_slot_fuses_map_get_compare_branch() {
        let nil_rk = crate::vm::bytecode::rk_make_const(0);
        let function = Function {
            consts: vec![Val::Nil, Val::from_str("needle")],
            code: vec![
                Op::LoadK(2, 1),
                Op::MapGetDynamic(1, 0, 2),
                Op::CmpNe(3, 1, nil_rk),
                Op::BoolBranch(3, 2),
                Op::LoadK(4, 0),
                Op::MapGetInterned(5, 0, 1),
                Op::CmpEq(6, 5, nil_rk),
                Op::BoolBranch(6, 2),
                Op::LoadK(7, 0),
                Op::Ret { base: 1, retc: 1 },
            ],
            n_regs: 8,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("map-get cmp branch must be BC32 encodable");

        let dynamic_pc = 1;
        let dynamic_word = bc.code32[dynamic_pc];
        let dynamic_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            dynamic_pc,
            dynamic_word,
            bc32::tag_of(dynamic_word),
        )
        .expect("dynamic map-get cmp hot slot");
        assert!(matches!(
            dynamic_slot.kind,
            PackedHotKind::MapGetDynamicCmpJmp {
                dst: 1,
                map: 0,
                key: 2,
                op: PackedCmpOp::Ne,
                ..
            }
        ));

        let interned_pc = dynamic_slot.next_pc + 1;
        let interned_word = bc.code32[interned_pc];
        let interned_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            interned_pc,
            interned_word,
            bc32::tag_of(interned_word),
        )
        .expect("interned map-get cmp hot slot");
        assert!(matches!(
            interned_slot.kind,
            PackedHotKind::MapGetInternedCmpJmp {
                dst: 5,
                map: 0,
                key: 1,
                op: PackedCmpOp::Eq,
                ..
            }
        ));
    }

    #[test]
    fn packed_hot_slot_fuses_add_int_feeding_floor_div_imm() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::AddInt(2, 0, 1),
                Op::FloorDivImm { dst: 3, src: 2, imm: 2 },
                Op::Ret { base: 3, retc: 1 },
            ],
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("add floor-div chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, word, bc32::tag_of(word))
            .expect("add floor-div hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::AddIntFloorDivImm {
                add_dst: 2,
                a: 0,
                b: 1,
                div_dst: 3,
                imm: 2,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_hot_slot_decodes_contains_k() {
        let function = Function {
            consts: vec![Val::from_str("needle")],
            code: vec![Op::ContainsK(1, 0, 0), Op::Ret { base: 1, retc: 1 }],
            n_regs: 2,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("ContainsK must be BC32 encodable");
        let word = bc.code32[0];
        let slot =
            build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, word, bc32::tag_of(word)).expect("ContainsK hot slot");

        assert!(matches!(slot.kind, PackedHotKind::ContainsK { dst: 1, src: 0, key: 0 }));
    }

    #[test]
    fn packed_hot_slot_decodes_capture_bool_and_set_branches() {
        let function = function_with(vec![
            Op::LoadCapture { dst: 1, idx: 0 },
            Op::ToBool(2, 1),
            Op::JmpTrueSet { r: 2, dst: 3, ofs: 1 },
            Op::JmpFalseSet { r: 2, dst: 3, ofs: 1 },
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("control hot slots must be BC32 encodable");

        let load_word = bc.code32[0];
        let load_slot = build_hot_slot(&bc.code32, bc.decoded.as_deref(), 0, load_word, bc32::tag_of(load_word))
            .expect("LoadCapture hot slot");
        assert!(matches!(load_slot.kind, PackedHotKind::LoadCapture { dst: 1, idx: 0 }));

        let bool_word = bc.code32[1];
        let bool_slot = build_hot_slot(&bc.code32, bc.decoded.as_deref(), 1, bool_word, bc32::tag_of(bool_word))
            .expect("ToBool hot slot");
        assert!(matches!(bool_slot.kind, PackedHotKind::ToBool { dst: 2, src: 1 }));

        let true_word = bc.code32[2];
        let true_slot = build_hot_slot(&bc.code32, bc.decoded.as_deref(), 2, true_word, bc32::tag_of(true_word))
            .expect("JmpTrueSet hot slot");
        assert!(matches!(
            true_slot.kind,
            PackedHotKind::JmpTrueSet { r: 2, dst: 3, ofs: 1 }
        ));

        let false_word = bc.code32[3];
        let false_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            3,
            false_word,
            bc32::tag_of(false_word),
        )
        .expect("JmpFalseSet hot slot");
        assert!(matches!(
            false_slot.kind,
            PackedHotKind::JmpFalseSet { r: 2, dst: 3, ofs: 1 }
        ));
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
        let slot = build_hot_slot(&bc.code32, bc.decoded.as_deref(), step_pc, word, bc32::tag_of(word))
            .expect("range step hot slot");

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
