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
            if let Some(next) = remap_const_string_concat(code, consts, i, dst, kidx) {
                code[i + 1] = next;
                removals.push(i);
                i += 2;
                continue;
            }

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

        if let Op::LoadLocal(dst, idx) = code[i]
            && let Some((consumer_idx, consumer)) = remap_deferred_loadlocal_consumer(code, i, dst, idx)
        {
            code[consumer_idx] = consumer;
            removals.push(i);
            i += 1;
            continue;
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

    remap_deferred_loadlocals_to_fixpoint(code);
    remap_multi_read_loadlocals_to_fixpoint(code);
    remap_identity_toiters_to_fixpoint(code, consts);
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

fn remap_deferred_loadlocals_to_fixpoint(code: &mut Vec<Op>) {
    loop {
        let mut changed = false;
        let mut i = 0;
        while i < code.len() {
            if let Op::LoadLocal(dst, idx) = code[i]
                && let Some((consumer_idx, consumer)) = remap_deferred_loadlocal_consumer(code, i, dst, idx)
            {
                code[consumer_idx] = consumer;
                code.remove(i);
                fixup_offsets(code, &[i]);
                changed = true;
                continue;
            }
            i += 1;
        }
        if !changed {
            return;
        }
    }
}

fn remap_multi_read_loadlocals_to_fixpoint(code: &mut Vec<Op>) {
    loop {
        let mut changed = false;
        let mut i = 0;
        while i < code.len() {
            if let Op::LoadLocal(dst, src) = code[i]
                && let Some(remap) = collect_multi_read_temp_remap(code, i, dst, src, false)
            {
                for (idx, op) in remap.consumers {
                    code[idx] = op;
                }
                code.remove(i);
                fixup_offsets(code, &[i]);
                changed = true;
                continue;
            }
            i += 1;
        }
        if !changed {
            return;
        }
    }
}

struct MultiReadLoadLocalRemap {
    consumers: Vec<(usize, Op)>,
}

fn has_external_branch_target_into_range(
    code: &[Op],
    load_idx: usize,
    last_read_idx: usize,
    allow_loop_back_edges: bool,
) -> bool {
    code.iter().enumerate().any(|(pc, op)| {
        let Some(target) = crate::vm::op_branch_target(pc, op) else {
            return false;
        };
        target > load_idx
            && target <= last_read_idx
            && (pc <= load_idx || (!allow_loop_back_edges && pc > last_read_idx))
    })
}

fn remap_identity_toiters_to_fixpoint(code: &mut Vec<Op>, consts: &[Val]) {
    loop {
        let mut changed = false;
        let mut i = 0;
        while i < code.len() {
            if let Op::ToIter { dst, src } = code[i]
                && to_iter_source_is_identity_value(code, consts, i, src)
                && let Some(remap) = collect_multi_read_temp_remap(code, i, dst, src, true)
            {
                for (idx, op) in remap.consumers {
                    code[idx] = op;
                }
                code.remove(i);
                fixup_offsets(code, &[i]);
                changed = true;
                continue;
            }
            i += 1;
        }
        if !changed {
            return;
        }
    }
}

fn collect_multi_read_temp_remap(
    code: &[Op],
    producer_idx: usize,
    dst: u16,
    src: u16,
    allow_loop_back_edges: bool,
) -> Option<MultiReadLoadLocalRemap> {
    const LOOKAHEAD_LIMIT: usize = 32;
    let end = code.len().min(producer_idx + 1 + LOOKAHEAD_LIMIT);
    let mut consumers = Vec::new();

    for idx in producer_idx + 1..end {
        let op = &code[idx];
        if crate::vm::op_writes_register(op, src) || crate::vm::op_writes_register(op, dst) {
            return None;
        }

        if crate::vm::op_reads_register(op, dst) {
            let mut remapped = op.clone();
            if !remap_single_read_operand(&mut remapped, dst, src) {
                return None;
            }
            consumers.push((idx, remapped));
            if reg_dead_after_single_consumer(code, idx + 1, dst) {
                if has_external_branch_target_into_range(code, producer_idx, idx, allow_loop_back_edges) {
                    return None;
                }
                return Some(MultiReadLoadLocalRemap { consumers });
            }
        }
    }

    None
}

fn to_iter_source_is_identity_value(code: &[Op], consts: &[Val], to_iter_idx: usize, src: u16) -> bool {
    for idx in (0..to_iter_idx).rev() {
        let op = &code[idx];
        if crate::vm::op_writes_register(op, src) {
            return op_writes_string_or_list_value(op, consts);
        }
        if crate::vm::op_is_control_boundary(op) || has_branch_target_to(code, idx) {
            return false;
        }
    }
    false
}

fn op_writes_string_or_list_value(op: &Op, consts: &[Val]) -> bool {
    match *op {
        Op::LoadK(_, kidx) => matches!(
            consts.get(kidx as usize),
            Some(Val::ShortStr(_) | Val::Str(_) | Val::List(_))
        ),
        Op::ToStr(_, _)
        | Op::StrConcatKnownCap(_, _, _)
        | Op::StrConcatToStr(_, _, _)
        | Op::BuildList { .. }
        | Op::ListSlice { .. } => true,
        _ => false,
    }
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
        if crate::vm::op_writes_register(op, reg) {
            return matches!(op, Op::LoadK(dst, kidx) if *dst == reg && matches!(consts.get(*kidx as usize), Some(Val::Nil)));
        }
        if crate::vm::op_reads_register(op, reg) || crate::vm::op_is_control_boundary(op) {
            return false;
        }
    }
    false
}

