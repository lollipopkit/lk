//! Peephole optimization passes for LK bytecode.
//!
//! These post-compilation passes scan for instruction patterns that can be
//! fused into single, more efficient opcodes. Each pass performs pattern
//! matching, replaces matched pairs with fused instructions, and adjusts
//! all relative jump offsets to maintain correctness.
//!
//! ## Current Fusions
//!
//! | Pattern | Fused Result | Benefit |
//! |---------|-------------|---------|
//! | `CmpI + JmpFalse` | `CmpIntJmp` | 1 dispatch and one temp bool write saved in typed loops |
//! | `CmpLtImm + JmpFalse/BoolBranch` | `CmpLtImmJmp` | 1 dispatch/iteration savings in while loops |
//! | `CmpLeImm + JmpFalse/BoolBranch` | `CmpLeImmJmp` | Same for <= based loops |
//! | `CmpEqImm + JmpFalse/BoolBranch` | `CmpEqImmJmp` | Same for `==` based guards |
//! | `CmpGtImm + JmpFalse/BoolBranch` | `CmpGtImmJmp` | Same for `>` based guards |
//! | `CmpGeImm + JmpFalse/BoolBranch` | `CmpGeImmJmp` | Same for `>=` based guards |
//! | `CmpNeImm + JmpFalse/BoolBranch` | `CmpNeImmJmp` | Same for `while x != imm` loops |
//! | `AddIntImm + Jmp` | `AddIntImmJmp` | Loop increment tail fusion |
//! | `AddIntImm + ForRangeStep` | `AddIntImmJmp` | Range-loop accumulator tail fusion |
//! | `LoadLocal + Ret/branch` | direct `Ret/branch` from local | Avoid copy-only locals |
//! | `LoadLocal + read-only op` | read source local directly | Avoid copy-only locals |
//! | `LoadK + RK op` | use RK const operand directly | Avoid constant materialization |
//! | `MapGet + != nil + branch` | `MapHas + branch` | Avoid cloning map values for presence checks |
//!
//! These fused ops are handled natively in both opcode and BC32 packed paths.

use crate::{
    val::Val,
    vm::bytecode::{Op, RK_INDEX_MASK, rk_index, rk_is_const, rk_make_const},
};

/// Fuse common two-instruction patterns into single fused opcodes.
///
/// 1. `CmpI(dst, a, b, kind)` + `JmpFalse(dst, ofs)` → `CmpIntJmp { kind, a, b, ofs+1 }`
/// 2. `CmpLtImm(dst, src, imm)` + `JmpFalse/BoolBranch(dst, ofs)` → `CmpLtImmJmp { src, imm, ofs+1 }`
/// 3. `CmpLeImm(dst, src, imm)` + `JmpFalse/BoolBranch(dst, ofs)` → `CmpLeImmJmp { src, imm, ofs+1 }`
/// 4. `CmpEqImm(dst, src, imm)` + `JmpFalse/BoolBranch(dst, ofs)` → `CmpEqImmJmp { src, imm, ofs+1 }`
/// 5. `CmpGtImm(dst, src, imm)` + `JmpFalse/BoolBranch(dst, ofs)` → `CmpGtImmJmp { src, imm, ofs+1 }`
/// 6. `CmpGeImm(dst, src, imm)` + `JmpFalse/BoolBranch(dst, ofs)` → `CmpGeImmJmp { src, imm, ofs+1 }`
/// 7. `CmpNeImm(dst, src, imm)` + `JmpFalse/BoolBranch(dst, ofs)` → `CmpNeImmJmp { src, imm, ofs+1 }`
/// 5. `AddIntImm(dst, src, imm)` + `Jmp(ofs)` (when dst==src) → `AddIntImmJmp { dst, imm, ofs+1 }`
/// 6. `AddIntImm(dst, src, imm)` + `ForRangeStep(back)` (when dst==src) → `AddIntImmJmp { dst, imm, back+1 }`
/// 7. `LoadLocal(tmp, idx)` + single consumer read-only op using `tmp` → consumer reads `idx`
///
/// The second instruction is removed and all relative jump offsets are adjusted.
#[cfg(test)]
pub fn peephole_fuse_cmp_jmp(code: &mut Vec<Op>) {
    peephole_fuse_cmp_jmp_with_consts(code, &[]);
}

