use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::bytecode::{IntCmpKind, Op};
use crate::vm::vm::frame::FrameState;
use crate::vm::vm::quickening::{self, QuickeningSite};

use super::super::helpers::assign_reg;
use super::super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, rk_read};

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_eq(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if quickening::execute_cmp_eq_site(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }
    let result = rk_read(regs, consts, a) == rk_read(regs, consts, b);
    assign_reg(frame_raw, regs, dst as usize, Val::Bool(result));
    Ok(())
}

#[inline]
pub(super) fn run_cmp_eq_jmp_false(regs: &[Val], consts: &[Val], pc: usize, ofs: i16, a: u16, b: u16) -> usize {
    let result = rk_read(regs, consts, a) == rk_read(regs, consts, b);
    branch_after_cmp(pc, ofs, result)
}

#[inline]
pub(super) fn run_cmp_i(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    a: u16,
    b: u16,
    kind: IntCmpKind,
) -> Result<()> {
    let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) else {
        anyhow::bail!("CmpI expects integer registers");
    };
    assign_reg(frame_raw, regs, dst as usize, Val::Bool(kind.eval(*lhs, *rhs)));
    Ok(())
}

#[inline]
pub(super) fn run_cmp_i_jmp_false(
    regs: &[Val],
    pc: usize,
    ofs: i16,
    a: u16,
    b: u16,
    kind: IntCmpKind,
) -> Result<usize> {
    let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) else {
        anyhow::bail!("CmpI expects integer registers");
    };
    Ok(branch_after_cmp(pc, ofs, kind.eval(*lhs, *rhs)))
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_ne(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if quickening::execute_cmp_ne_site(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }
    let result = rk_read(regs, consts, a) != rk_read(regs, consts, b);
    assign_reg(frame_raw, regs, dst as usize, Val::Bool(result));
    Ok(())
}