fn branch_reads(op: &Op, reg: u16) -> Option<()> {
    match *op {
        Op::JmpFalse(r, _) | Op::BoolBranch(r, _) if r == reg => Some(()),
        _ => None,
    }
}

fn has_branch_target_to(code: &[Op], target: usize) -> bool {
    crate::vm::has_branch_target_to(code, target)
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

fn remap_const_string_concat(code: &[Op], consts: &[Val], load_idx: usize, dst: u16, kidx: u16) -> Option<Op> {
    if !matches!(consts.get(kidx as usize), Some(Val::ShortStr(_) | Val::Str(_))) {
        return None;
    }
    let rk = rk_make_const(kidx);
    let next = code.get(load_idx + 1)?;
    let (out, remapped) = match *next {
        Op::StrConcatKnownCap(out, a, b) if a == dst => (out, Op::Add(out, rk, b)),
        Op::StrConcatKnownCap(out, a, b) if b == dst => (out, Op::Add(out, a, rk)),
        Op::StrConcatToStr(out, lhs, src)
            if lhs == dst && to_str_source_is_add_equivalent(code, consts, load_idx + 1, src) =>
        {
            (out, Op::Add(out, rk, src))
        }
        _ => return None,
    };
    (out == dst || reg_dead_after_single_consumer(code, load_idx + 2, dst)).then_some(remapped)
}

fn to_str_source_is_add_equivalent(code: &[Op], consts: &[Val], pos: usize, src: u16) -> bool {
    for idx in (0..pos).rev() {
        let op = &code[idx];
        if crate::vm::op_writes_register(op, src) {
            return op_writes_string_or_number_value(op, consts, src);
        }
        if crate::vm::op_is_control_boundary(op) || has_branch_target_to(code, idx) {
            return false;
        }
    }
    false
}

fn op_writes_string_or_number_value(op: &Op, consts: &[Val], reg: u16) -> bool {
    match *op {
        Op::LoadK(dst, kidx) if dst == reg => matches!(
            consts.get(kidx as usize),
            Some(Val::ShortStr(_) | Val::Str(_) | Val::Int(_) | Val::Float(_))
        ),
        Op::ToStr(dst, _)
        | Op::StrConcatKnownCap(dst, _, _)
        | Op::StrConcatToStr(dst, _, _)
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
        | Op::Floor { dst, .. }
        | Op::FloorDivImm { dst, .. }
            if dst == reg =>
        {
            true
        }
        Op::RangeLoopI {
            idx, write_idx: true, ..
        } if idx == reg => true,
        _ => false,
    }
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
        | Op::FloorDivImm { src, .. }
        | Op::ToIter { src, .. } => remap(src),
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
        Op::ForRangePrep { limit, .. } => remap(limit),
        Op::PatternMatch { src, .. } => remap(src),
        _ => {}
    }

    changed
}