pub fn peephole_fuse_cmp_jmp_with_consts(code: &mut Vec<Op>, consts: &[Val]) {
    let mut removals: Vec<usize> = Vec::new();

    let mut i = 0;
    while i + 1 < code.len() {
        if let Some(fused) = fuse_map_get_ne_nil_with_loaded_nil(code, consts, i) {
            code[i] = fused.op;
            removals.extend_from_slice(&[i + 1, i + 2]);
            i += 4;
            continue;
        }

        if let Some(fused) = fuse_map_get_ne_nil_with_known_nil(code, consts, i) {
            code[i] = fused.op;
            removals.push(i + 1);
            i += 3;
            continue;
        }

        if let Op::LoadK(dst, kidx) = code[i]
            && kidx <= RK_INDEX_MASK
            && !has_branch_target_to(code, i + 1)
        {
            let mut next = code[i + 1].clone();
            if remap_rk_read_operand(&mut next, dst, rk_make_const(kidx))
                && reg_dead_after_single_consumer(code, i + 2, dst)
            {
                code[i + 1] = next;
                removals.push(i);
                i += 2;
                continue;
            }
        }

        if let Op::LoadLocal(dst, idx) = code[i] {
            let mut next = code[i + 1].clone();
            if !has_branch_target_to(code, i + 1)
                && remap_single_read_operand(&mut next, dst, idx)
                && reg_dead_after_single_consumer(code, i + 2, dst)
            {
                code[i + 1] = next;
                removals.push(i);
                i += 2;
                continue;
            }
        }

        if let (
            Op::LoadGlobal(receiver, receiver_name),
            Op::CallMethod0 {
                dst,
                receiver: call_receiver,
                method,
            },
        ) = (&code[i], &code[i + 1])
            && dst == receiver
            && call_receiver == receiver
        {
            code[i + 1] = Op::CallGlobalMethod0 {
                dst: *dst,
                receiver: *receiver_name,
                method: *method,
            };
            removals.push(i);
            i += 2;
            continue;
        }

        match (&code[i], &code[i + 1]) {
            (Op::CmpI { dst, a, b, kind }, Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpIntJmp {
                    kind: *kind,
                    a: *a,
                    b: *b,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpLtImm(dst, src, imm), Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(imm) && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpLtImmJmp {
                    r: *src,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpLeImm(dst, src, imm), Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(imm) && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpLeImmJmp {
                    r: *src,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpEqImm(dst, src, imm), Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpEqImmJmp {
                    r: *src,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpGtImm(dst, src, imm), Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpGtImmJmp {
                    r: *src,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpGeImm(dst, src, imm), Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpGeImmJmp {
                    r: *src,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpNeImm(dst, src, imm), Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs))
                if *dst == *r && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpNeImmJmp {
                    r: *src,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::AddIntImm(dst, src, imm), Op::Jmp(ofs))
                if dst == src && (-128..=127).contains(imm) && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::AddIntImmJmp {
                    r: *dst,
                    imm: *imm,
                    ofs: *ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            (Op::AddIntImm(dst, src, imm), Op::ForRangeStep { back_ofs, .. })
                if dst == src && (-128..=127).contains(imm) && (-128..=127).contains(back_ofs) =>
            {
                code[i] = Op::AddIntImmJmp {
                    r: *dst,
                    imm: *imm,
                    ofs: *back_ofs + 1,
                };
                removals.push(i + 1);
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !removals.is_empty() {
        // Remove fused instructions (reverse order to keep indices valid)
        for &idx in removals.iter().rev() {
            code.remove(idx);
        }

        // Fix all relative jump offsets
        fixup_offsets(code, &removals);
    }

    fuse_map_presence_after_rk_remap(code, consts);

    for op in code.iter_mut() {
        match *op {
            Op::JmpFalse(r, ofs) => *op = Op::BoolBranch(r, ofs),
            Op::ForRangeLoop {
                idx,
                limit,
                step,
                inclusive,
                write_idx,
                ofs,
            } => {
                *op = Op::RangeLoopI {
                    idx,
                    limit,
                    step,
                    inclusive,
                    write_idx,
                    ofs,
                };
            }
            _ => {}
        }
    }
}

fn fuse_map_presence_after_rk_remap(code: &mut Vec<Op>, consts: &[Val]) {
    let mut removals: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + 2 < code.len() {
        if let Some(fused) = fuse_map_get_ne_nil_with_known_nil(code, consts, i) {
            code[i] = fused.op;
            removals.push(i + 1);
            i += 3;
        } else {
            i += 1;
        }
    }

    if removals.is_empty() {
        return;
    }

    for &idx in removals.iter().rev() {
        code.remove(idx);
    }
    fixup_offsets(code, &removals);
}

struct FusedMapPresence {
    op: Op,
}

fn fuse_map_get_ne_nil_with_loaded_nil(code: &[Op], consts: &[Val], i: usize) -> Option<FusedMapPresence> {
    let load_idx = i + 1;
    let cmp_idx = i + 2;
    let branch_idx = i + 3;
    let (nil_reg, nil_kidx) = match code.get(load_idx)? {
        Op::LoadK(reg, kidx) => (*reg, *kidx),
        _ => return None,
    };
    if !matches!(consts.get(nil_kidx as usize), Some(Val::Nil)) {
        return None;
    }
    if has_branch_target_to(code, load_idx) || has_branch_target_to(code, cmp_idx) {
        return None;
    }
    let (get_dst, fused_op) = map_presence_op_from_get(&code[i])?;
    let cmp_dst = cmp_ne_reg_nil_at(code, cmp_idx, get_dst, Some(nil_reg), consts)?;
    branch_reads(&code[branch_idx], cmp_dst)?;
    if !reg_dead_after_single_consumer(code, branch_idx + 1, get_dst) {
        return None;
    }
    Some(FusedMapPresence {
        op: fused_op.with_dst(cmp_dst),
    })
}

fn fuse_map_get_ne_nil_with_known_nil(code: &[Op], consts: &[Val], i: usize) -> Option<FusedMapPresence> {
    let cmp_idx = i + 1;
    let branch_idx = i + 2;
    if has_branch_target_to(code, cmp_idx) {
        return None;
    }
    let (get_dst, fused_op) = map_presence_op_from_get(&code[i])?;
    let cmp_dst = cmp_ne_reg_nil_at(code, cmp_idx, get_dst, None, consts)?;
    branch_reads(&code[branch_idx], cmp_dst)?;
    if !reg_dead_after_single_consumer(code, branch_idx + 1, get_dst) {
        return None;
    }
    Some(FusedMapPresence {
        op: fused_op.with_dst(cmp_dst),
    })
}

enum MapPresenceTemplate {
    Dynamic { map: u16, key: u16 },
    Interned { map: u16, kidx: u16 },
}

impl MapPresenceTemplate {
    fn with_dst(self, dst: u16) -> Op {
        match self {
            Self::Dynamic { map, key } => Op::MapHas(dst, map, key),
            Self::Interned { map, kidx } => Op::MapHasK(dst, map, kidx),
        }
    }
}

fn map_presence_op_from_get(op: &Op) -> Option<(u16, MapPresenceTemplate)> {
    match *op {
        Op::MapGetDynamic(dst, map, key) if rk_is_const(key) => Some((
            dst,
            MapPresenceTemplate::Interned {
                map,
                kidx: rk_index(key),
            },
        )),
        Op::MapGetDynamic(dst, map, key) => Some((dst, MapPresenceTemplate::Dynamic { map, key })),
        Op::MapGetInterned(dst, map, kidx) => Some((dst, MapPresenceTemplate::Interned { map, kidx })),
        _ => None,
    }
}

fn cmp_ne_reg_nil_at(code: &[Op], cmp_idx: usize, value_reg: u16, nil_reg: Option<u16>, consts: &[Val]) -> Option<u16> {
    let op = code.get(cmp_idx)?;
    let Op::CmpNe(dst, a, b) = *op else {
        return None;
    };
    if a == value_reg && is_nil_operand(code, cmp_idx, b, nil_reg, consts) {
        return Some(dst);
    }
    if b == value_reg && is_nil_operand(code, cmp_idx, a, nil_reg, consts) {
        return Some(dst);
    }
    None
}

fn is_nil_operand(code: &[Op], pos: usize, operand: u16, nil_reg: Option<u16>, consts: &[Val]) -> bool {
    if nil_reg == Some(operand) {
        return true;
    }
    if rk_is_const(operand) && matches!(consts.get(rk_index(operand) as usize), Some(Val::Nil)) {
        return true;
    }
    reg_last_write_is_nil_load(code, pos, operand, consts)
}

fn reg_last_write_is_nil_load(code: &[Op], pos: usize, reg: u16, consts: &[Val]) -> bool {
    for op in code[..pos].iter().rev() {
        if op_writes_reg(op, reg) {
            return matches!(op, Op::LoadK(dst, kidx) if *dst == reg && matches!(consts.get(*kidx as usize), Some(Val::Nil)));
        }
        if op_reads_reg(op, reg) || is_control_boundary(op) {
            return false;
        }
    }
    false
}

fn is_control_boundary(op: &Op) -> bool {
    matches!(
        op,
        Op::Jmp(_)
            | Op::JmpFalse(_, _)
            | Op::BoolBranch(_, _)
            | Op::JmpIfNil(_, _)
            | Op::JmpIfNotNil(_, _)
            | Op::JmpFalseSet { .. }
            | Op::JmpTrueSet { .. }
            | Op::NullishPick { .. }
            | Op::Break(_)
            | Op::Continue(_)
            | Op::Ret { .. }
    )
}

fn branch_reads(op: &Op, reg: u16) -> Option<()> {
    match *op {
        Op::JmpFalse(r, _) | Op::BoolBranch(r, _) if r == reg => Some(()),
        _ => None,
    }
}

fn has_branch_target_to(code: &[Op], target: usize) -> bool {
    code.iter()
        .enumerate()
        .any(|(pc, op)| branch_target(pc, op) == Some(target))
}

fn branch_target(pc: usize, op: &Op) -> Option<usize> {
    let ofs = match op {
        Op::Jmp(ofs)
        | Op::JmpFalse(_, ofs)
        | Op::JmpIfNil(_, ofs)
        | Op::JmpIfNotNil(_, ofs)
        | Op::BoolBranch(_, ofs)
        | Op::Break(ofs)
        | Op::Continue(ofs)
        | Op::AddIntImmJmp { ofs, .. }
        | Op::CmpIntJmp { ofs, .. }
        | Op::CmpLtImmJmp { ofs, .. }
        | Op::CmpLeImmJmp { ofs, .. }
        | Op::CmpEqImmJmp { ofs, .. }
        | Op::CmpGtImmJmp { ofs, .. }
        | Op::CmpGeImmJmp { ofs, .. }
        | Op::CmpNeImmJmp { ofs, .. }
        | Op::RangeLoopI { ofs, .. }
        | Op::ForRangeLoop { ofs, .. } => *ofs,
        Op::JmpFalseSet { ofs, .. } | Op::JmpTrueSet { ofs, .. } | Op::NullishPick { ofs, .. } => *ofs,
        Op::ForRangeStep { back_ofs, .. } => *back_ofs,
        _ => return None,
    };
    let target = pc as isize + ofs as isize;
    (target >= 0).then_some(target as usize)
}

fn remap_rk_read_operand(op: &mut Op, from: u16, to: u16) -> bool {
    let mut changed = false;
    let mut remap = |reg: &mut u16| {
        if *reg == from {
            *reg = to;
            changed = true;
        }
    };

    match op {
        Op::Add(_, a, b)
        | Op::Sub(_, a, b)
        | Op::Mul(_, a, b)
        | Op::Div(_, a, b)
        | Op::Mod(_, a, b)
        | Op::CmpEq(_, a, b)
        | Op::CmpNe(_, a, b)
        | Op::CmpLt(_, a, b)
        | Op::CmpLe(_, a, b)
        | Op::CmpGt(_, a, b)
        | Op::CmpGe(_, a, b) => {
            remap(a);
            remap(b);
        }
        _ => {}
    }

    changed
}

fn remap_single_read_operand(op: &mut Op, from: u16, to: u16) -> bool {
    let mut changed = false;
    let mut remap = |reg: &mut u16| {
        if *reg == from {
            *reg = to;
            changed = true;
        }
    };

    match op {
        Op::Not(_, src)
        | Op::ToStr(_, src)
        | Op::ToBool(_, src)
        | Op::CmpEqImm(_, src, _)
        | Op::CmpNeImm(_, src, _)
        | Op::CmpLtImm(_, src, _)
        | Op::CmpLeImm(_, src, _)
        | Op::CmpGtImm(_, src, _)
        | Op::CmpGeImm(_, src, _)
        | Op::JmpIfNil(src, _)
        | Op::JmpIfNotNil(src, _)
        | Op::Ret { base: src, retc: 1 }
        | Op::JmpFalse(src, _)
        | Op::BoolBranch(src, _)
        | Op::PatternMatchOrFail { src, .. }
        | Op::Len { src, .. }
        | Op::ListLen { src, .. }
        | Op::MapLen { src, .. }
        | Op::StrLen { src, .. }
        | Op::Floor { src, .. }
        | Op::FloorDivImm { src, .. } => remap(src),
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
        | Op::Index { base: a, idx: b, .. }
        | Op::ListSlice { src: a, start: b, .. }
        | Op::MapHas(_, a, b)
        | Op::MapGetDynamic(_, a, b) => {
            remap(a);
            remap(b);
        }
        Op::AddIntImm(_, src, _)
        | Op::CmpLtImmJmp { r: src, .. }
        | Op::CmpLeImmJmp { r: src, .. }
        | Op::CmpEqImmJmp { r: src, .. }
        | Op::CmpGtImmJmp { r: src, .. }
        | Op::CmpGeImmJmp { r: src, .. }
        | Op::CmpNeImmJmp { r: src, .. }
        | Op::JmpNilOrFalseJmp { r: src, .. }
        | Op::AccessK(_, src, _)
        | Op::IndexK(_, src, _)
        | Op::ListIndexI(_, src, _)
        | Op::StrIndexI(_, src, _)
        | Op::StartsWithK(_, src, _)
        | Op::ContainsK(_, src, _)
        | Op::MapGetInterned(_, src, _)
        | Op::MapHasK(_, src, _) => remap(src),
        Op::CmpI { a, b, .. } | Op::CmpIntJmp { a, b, .. } => {
            remap(a);
            remap(b);
        }
        Op::NullishPick { l, .. } | Op::JmpFalseSet { r: l, .. } | Op::JmpTrueSet { r: l, .. } => remap(l),
        Op::PatternMatch { src, .. } => remap(src),
        _ => {}
    }

    changed
}

fn reg_dead_after_single_consumer(code: &[Op], start: usize, reg: u16) -> bool {
    for op in &code[start..] {
        if op_reads_reg(op, reg) {
            return false;
        }
        if op_writes_reg(op, reg) {
            return true;
        }
    }
    true
}

fn op_reads_reg(op: &Op, reg: u16) -> bool {
    let is = |value: &u16| *value == reg;
    match op {
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
        | Op::CmpLtImmJmp { r: src, .. }
        | Op::JmpNilOrFalseJmp { r: src, .. }
        | Op::CmpLeImmJmp { r: src, .. }
        | Op::CmpEqImmJmp { r: src, .. }
        | Op::CmpGtImmJmp { r: src, .. }
        | Op::CmpGeImmJmp { r: src, .. }
        | Op::CmpNeImmJmp { r: src, .. }
        | Op::Ret { base: src, retc: 1 }
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
            reg >= *base && reg < end
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
        | Op::CallNativeFast { f, base, argc, .. } => {
            is(f) || (reg >= *base && reg < base.saturating_add(*argc as u16))
        }
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
                || (reg >= *base_pos && reg < base_pos.saturating_add(*posc as u16))
                || (reg >= *base_named && reg < base_named.saturating_add((*namedc as u16).saturating_mul(2)))
        }
        _ => false,
    }
}

pub(super) fn op_writes_reg(op: &Op, reg: u16) -> bool {
    let is = |value: &u16| *value == reg;
    match op {
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
        | Op::CallNativeFast { base, retc, .. } => reg >= *base && reg < base.saturating_add(*retc as u16),
        Op::CallMethod0 { dst, .. } => is(dst),
        Op::CallGlobalMethod0 { dst, .. } => is(dst),
        Op::CallNamed { base_pos, retc, .. } | Op::CallNamedFallback { base_pos, retc, .. } => {
            reg >= *base_pos && reg < base_pos.saturating_add(*retc as u16)
        }
        _ => false,
    }
}

fn fixup_offsets(code: &mut [Op], removals: &[usize]) {
    for (p, op) in code.iter_mut().enumerate() {
        let old_src = p + count_removed_before(p, removals);
        let new_ofs = match &*op {
            Op::Jmp(ofs)
            | Op::JmpFalse(_, ofs)
            | Op::BoolBranch(_, ofs)
            | Op::JmpFalseSet { ofs, .. }
            | Op::JmpTrueSet { ofs, .. }
            | Op::NullishPick { ofs, .. }
            | Op::CmpLtImmJmp { ofs, .. }
            | Op::CmpIntJmp { ofs, .. }
            | Op::CmpLeImmJmp { ofs, .. }
            | Op::CmpEqImmJmp { ofs, .. }
            | Op::CmpGtImmJmp { ofs, .. }
            | Op::CmpGeImmJmp { ofs, .. }
            | Op::CmpNeImmJmp { ofs, .. }
            | Op::JmpNilOrFalseJmp { ofs, .. }
            | Op::AddIntImmJmp { ofs, .. }
            | Op::Break(ofs)
            | Op::Continue(ofs) => {
                let old_target = old_src as isize + *ofs as isize;
                let new_target = old_target - shifted(old_target as usize, removals);
                (new_target - p as isize) as i16
            }
            Op::ForRangeLoop { ofs, .. } | Op::RangeLoopI { ofs, .. } => {
                let old_target = old_src as isize + *ofs as isize;
                let new_target = old_target - shifted(old_target as usize, removals);
                (new_target - p as isize) as i16
            }
            Op::ForRangeStep { back_ofs, .. } => {
                let old_target = old_src as isize + *back_ofs as isize;
                let new_target = old_target - shifted(old_target as usize, removals);
                (new_target - p as isize) as i16
            }
            _ => continue,
        };
        set_offset(op, new_ofs);
    }
}

/// How many removal positions (original coords) are strictly before `pos`?
fn shifted(pos: usize, removals: &[usize]) -> isize {
    removals.iter().take_while(|&&r| r < pos).count() as isize
}

/// How many removal positions are at or before new_pos + accumulated shift?
fn count_removed_before(new_pos: usize, removals: &[usize]) -> usize {
    let mut shift = 0;
    loop {
        let n = removals.iter().take_while(|&&r| r <= new_pos + shift).count();
        if n == shift {
            break;
        }
        shift = n;
    }
    shift
}

fn set_offset(op: &mut Op, ofs: i16) {
    match op {
        Op::Jmp(o)
        | Op::JmpFalse(_, o)
        | Op::BoolBranch(_, o)
        | Op::JmpFalseSet { ofs: o, .. }
        | Op::JmpTrueSet { ofs: o, .. }
        | Op::NullishPick { ofs: o, .. }
        | Op::CmpLtImmJmp { ofs: o, .. }
        | Op::CmpIntJmp { ofs: o, .. }
        | Op::CmpLeImmJmp { ofs: o, .. }
        | Op::CmpEqImmJmp { ofs: o, .. }
        | Op::CmpGtImmJmp { ofs: o, .. }
        | Op::CmpGeImmJmp { ofs: o, .. }
        | Op::CmpNeImmJmp { ofs: o, .. }
        | Op::JmpNilOrFalseJmp { ofs: o, .. }
        | Op::AddIntImmJmp { ofs: o, .. }
        | Op::Break(o)
        | Op::Continue(o) => *o = ofs,
        Op::ForRangeLoop { ofs: o, .. } | Op::RangeLoopI { ofs: o, .. } => *o = ofs,
        Op::ForRangeStep { back_ofs: o, .. } => *o = ofs,
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::bytecode::IntCmpKind;

    #[test]
    fn cmp_i_followed_by_branch_fuses_to_cmp_int_jmp() {
        let mut code = vec![
            Op::CmpI {
                dst: 4,
                a: 1,
                b: 2,
                kind: IntCmpKind::Lt,
            },
            Op::JmpFalse(4, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(
            code[0],
            Op::CmpIntJmp {
                kind: IntCmpKind::Lt,
                a: 1,
                b: 2,
                ofs: 2
            }
        ));
    }

    #[test]
    fn cmp_ne_imm_followed_by_branch_fuses_to_cmp_ne_imm_jmp() {
        let mut code = vec![
            Op::CmpNeImm(4, 1, 7),
            Op::BoolBranch(4, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::CmpNeImmJmp { r: 1, imm: 7, ofs: 2 }));
    }

    #[test]
    fn cmp_eq_imm_followed_by_branch_fuses_to_cmp_eq_imm_jmp() {
        let mut code = vec![
            Op::CmpEqImm(4, 1, 7),
            Op::BoolBranch(4, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::CmpEqImmJmp { r: 1, imm: 7, ofs: 2 }));
    }

    #[test]
    fn cmp_gt_ge_imm_followed_by_branch_fuses_to_cmp_imm_jmp() {
        let mut code = vec![
            Op::CmpGtImm(4, 1, 7),
            Op::BoolBranch(4, 2),
            Op::CmpGeImm(5, 2, 9),
            Op::BoolBranch(5, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::CmpGtImmJmp { r: 1, imm: 7, ofs: 2 }));
        assert!(matches!(code[1], Op::CmpGeImmJmp { r: 2, imm: 9, ofs: 2 }));
    }

    #[test]
    fn wide_cmp_imm_followed_by_branch_fuses_to_cmp_imm_jmp() {
        let mut code = vec![
            Op::CmpGtImm(4, 1, 900),
            Op::BoolBranch(4, 2),
            Op::CmpNeImm(5, 2, -300),
            Op::BoolBranch(5, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::CmpGtImmJmp { r: 1, imm: 900, ofs: 2 }));
        assert!(matches!(
            code[1],
            Op::CmpNeImmJmp {
                r: 2,
                imm: -300,
                ofs: 2
            }
        ));
    }

    #[test]
    fn cmp_lt_le_imm_followed_by_branch_fuses_to_cmp_imm_jmp() {
        let mut code = vec![
            Op::CmpLtImm(4, 1, 7),
            Op::BoolBranch(4, 2),
            Op::CmpLeImm(5, 2, 9),
            Op::BoolBranch(5, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::CmpLtImmJmp { r: 1, imm: 7, ofs: 2 }));
        assert!(matches!(code[1], Op::CmpLeImmJmp { r: 2, imm: 9, ofs: 2 }));
    }

    #[test]
    fn map_get_dynamic_ne_nil_branch_fuses_to_map_has() {
        let consts = vec![Val::Nil];
        let mut code = vec![
            Op::MapGetDynamic(4, 1, 2),
            Op::LoadK(5, 0),
            Op::CmpNe(6, 4, 5),
            Op::BoolBranch(6, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::MapHas(6, 1, 2)));
        assert!(matches!(code[1], Op::BoolBranch(6, 2)));
    }

    #[test]
    fn map_get_dynamic_ne_prior_nil_reg_branch_fuses_to_map_has() {
        let consts = vec![Val::Nil];
        let mut code = vec![
            Op::LoadK(5, 0),
            Op::MapGetDynamic(4, 1, 2),
            Op::CmpNe(6, 4, 5),
            Op::BoolBranch(6, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 5);
        assert!(matches!(code[1], Op::MapHas(6, 1, 2)));
        assert!(matches!(code[2], Op::BoolBranch(6, 2)));
    }

    #[test]
    fn map_get_dynamic_ne_nil_rk_branch_fuses_to_map_has() {
        let consts = vec![Val::Nil];
        let mut code = vec![
            Op::MapGetDynamic(4, 1, 2),
            Op::CmpNe(6, 4, rk_make_const(0)),
            Op::BoolBranch(6, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::MapHas(6, 1, 2)));
        assert!(matches!(code[1], Op::BoolBranch(6, 2)));
    }

    #[test]
    fn map_get_interned_ne_nil_branch_fuses_to_map_has_k() {
        let consts = vec![Val::from_str("k"), Val::Nil];
        let mut code = vec![
            Op::MapGetInterned(4, 1, 0),
            Op::LoadK(5, 1),
            Op::CmpNe(6, 4, 5),
            Op::JmpFalse(6, 2),
            Op::Ret { base: 0, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::MapHasK(6, 1, 0)));
        assert!(matches!(code[1], Op::BoolBranch(6, 2)));
    }

    #[test]
    fn loadlocal_single_read_operand_is_remapped() {
        let mut code = vec![Op::LoadLocal(4, 1), Op::Add(5, 4, 2), Op::Ret { base: 5, retc: 1 }];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::Add(5, 1, 2)));
        assert!(matches!(code[1], Op::Ret { base: 5, retc: 1 }));
    }

    #[test]
    fn loadlocal_assignment_staging_is_preserved() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::Move(5, 4),
            Op::LoadLocal(6, 1),
            Op::StoreLocal(7, 6),
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::LoadLocal(4, 1)));
        assert!(matches!(code[1], Op::Move(5, 4)));
        assert!(matches!(code[2], Op::LoadLocal(6, 1)));
        assert!(matches!(code[3], Op::StoreLocal(7, 6)));
    }

    #[test]
    fn loadlocal_with_second_consumer_is_preserved() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::JmpIfNil(4, 2),
            Op::PatternMatch {
                dst: 5,
                src: 4,
                plan: 0,
            },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::LoadLocal(4, 1)));
        assert!(matches!(code[1], Op::JmpIfNil(4, 2)));
        assert!(matches!(
            code[2],
            Op::PatternMatch {
                dst: 5,
                src: 4,
                plan: 0
            }
        ));
    }

    #[test]
    fn loadk_rk_compatible_operand_is_remapped() {
        let mut code = vec![Op::LoadK(4, 3), Op::Add(5, 1, 4), Op::Ret { base: 5, retc: 1 }];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::Add(5, 1, rhs) if rhs == rk_make_const(3)));
        assert!(matches!(code[1], Op::Ret { base: 5, retc: 1 }));
    }

    #[test]
    fn loadk_non_rk_consumer_is_preserved() {
        let mut code = vec![Op::LoadK(4, 3), Op::StoreLocal(8, 4)];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::LoadK(4, 3)));
        assert!(matches!(code[1], Op::StoreLocal(8, 4)));
    }
}
