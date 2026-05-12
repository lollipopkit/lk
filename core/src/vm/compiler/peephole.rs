//! Peephole optimization passes for LKR bytecode.
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
///
/// The second instruction is removed and all relative jump offsets are adjusted.
pub fn peephole_fuse_cmp_jmp(code: &mut Vec<Op>) {
    let mut removals: Vec<usize> = Vec::new();

    let mut i = 0;
    while i + 1 < code.len() {
        match (&code[i], &code[i + 1]) {
            (Op::CmpLtImm(dst, src, imm), Op::JmpFalse(r, ofs)) if *dst == *r
                && (-128..=127).contains(imm) && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpLtImmJmp { r: *src, imm: *imm, ofs: *ofs + 1 };
                removals.push(i + 1);
                i += 2;
            }
            (Op::CmpLeImm(dst, src, imm), Op::JmpFalse(r, ofs)) if *dst == *r
                && (-128..=127).contains(imm) && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::CmpLeImmJmp { r: *src, imm: *imm, ofs: *ofs + 1 };
                removals.push(i + 1);
                i += 2;
            }
            (Op::AddIntImm(dst, src, imm), Op::Jmp(ofs)) if dst == src
                && (-128..=127).contains(imm) && (-128..=127).contains(ofs) =>
            {
                code[i] = Op::AddIntImmJmp { r: *dst, imm: *imm, ofs: *ofs + 1 };
                removals.push(i + 1);
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    if removals.is_empty() {
        return;
    }

    // Remove fused instructions (reverse order to keep indices valid)
    for &idx in removals.iter().rev() {
        code.remove(idx);
    }

    // Fix all relative jump offsets
    fixup_offsets(code, &removals);
}

fn fixup_offsets(code: &mut Vec<Op>, removals: &[usize]) {
    for p in 0..code.len() {
        let old_src = p + count_removed_before(p, removals);
        let new_ofs = match &code[p] {
            Op::Jmp(ofs)
            | Op::JmpFalse(_, ofs)
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
            Op::ForRangeLoop { ofs, .. } => {
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
        set_offset(&mut code[p], new_ofs);
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
        if n == shift { break; }
        shift = n;
    }
    shift
}

fn set_offset(op: &mut Op, ofs: i16) {
    match op {
        Op::Jmp(o)
        | Op::JmpFalse(_, o)
        | Op::JmpFalseSet { ofs: o, .. }
        | Op::JmpTrueSet { ofs: o, .. }
        | Op::NullishPick { ofs: o, .. }
        | Op::CmpLtImmJmp { ofs: o, .. }
        | Op::CmpLeImmJmp { ofs: o, .. }
        | Op::JmpNilOrFalseJmp { ofs: o, .. }
        | Op::AddIntImmJmp { ofs: o, .. }
        | Op::Break(o)
        | Op::Continue(o) => *o = ofs,
        Op::ForRangeLoop { ofs: o, .. } => *o = ofs,
        Op::ForRangeStep { back_ofs: o, .. } => *o = ofs,
        _ => unreachable!(),
    }
}