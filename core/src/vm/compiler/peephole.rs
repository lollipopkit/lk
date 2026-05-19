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
//! | `CmpLtImm + JmpFalse` | `CmpLtImmJmp` | 1 dispatch/iteration savings in while loops |
//! | `CmpLeImm + JmpFalse` | `CmpLeImmJmp` | Same for <= based loops |
//! | `AddIntImm + Jmp` | `AddIntImmJmp` | Loop increment tail fusion |
//! | `AddIntImm + ForRangeStep` | `AddIntImmJmp` | Range-loop accumulator tail fusion |
//! | `LoadLocal + Ret/branch` | direct `Ret/branch` from local | Avoid copy-only locals |
//! | `LoadLocal + read-only op` | read source local directly | Avoid copy-only locals |
//!
//! These fused ops are handled natively in `opcode.rs`. BC32 packing sees
//! them as unsupported opcodes and gracefully skips those functions, which
//! then run on the optimized opcode.rs path.

use crate::vm::bytecode::Op;

/// Fuse common two-instruction patterns into single fused opcodes.
///
/// 1. `CmpLtImm(dst, src, imm)` + `JmpFalse(dst, ofs)` → `CmpLtImmJmp { src, imm, ofs+1 }`
/// 2. `CmpLeImm(dst, src, imm)` + `JmpFalse(dst, ofs)` → `CmpLeImmJmp { src, imm, ofs+1 }`
/// 3. `AddIntImm(dst, src, imm)` + `Jmp(ofs)` (when dst==src) → `AddIntImmJmp { dst, imm, ofs+1 }`
/// 4. `AddIntImm(dst, src, imm)` + `ForRangeStep(back)` (when dst==src) → `AddIntImmJmp { dst, imm, back+1 }`
/// 5. `LoadLocal(tmp, idx)` + single consumer read-only op using `tmp` → consumer reads `idx`
///
/// The second instruction is removed and all relative jump offsets are adjusted.
pub fn peephole_fuse_cmp_jmp(code: &mut Vec<Op>) {
    let mut removals: Vec<usize> = Vec::new();

    let mut i = 0;
    while i + 1 < code.len() {
        if let Op::LoadLocal(dst, idx) = code[i] {
            let mut next = code[i + 1].clone();
            if remap_single_read_operand(&mut next, dst, idx) {
                code[i + 1] = next;
                removals.push(i);
                i += 2;
                continue;
            }
        }

        match (&code[i], &code[i + 1]) {
            (Op::CmpLtImm(dst, src, imm), Op::JmpFalse(r, ofs))
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
            (Op::CmpLeImm(dst, src, imm), Op::JmpFalse(r, ofs))
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
        | Op::Floor { src, .. } => remap(src),
        Op::Add(_, a, b)
        | Op::StrConcatKnownCap(_, a, b)
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
        | Op::ListSlice {
            src: a, start: b, ..
        }
        | Op::MapHas(_, a, b)
        | Op::MapGetDynamic(_, a, b) => {
            remap(a);
            remap(b);
        }
        Op::AddIntImm(_, src, _)
        | Op::CmpLtImmJmp { r: src, .. }
        | Op::CmpLeImmJmp { r: src, .. }
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
        Op::CmpI { a, b, .. } => {
            remap(a);
            remap(b);
        }
        Op::NullishPick { l, .. }
        | Op::JmpFalseSet { r: l, .. }
        | Op::JmpTrueSet { r: l, .. } => remap(l),
        Op::PatternMatch { src, .. } => remap(src),
        _ => {}
    }

    changed
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
            | Op::CmpLeImmJmp { ofs, .. }
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
        | Op::CmpLeImmJmp { ofs: o, .. }
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

    #[test]
    fn loadlocal_single_read_operand_is_remapped() {
        let mut code = vec![
            Op::LoadLocal(4, 1),
            Op::Add(5, 4, 2),
            Op::Ret { base: 5, retc: 1 },
        ];

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
}