fn remap_deferred_loadlocal_consumer(code: &[Op], load_idx: usize, dst: u16, src: u16) -> Option<(usize, Op)> {
    const LOOKAHEAD_LIMIT: usize = 8;
    let end = code.len().min(load_idx + 1 + LOOKAHEAD_LIMIT);
    for consumer_idx in load_idx + 1..end {
        if has_branch_target_to(code, consumer_idx) {
            return None;
        }
        let op = &code[consumer_idx];
        if crate::vm::op_reads_register(op, dst) {
            let mut remapped = op.clone();
            if remap_single_read_operand(&mut remapped, dst, src)
                && reg_dead_after_single_consumer(code, consumer_idx + 1, dst)
            {
                return Some((consumer_idx, remapped));
            }
            return None;
        }
        if !can_defer_loadlocal_across(op, dst, src) {
            return None;
        }
    }
    None
}

fn can_defer_loadlocal_across(op: &Op, dst: u16, src: u16) -> bool {
    if crate::vm::op_is_control_boundary(op)
        || crate::vm::op_reads_register(op, dst)
        || crate::vm::op_writes_register(op, dst)
        || crate::vm::op_writes_register(op, src)
    {
        return false;
    }
    matches!(
        op,
        Op::LoadK(_, _)
            | Op::LoadLocal(_, _)
            | Op::Move(_, _)
            | Op::Not(_, _)
            | Op::ToStr(_, _)
            | Op::ToBool(_, _)
            | Op::Add(_, _, _)
            | Op::StrConcatKnownCap(_, _, _)
            | Op::StrConcatToStr(_, _, _)
            | Op::Sub(_, _, _)
            | Op::Mul(_, _, _)
            | Op::Div(_, _, _)
            | Op::Mod(_, _, _)
            | Op::AddInt(_, _, _)
            | Op::AddFloat(_, _, _)
            | Op::AddIntImm(_, _, _)
            | Op::SubInt(_, _, _)
            | Op::SubFloat(_, _, _)
            | Op::MulInt(_, _, _)
            | Op::MulFloat(_, _, _)
            | Op::DivFloat(_, _, _)
            | Op::ModInt(_, _, _)
            | Op::ModFloat(_, _, _)
            | Op::CmpEq(_, _, _)
            | Op::CmpNe(_, _, _)
            | Op::CmpLt(_, _, _)
            | Op::CmpLe(_, _, _)
            | Op::CmpGt(_, _, _)
            | Op::CmpGe(_, _, _)
            | Op::CmpEqImm(_, _, _)
            | Op::CmpNeImm(_, _, _)
            | Op::CmpLtImm(_, _, _)
            | Op::CmpLeImm(_, _, _)
            | Op::CmpGtImm(_, _, _)
            | Op::CmpGeImm(_, _, _)
            | Op::In(_, _, _)
            | Op::Access(_, _, _)
            | Op::AccessK(_, _, _)
            | Op::IndexK(_, _, _)
            | Op::ListIndexI(_, _, _)
            | Op::StrIndexI(_, _, _)
            | Op::Len { .. }
            | Op::ListLen { .. }
            | Op::MapLen { .. }
            | Op::StrLen { .. }
            | Op::StartsWithK(_, _, _)
            | Op::ContainsK(_, _, _)
            | Op::MapHas(_, _, _)
            | Op::MapGetInterned(_, _, _)
            | Op::MapGetDynamic(_, _, _)
            | Op::MapHasK(_, _, _)
            | Op::Floor { .. }
            | Op::FloorDivImm { .. }
    )
}