#[inline]
pub(super) fn run_cmp_ne_jmp_false(regs: &[Val], consts: &[Val], pc: usize, ofs: i16, a: u16, b: u16) -> usize {
    let result = rk_read(regs, consts, a) != rk_read(regs, consts, b);
    branch_after_cmp(pc, ofs, result)
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn run_cmp_numeric(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
    quicken: fn(&mut Vec<QuickeningSite>, usize, &mut [Val], &[Val], u16, u16, u16) -> Result<bool>,
    int_cmp: impl FnOnce(i64, i64) -> bool,
    float_cmp: impl FnOnce(f64, f64) -> bool,
    fallback: BinOp,
) -> Result<()> {
    if quicken(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }
    if !crate::vm::Vm::cmp2_try_numeric(frame_raw, regs, consts, dst, a, b, int_cmp, float_cmp) {
        let result = fallback.cmp(rk_read(regs, consts, a), rk_read(regs, consts, b))?;
        assign_reg(frame_raw, regs, dst as usize, Val::Bool(result));
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn run_cmp_numeric_jmp_false(
    regs: &[Val],
    consts: &[Val],
    pc: usize,
    ofs: i16,
    a: u16,
    b: u16,
    int_cmp: impl FnOnce(i64, i64) -> bool,
    float_cmp: impl FnOnce(f64, f64) -> bool,
    fallback: BinOp,
) -> Result<usize> {
    let result = match (rk_read(regs, consts, a), rk_read(regs, consts, b)) {
        (Val::Int(x), Val::Int(y)) => int_cmp(*x, *y),
        (Val::Float(x), Val::Float(y)) => float_cmp(*x, *y),
        (Val::Int(x), Val::Float(y)) => float_cmp(*x as f64, *y),
        (Val::Float(x), Val::Int(y)) => float_cmp(*x, *y as f64),
        _ => fallback.cmp(rk_read(regs, consts, a), rk_read(regs, consts, b))?,
    };
    Ok(branch_after_cmp(pc, ofs, result))
}

#[inline]
fn branch_after_cmp(pc: usize, ofs: i16, result: bool) -> usize {
    if result {
        pc + 2
    } else {
        ((pc as isize) + 1 + (ofs as isize)) as usize
    }
}

#[inline]
pub(super) fn run_cmp_eq_imm_jmp_false(
    regs: &[Val],
    consts: &[Val],
    pc: usize,
    ofs: i16,
    a: u16,
    imm: i16,
) -> Result<usize> {
    let imm_i64 = imm as i64;
    let result = match rk_read(regs, consts, a) {
        Val::Int(x) => *x == imm_i64,
        Val::Float(x) => *x == imm_i64 as f64,
        other => BinOp::Eq.cmp(other, &Val::Int(imm_i64))?,
    };
    Ok(branch_after_cmp(pc, ofs, result))
}

#[inline]
pub(super) fn run_cmp_ne_imm_jmp_false(
    regs: &[Val],
    consts: &[Val],
    pc: usize,
    ofs: i16,
    a: u16,
    imm: i16,
) -> Result<usize> {
    let imm_i64 = imm as i64;
    let result = match rk_read(regs, consts, a) {
        Val::Int(x) => *x != imm_i64,
        Val::Float(x) => *x != imm_i64 as f64,
        other => BinOp::Ne.cmp(other, &Val::Int(imm_i64))?,
    };
    Ok(branch_after_cmp(pc, ofs, result))
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_ord_imm_jmp_false(
    regs: &[Val],
    consts: &[Val],
    pc: usize,
    ofs: i16,
    a: u16,
    imm: i16,
    int_cmp: impl FnOnce(i64, i64) -> bool,
    float_cmp: impl FnOnce(f64, f64) -> bool,
    fallback: BinOp,
) -> Result<usize> {
    let imm_i64 = imm as i64;
    let result = match rk_read(regs, consts, a) {
        Val::Int(x) => int_cmp(*x, imm_i64),
        Val::Float(x) => float_cmp(*x, imm_i64 as f64),
        other => fallback.cmp(other, &Val::Int(imm_i64))?,
    };
    Ok(branch_after_cmp(pc, ofs, result))
}

pub(super) fn run_cmp_imm_or_branch(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    code: &[Op],
    pc: usize,
    op: &Op,
) -> Result<usize> {
    let (dst, a, imm, kind) = match *op {
        Op::CmpEqImm(dst, a, imm) => (dst, a, imm, IntCmpKind::Eq),
        Op::CmpNeImm(dst, a, imm) => (dst, a, imm, IntCmpKind::Ne),
        Op::CmpLtImm(dst, a, imm) => (dst, a, imm, IntCmpKind::Lt),
        Op::CmpLeImm(dst, a, imm) => (dst, a, imm, IntCmpKind::Le),
        Op::CmpGtImm(dst, a, imm) => (dst, a, imm, IntCmpKind::Gt),
        Op::CmpGeImm(dst, a, imm) => (dst, a, imm, IntCmpKind::Ge),
        _ => return Ok(pc + 1),
    };
    if let Some(Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs)) = code.get(pc + 1)
        && *r == dst
    {
        return match kind {
            IntCmpKind::Eq => run_cmp_eq_imm_jmp_false(regs, consts, pc, *ofs, a, imm),
            IntCmpKind::Ne => run_cmp_ne_imm_jmp_false(regs, consts, pc, *ofs, a, imm),
            IntCmpKind::Lt => {
                run_cmp_ord_imm_jmp_false(regs, consts, pc, *ofs, a, imm, |x, y| x < y, |x, y| x < y, BinOp::Lt)
            }
            IntCmpKind::Le => {
                run_cmp_ord_imm_jmp_false(regs, consts, pc, *ofs, a, imm, |x, y| x <= y, |x, y| x <= y, BinOp::Le)
            }
            IntCmpKind::Gt => {
                run_cmp_ord_imm_jmp_false(regs, consts, pc, *ofs, a, imm, |x, y| x > y, |x, y| x > y, BinOp::Gt)
            }
            IntCmpKind::Ge => {
                run_cmp_ord_imm_jmp_false(regs, consts, pc, *ofs, a, imm, |x, y| x >= y, |x, y| x >= y, BinOp::Ge)
            }
        };
    }

    match kind {
        IntCmpKind::Eq => cmp_eq_imm(frame_raw, regs, consts, dst, a, imm, BinOp::Eq)?,
        IntCmpKind::Ne => cmp_ne_imm(frame_raw, regs, consts, dst, a, imm, BinOp::Ne)?,
        IntCmpKind::Lt => cmp_ord_imm(
            frame_raw,
            regs,
            consts,
            dst,
            a,
            imm,
            |x, y| x < y,
            |x, y| x < y,
            BinOp::Lt,
        )?,
        IntCmpKind::Le => cmp_ord_imm(
            frame_raw,
            regs,
            consts,
            dst,
            a,
            imm,
            |x, y| x <= y,
            |x, y| x <= y,
            BinOp::Le,
        )?,
        IntCmpKind::Gt => cmp_ord_imm(
            frame_raw,
            regs,
            consts,
            dst,
            a,
            imm,
            |x, y| x > y,
            |x, y| x > y,
            BinOp::Gt,
        )?,
        IntCmpKind::Ge => cmp_ord_imm(
            frame_raw,
            regs,
            consts,
            dst,
            a,
            imm,
            |x, y| x >= y,
            |x, y| x >= y,
            BinOp::Ge,
        )?,
    }
    Ok(pc + 1)
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_lt(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    run_cmp_numeric(
        frame_raw,
        regs,
        consts,
        quickening,
        pc,
        dst,
        a,
        b,
        quickening::execute_cmp_lt_site,
        |x, y| x < y,
        |x, y| x < y,
        BinOp::Lt,
    )
}

#[inline]
pub(super) fn run_cmp_lt_jmp_false(regs: &[Val], consts: &[Val], pc: usize, ofs: i16, a: u16, b: u16) -> Result<usize> {
    run_cmp_numeric_jmp_false(regs, consts, pc, ofs, a, b, |x, y| x < y, |x, y| x < y, BinOp::Lt)
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_le(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    run_cmp_numeric(
        frame_raw,
        regs,
        consts,
        quickening,
        pc,
        dst,
        a,
        b,
        quickening::execute_cmp_le_site,
        |x, y| x <= y,
        |x, y| x <= y,
        BinOp::Le,
    )
}

#[inline]
pub(super) fn run_cmp_le_jmp_false(regs: &[Val], consts: &[Val], pc: usize, ofs: i16, a: u16, b: u16) -> Result<usize> {
    run_cmp_numeric_jmp_false(regs, consts, pc, ofs, a, b, |x, y| x <= y, |x, y| x <= y, BinOp::Le)
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_gt(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    run_cmp_numeric(
        frame_raw,
        regs,
        consts,
        quickening,
        pc,
        dst,
        a,
        b,
        quickening::execute_cmp_gt_site,
        |x, y| x > y,
        |x, y| x > y,
        BinOp::Gt,
    )
}

#[inline]
pub(super) fn run_cmp_gt_jmp_false(regs: &[Val], consts: &[Val], pc: usize, ofs: i16, a: u16, b: u16) -> Result<usize> {
    run_cmp_numeric_jmp_false(regs, consts, pc, ofs, a, b, |x, y| x > y, |x, y| x > y, BinOp::Gt)
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_cmp_ge(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    run_cmp_numeric(
        frame_raw,
        regs,
        consts,
        quickening,
        pc,
        dst,
        a,
        b,
        quickening::execute_cmp_ge_site,
        |x, y| x >= y,
        |x, y| x >= y,
        BinOp::Ge,
    )
}

#[inline]
pub(super) fn run_cmp_ge_jmp_false(regs: &[Val], consts: &[Val], pc: usize, ofs: i16, a: u16, b: u16) -> Result<usize> {
    run_cmp_numeric_jmp_false(regs, consts, pc, ofs, a, b, |x, y| x >= y, |x, y| x >= y, BinOp::Ge)
}

#[inline]
pub(super) fn run_in(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    let result = BinOp::In.cmp(rk_read(regs, consts, a), rk_read(regs, consts, b))?;
    assign_reg(frame_raw, regs, dst as usize, Val::Bool(result));
    Ok(())
}
