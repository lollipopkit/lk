use super::super::helpers::{assign_reg_from_reg_with_metrics, assign_reg_with_metrics};
use super::super::math::{cmp_eq_imm, cmp_ne_imm, cmp_ord_imm, int_binop, int_binop_imm, rk_read};
use super::*;
use crate::op::BinOp;

#[inline(always)]
pub(super) fn exec_cmp_hot(
    regs: &mut [Val],
    func: &Function,
    op: PackedCmpOp,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    match op {
        PackedCmpOp::Eq => assign_reg_with_metrics(
            regs,
            dst as usize,
            Val::Bool(rk_read(regs, &func.consts, a) == rk_read(regs, &func.consts, b)),
            collect_metrics,
        ),
        PackedCmpOp::Ne => assign_reg_with_metrics(
            regs,
            dst as usize,
            Val::Bool(rk_read(regs, &func.consts, a) != rk_read(regs, &func.consts, b)),
            collect_metrics,
        ),
        PackedCmpOp::Lt => {
            if !Vm::cmp2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                |x, y| x < y,
                |x, y| x < y,
                collect_metrics,
            ) {
                let res = BinOp::Lt.cmp(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
        }
        PackedCmpOp::Le => {
            if !Vm::cmp2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                |x, y| x <= y,
                |x, y| x <= y,
                collect_metrics,
            ) {
                let res = BinOp::Le.cmp(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
        }
        PackedCmpOp::Gt => {
            if !Vm::cmp2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                |x, y| x > y,
                |x, y| x > y,
                collect_metrics,
            ) {
                let res = BinOp::Gt.cmp(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
        }
        PackedCmpOp::Ge => {
            if !Vm::cmp2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                |x, y| x >= y,
                |x, y| x >= y,
                collect_metrics,
            ) {
                let res = BinOp::Ge.cmp(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
        }
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_cmp_int(
    regs: &mut [Val],
    op: PackedCmpOp,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    let cmp = cmp_int_regs(regs, op, a, b)?;
    assign_reg_with_metrics(regs, dst as usize, Val::Bool(cmp), collect_metrics);
    Ok(())
}

#[inline(always)]
pub(super) fn exec_cmp_int_jmp(
    regs: &[Val],
    op: PackedCmpOp,
    a: u16,
    b: u16,
    pc: usize,
    ofs: i16,
) -> Result<Option<usize>> {
    Ok((!cmp_int_regs(regs, op, a, b)?).then_some(((pc as isize) + (ofs as isize)) as usize))
}

#[inline(always)]
pub(super) fn exec_cmove_int(
    regs: &mut [Val],
    op: PackedCmpOp,
    dst: u16,
    src: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if cmp_int_regs(regs, op, a, b)? {
        let Val::Int(value) = regs[src as usize] else {
            anyhow::bail!("CMoveInt expects integer registers");
        };
        assign_reg_with_metrics(regs, dst as usize, Val::Int(value), collect_metrics);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn exec_cmp_int_move(
    regs: &mut [Val],
    op: PackedCmpOp,
    a: u16,
    b: u16,
    dst: u16,
    src: u16,
    pc: usize,
    ofs: i16,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    if !cmp_int_regs(regs, op, a, b)? {
        Ok(Some(((pc as isize) + (ofs as isize)) as usize))
    } else {
        assign_reg_from_reg_with_metrics(regs, dst as usize, src as usize, collect_metrics);
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn exec_cmp_int_add_int_imm(
    regs: &mut [Val],
    func: &Function,
    op: PackedCmpOp,
    a: u16,
    b: u16,
    dst: u16,
    src: u16,
    imm: i16,
    pc: usize,
    ofs: i16,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    if !cmp_int_regs(regs, op, a, b)? {
        Ok(Some(((pc as isize) + (ofs as isize)) as usize))
    } else {
        let dst_idx = dst as usize;
        let src_idx = src as usize;
        if let Val::Int(value) = regs[src_idx] {
            assign_reg_with_metrics(regs, dst_idx, Val::Int(value + imm as i64), collect_metrics);
        } else {
            int_binop_imm(
                regs,
                &func.consts,
                dst,
                src,
                imm,
                |x, y| x + y,
                BinOp::Add,
                collect_metrics,
            )?;
        }
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn exec_cmp_int_sub_access_sub(
    regs: &mut [Val],
    func: &Function,
    access_ic: &mut [Option<AccessIc>],
    op: PackedCmpOp,
    a: u16,
    b: u16,
    first_dst: u16,
    first_a: u16,
    first_b: u16,
    access_pc: usize,
    access_dst: u16,
    access_base: u16,
    access_field: u16,
    final_dst: u16,
    final_a: u16,
    final_b: u16,
    pc: usize,
    ofs: i16,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    if !cmp_int_regs(regs, op, a, b)? {
        Ok(Some(((pc as isize) + (ofs as isize)) as usize))
    } else {
        exec_sub_access_sub_hot(
            regs,
            func,
            access_ic,
            access_pc,
            first_dst,
            first_a,
            first_b,
            access_dst,
            access_base,
            access_field,
            final_dst,
            final_a,
            final_b,
            collect_metrics,
        )?;
        Ok(None)
    }
}

#[inline(always)]
pub(super) fn exec_cmp_jmp(
    regs: &[Val],
    func: &Function,
    op: PackedCmpOp,
    a: u16,
    b: u16,
    pc: usize,
    ofs: i16,
) -> Result<Option<usize>> {
    let lhs = rk_read(regs, &func.consts, a);
    let rhs = rk_read(regs, &func.consts, b);
    let cmp = match (op, lhs, rhs) {
        (PackedCmpOp::Eq, left, right) => left == right,
        (PackedCmpOp::Ne, left, right) => left != right,
        (PackedCmpOp::Lt, Val::Int(left), Val::Int(right)) => left < right,
        (PackedCmpOp::Le, Val::Int(left), Val::Int(right)) => left <= right,
        (PackedCmpOp::Gt, Val::Int(left), Val::Int(right)) => left > right,
        (PackedCmpOp::Ge, Val::Int(left), Val::Int(right)) => left >= right,
        (PackedCmpOp::Lt, _, _) => BinOp::Lt.cmp(lhs, rhs)?,
        (PackedCmpOp::Le, _, _) => BinOp::Le.cmp(lhs, rhs)?,
        (PackedCmpOp::Gt, _, _) => BinOp::Gt.cmp(lhs, rhs)?,
        (PackedCmpOp::Ge, _, _) => BinOp::Ge.cmp(lhs, rhs)?,
    };
    Ok((!cmp).then_some(((pc as isize) + (ofs as isize)) as usize))
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn exec_cmp_imm(
    regs: &mut [Val],
    func: &Function,
    op: PackedCmpImmOp,
    dst: u16,
    src: u16,
    imm: i16,
    collect_metrics: bool,
) -> Result<()> {
    let dst_idx = dst as usize;
    let src_idx = src as usize;
    let imm_i64 = imm as i64;
    match (&regs[src_idx], op) {
        (Val::Int(x), PackedCmpImmOp::Eq) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Bool(*x == imm_i64), collect_metrics)
        }
        (Val::Int(x), PackedCmpImmOp::Ne) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Bool(*x != imm_i64), collect_metrics)
        }
        (Val::Int(x), PackedCmpImmOp::Lt) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Bool(*x < imm_i64), collect_metrics)
        }
        (Val::Int(x), PackedCmpImmOp::Le) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Bool(*x <= imm_i64), collect_metrics)
        }
        (Val::Int(x), PackedCmpImmOp::Gt) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Bool(*x > imm_i64), collect_metrics)
        }
        (Val::Int(x), PackedCmpImmOp::Ge) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Bool(*x >= imm_i64), collect_metrics)
        }
        _ => match op {
            PackedCmpImmOp::Eq => cmp_eq_imm(regs, &func.consts, dst, src, imm, BinOp::Eq, collect_metrics)?,
            PackedCmpImmOp::Ne => cmp_ne_imm(regs, &func.consts, dst, src, imm, BinOp::Ne, collect_metrics)?,
            PackedCmpImmOp::Lt => cmp_ord_imm(
                regs,
                &func.consts,
                dst,
                src,
                imm,
                |x, y| x < y,
                |x, y| x < y,
                BinOp::Lt,
                collect_metrics,
            )?,
            PackedCmpImmOp::Le => cmp_ord_imm(
                regs,
                &func.consts,
                dst,
                src,
                imm,
                |x, y| x <= y,
                |x, y| x <= y,
                BinOp::Le,
                collect_metrics,
            )?,
            PackedCmpImmOp::Gt => cmp_ord_imm(
                regs,
                &func.consts,
                dst,
                src,
                imm,
                |x, y| x > y,
                |x, y| x > y,
                BinOp::Gt,
                collect_metrics,
            )?,
            PackedCmpImmOp::Ge => cmp_ord_imm(
                regs,
                &func.consts,
                dst,
                src,
                imm,
                |x, y| x >= y,
                |x, y| x >= y,
                BinOp::Ge,
                collect_metrics,
            )?,
        },
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_cmp_imm_jmp(
    regs: &[Val],
    func: &Function,
    op: PackedCmpImmOp,
    src: u16,
    imm: i16,
    pc: usize,
    ofs: i16,
) -> Result<Option<usize>> {
    Ok((!cmp_imm_value(regs, func, op, src, imm)?).then_some(((pc as isize) + (ofs as isize)) as usize))
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn exec_cmp_imm_mul_int_add_int(
    regs: &mut [Val],
    func: &Function,
    op: PackedCmpImmOp,
    src: u16,
    imm: i16,
    mul_dst: u16,
    mul_a: u16,
    mul_b: u16,
    add_dst: u16,
    add_a: u16,
    add_b: u16,
    pc: usize,
    ofs: i16,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    if !cmp_imm_value(regs, func, op, src, imm)? {
        Ok(Some(((pc as isize) + (ofs as isize)) as usize))
    } else {
        int_binop(
            regs,
            &func.consts,
            mul_dst,
            mul_a,
            mul_b,
            i64::wrapping_mul,
            BinOp::Mul,
            collect_metrics,
        )?;
        int_binop(
            regs,
            &func.consts,
            add_dst,
            add_a,
            add_b,
            i64::wrapping_add,
            BinOp::Add,
            collect_metrics,
        )?;
        Ok(None)
    }
}

#[inline(always)]
pub(super) fn exec_cmp_lt_imm_jmp(regs: &[Val], r: u16, imm: i16, pc: usize, ofs: i16) -> Option<usize> {
    let skip = match &regs[r as usize] {
        Val::Int(x) => *x >= (imm as i64),
        _ => true,
    };
    skip.then_some(((pc as isize) + (ofs as isize)) as usize)
}

#[inline(always)]
pub(super) fn exec_cmp_le_imm_jmp(regs: &[Val], r: u16, imm: i16, pc: usize, ofs: i16) -> Option<usize> {
    let skip = match &regs[r as usize] {
        Val::Int(x) => *x > (imm as i64),
        _ => true,
    };
    skip.then_some(((pc as isize) + (ofs as isize)) as usize)
}

#[inline(always)]
fn cmp_int_regs(regs: &[Val], op: PackedCmpOp, a: u16, b: u16) -> Result<bool> {
    let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) else {
        return Err(anyhow!("CmpI expects integer registers"));
    };
    Ok(match op {
        PackedCmpOp::Eq => lhs == rhs,
        PackedCmpOp::Ne => lhs != rhs,
        PackedCmpOp::Lt => lhs < rhs,
        PackedCmpOp::Le => lhs <= rhs,
        PackedCmpOp::Gt => lhs > rhs,
        PackedCmpOp::Ge => lhs >= rhs,
    })
}

#[inline(always)]
fn cmp_imm_value(regs: &[Val], func: &Function, op: PackedCmpImmOp, src: u16, imm: i16) -> Result<bool> {
    let src_idx = src as usize;
    let imm_i64 = imm as i64;
    Ok(match (regs.get(src_idx), op) {
        (Some(Val::Int(x)), PackedCmpImmOp::Eq) => *x == imm_i64,
        (Some(Val::Int(x)), PackedCmpImmOp::Ne) => *x != imm_i64,
        (Some(Val::Int(x)), PackedCmpImmOp::Lt) => *x < imm_i64,
        (Some(Val::Int(x)), PackedCmpImmOp::Le) => *x <= imm_i64,
        (Some(Val::Int(x)), PackedCmpImmOp::Gt) => *x > imm_i64,
        (Some(Val::Int(x)), PackedCmpImmOp::Ge) => *x >= imm_i64,
        _ => {
            let imm_val = Val::Int(imm_i64);
            match op {
                PackedCmpImmOp::Eq => rk_read(regs, &func.consts, src) == &imm_val,
                PackedCmpImmOp::Ne => rk_read(regs, &func.consts, src) != &imm_val,
                PackedCmpImmOp::Lt => BinOp::Lt.cmp(rk_read(regs, &func.consts, src), &imm_val)?,
                PackedCmpImmOp::Le => BinOp::Le.cmp(rk_read(regs, &func.consts, src), &imm_val)?,
                PackedCmpImmOp::Gt => BinOp::Gt.cmp(rk_read(regs, &func.consts, src), &imm_val)?,
                PackedCmpImmOp::Ge => BinOp::Ge.cmp(rk_read(regs, &func.consts, src), &imm_val)?,
            }
        }
    })
}