fn reg_dead_after_single_consumer(code: &[Op], start: usize, reg: u16) -> bool {
    crate::vm::register_dead_after_ops(code[start..].iter(), reg)
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
    }

    #[test]
    fn delayed_loadlocal_single_read_operand_is_remapped() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::LoadK(5, 2),
            Op::LoadLocal(6, 3),
            Op::MapGetDynamic(7, 6, 4),
            Op::Ret { base: 7, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::LoadK(5, 2)));
        assert!(matches!(code[1], Op::MapGetDynamic(7, 3, 1)));
    }

    #[test]
    fn delayed_loadlocal_crosses_readonly_access_op() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::LoadLocal(6, 3),
            Op::MapGetDynamic(7, 10, 11),
            Op::MapGetDynamic(8, 6, 4),
            Op::Ret { base: 8, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::MapGetDynamic(7, 10, 11)));
        assert!(matches!(code[1], Op::MapGetDynamic(8, 3, 1)));
    }

    #[test]
    fn delayed_loadlocal_reaches_fixpoint_after_neighbor_removal() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::LoadLocal(6, 3),
            Op::MapGetDynamic(7, 6, 4),
            Op::Ret { base: 7, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::MapGetDynamic(7, 3, 1)));
    }

    #[test]
    fn delayed_loadlocal_stops_when_readonly_op_writes_temp() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::MapGetDynamic(4, 10, 11),
            Op::MapGetDynamic(8, 6, 4),
            Op::Ret { base: 8, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[0], Op::LoadLocal(4, 1)));
        assert!(matches!(code[1], Op::MapGetDynamic(4, 10, 11)));
    }

    #[test]
    fn multi_read_loadlocal_remaps_across_internal_branches() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::CmpGtImmJmp { r: 4, imm: 10, ofs: 2 },
            Op::AddIntImmJmp { r: 9, imm: 1, ofs: 2 },
            Op::CmpGtImmJmp { r: 4, imm: 3, ofs: 2 },
            Op::AddIntImm(9, 9, 1),
            Op::Ret { base: 9, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 5);
        assert!(matches!(code[0], Op::CmpGtImmJmp { r: 1, imm: 10, .. }));
        assert!(matches!(code[2], Op::CmpGtImmJmp { r: 1, imm: 3, .. }));
    }

    #[test]
    fn multi_read_loadlocal_stops_when_source_is_written_before_last_read() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::CmpGtImmJmp { r: 4, imm: 10, ofs: 2 },
            Op::StoreLocal(1, 8),
            Op::CmpGtImmJmp { r: 4, imm: 3, ofs: 2 },
            Op::Ret { base: 4, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert!(matches!(code[0], Op::LoadLocal(4, 1)));
    }

    #[test]
    fn multi_read_loadlocal_rejects_external_jump_into_candidate_region() {
        let mut code = vec![
            Op::CmpEqImmJmp { r: 0, imm: 0, ofs: 3 },
            Op::LoadLocal(4, 1),
            Op::CmpGtImmJmp { r: 4, imm: 10, ofs: 2 },
            Op::CmpGtImmJmp { r: 4, imm: 3, ofs: 2 },
            Op::Ret { base: 4, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert!(matches!(code[1], Op::LoadLocal(4, 1)));
    }

    #[test]
    fn delayed_loadlocal_for_range_limit_is_remapped() {
        let mut code = vec![
            Op::LoadK(0, 0),
            Op::LoadLocal(4, 1),
            Op::LoadK(5, 2),
            Op::ForRangePrep {
                idx: 0,
                limit: 4,
                step: 6,
                inclusive: true,
                explicit: false,
            },
            Op::RangeLoopI {
                idx: 0,
                limit: 1,
                step: 6,
                inclusive: true,
                write_idx: true,
                ofs: 2,
            },
            Op::Ret { base: 0, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 5);
        assert!(matches!(code[2], Op::ForRangePrep { limit: 1, .. }));
    }

    #[test]
    fn delayed_loadlocal_to_iter_source_is_remapped() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::ToIter { dst: 5, src: 4 },
            Op::Ret { base: 5, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::ToIter { dst: 5, src: 1 }));
    }

    #[test]
    fn identity_to_iter_on_string_source_is_removed() {
        let consts = vec![Val::from_str("tenant")];
        let mut code = vec![
            Op::LoadK(1, 0),
            Op::ToIter { dst: 2, src: 1 },
            Op::Len { dst: 3, src: 2 },
            Op::Index {
                dst: 4,
                base: 2,
                idx: 5,
            },
            Op::Ret { base: 4, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[1], Op::Len { dst: 3, src: 1 }));
        assert!(matches!(
            code[2],
            Op::Index {
                dst: 4,
                base: 1,
                idx: 5
            }
        ));
    }

    #[test]
    fn identity_to_iter_is_preserved_for_unknown_source() {
        let mut code = vec![
            Op::LoadGlobal(1, 0),
            Op::ToIter { dst: 2, src: 1 },
            Op::Len { dst: 3, src: 2 },
            Op::Ret { base: 3, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 4);
        assert!(matches!(code[1], Op::ToIter { dst: 2, src: 1 }));
    }

    #[test]
    fn identity_to_iter_rejects_external_jump_into_candidate_region() {
        let consts = vec![Val::from_str("tenant")];
        let mut code = vec![
            Op::CmpEqImmJmp { r: 0, imm: 0, ofs: 3 },
            Op::LoadK(1, 0),
            Op::ToIter { dst: 2, src: 1 },
            Op::Len { dst: 3, src: 2 },
            Op::Ret { base: 3, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert!(matches!(code[2], Op::ToIter { dst: 2, src: 1 }));
    }

    #[test]
    fn const_string_known_cap_concat_uses_rk_add() {
        let consts = vec![Val::from_str("tenant-")];
        let mut code = vec![
            Op::LoadK(4, 0),
            Op::StrConcatKnownCap(5, 1, 4),
            Op::Ret { base: 5, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::Add(5, 1, rhs) if rhs == rk_make_const(0)));
    }

    #[test]
    fn const_string_to_str_concat_uses_rk_add_for_known_number_source() {
        let consts = vec![Val::from_str("tenant-")];
        let mut code = vec![
            Op::ModInt(3, 1, 2),
            Op::LoadK(4, 0),
            Op::StrConcatToStr(5, 4, 3),
            Op::Ret { base: 5, retc: 1 },
        ];

        peephole_fuse_cmp_jmp_with_consts(&mut code, &consts);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[1], Op::Add(5, lhs, 3) if lhs == rk_make_const(0)));
    }

    #[test]
    fn delayed_loadlocal_for_range_idx_is_preserved() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::ForRangePrep {
                idx: 4,
                limit: 2,
                step: 3,
                inclusive: true,
                explicit: false,
            },
            Op::Ret { base: 4, retc: 1 },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::LoadLocal(4, 1)));
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
    fn loadlocal_with_remappable_second_consumer_is_rewritten() {
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

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::JmpIfNil(1, 2)));
        assert!(matches!(
            code[1],
            Op::PatternMatch {
                dst: 5,
                src: 1,
                plan: 0
            }
        ));
    }

    #[test]
    fn loadlocal_with_unremappable_second_consumer_is_preserved() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::JmpIfNil(4, 2),
            Op::BuildList {
                dst: 5,
                base: 4,
                len: 1,
            },
        ];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 3);
        assert!(matches!(code[0], Op::LoadLocal(4, 1)));
        assert!(matches!(code[1], Op::JmpIfNil(4, 2)));
        assert!(matches!(
            code[2],
            Op::BuildList {
                dst: 5,
                base: 4,
                len: 1
            }
        ));
    }

    #[test]
    fn loadk_rk_compatible_operand_is_remapped() {
        let mut code = vec![Op::LoadK(4, 3), Op::Add(5, 1, 4), Op::Ret { base: 5, retc: 1 }];

        peephole_fuse_cmp_jmp(&mut code);

        assert_eq!(code.len(), 2);
        assert!(matches!(code[0], Op::Add(5, 1, rhs) if rhs == rk_make_const(3)));
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
