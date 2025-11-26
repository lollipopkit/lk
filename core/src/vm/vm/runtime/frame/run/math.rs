use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::bytecode::{rk_index, rk_is_const};

use super::helpers::assign_reg_slice;

#[inline]
pub(super) fn rk_read<'a>(regs: &'a [Val], consts: &'a [Val], rk: u16) -> &'a Val {
    let idx = rk_index(rk) as usize;
    if rk_is_const(rk) { &consts[idx] } else { &regs[idx] }
}

#[inline]
pub(super) fn read_int_pair(regs: &[Val], consts: &[Val], a: u16, b: u16) -> Option<(i64, i64)> {
    match (rk_read(regs, consts, a), rk_read(regs, consts, b)) {
        (Val::Int(x), Val::Int(y)) => Some((*x, *y)),
        _ => None,
    }
}

#[inline]
pub(super) fn read_float_pair(regs: &[Val], consts: &[Val], a: u16, b: u16) -> Option<(f64, f64)> {
    fn to_f64(v: &Val) -> Option<f64> {
        match v {
            Val::Float(f) => Some(*f),
            Val::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    Some((to_f64(rk_read(regs, consts, a))?, to_f64(rk_read(regs, consts, b))?))
}

#[inline]
pub(super) fn int_binop<F>(
    frame_raw: *mut super::FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    op: F,
    fallback: BinOp,
) -> Result<()>
where
    F: FnOnce(i64, i64) -> i64,
{
    let dst_idx = dst as usize;
    if let Some((x, y)) = read_int_pair(regs, consts, a, b) {
        assign_reg_slice(frame_raw, regs, dst_idx, Val::Int(op(x, y)));
        Ok(())
    } else {
        let lhs = rk_read(regs, consts, a);
        let rhs = rk_read(regs, consts, b);
        tracing::debug!(
            target: "lkr::vm::slowpath",
            op = ?fallback,
            lhs = lhs.type_name(),
            rhs = rhs.type_name(),
            "int_binop fallback"
        );
        let value = fallback.eval_vals(lhs, rhs)?;
        assign_reg_slice(frame_raw, regs, dst_idx, value);
        Ok(())
    }
}

#[inline]
pub(super) fn float_binop<F>(
    frame_raw: *mut super::FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    op: F,
    fallback: BinOp,
) -> Result<()>
where
    F: FnOnce(f64, f64) -> f64,
{
    let dst_idx = dst as usize;
    if let Some((x, y)) = read_float_pair(regs, consts, a, b) {
        assign_reg_slice(frame_raw, regs, dst_idx, Val::Float(op(x, y)));
        Ok(())
    } else {
        let lhs = rk_read(regs, consts, a);
        let rhs = rk_read(regs, consts, b);
        tracing::debug!(
            target: "lkr::vm::slowpath",
            op = ?fallback,
            lhs = lhs.type_name(),
            rhs = rhs.type_name(),
            "float_binop fallback"
        );
        let value = fallback.eval_vals(lhs, rhs)?;
        assign_reg_slice(frame_raw, regs, dst_idx, value);
        Ok(())
    }
}

#[inline]
pub(super) fn int_binop_imm<F>(
    frame_raw: *mut super::FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    imm: i16,
    op: F,
    fallback: BinOp,
) -> Result<()>
where
    F: FnOnce(i64, i64) -> i64,
{
    let aval = rk_read(regs, consts, a);
    let dst_idx = dst as usize;
    if let Val::Int(x) = aval {
        assign_reg_slice(frame_raw, regs, dst_idx, Val::Int(op(*x, imm as i64)));
        return Ok(());
    }

    let imm_val = Val::Int(imm as i64);
    tracing::debug!(
        target: "lkr::vm::slowpath",
        op = ?fallback,
        lhs = aval.type_name(),
        rhs = "ImmediateInt",
        imm,
        "int_binop_imm fallback"
    );
    let value = fallback.eval_vals(aval, &imm_val)?;
    assign_reg_slice(frame_raw, regs, dst_idx, value);
    Ok(())
}

#[inline]
pub(super) fn cmp_ord_imm(
    frame_raw: *mut super::FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    imm: i16,
    int_cmp: impl FnOnce(i64, i64) -> bool,
    float_cmp: impl FnOnce(f64, f64) -> bool,
    fallback: BinOp,
) -> Result<()> {
    let aval = rk_read(regs, consts, a);
    let imm_i64 = imm as i64;
    let dst_idx = dst as usize;
    match aval {
        Val::Int(x) => {
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(int_cmp(*x, imm_i64)));
            Ok(())
        }
        Val::Float(x) => {
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(float_cmp(*x, imm_i64 as f64)));
            Ok(())
        }
        _ => {
            let imm_val = Val::Int(imm_i64);
            tracing::debug!(
                target: "lkr::vm::slowpath",
                op = ?fallback,
                lhs = aval.type_name(),
                rhs = "ImmediateInt",
                imm,
                "cmp_ord_imm fallback"
            );
            let res = fallback.cmp(aval, &imm_val)?;
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(res));
            Ok(())
        }
    }
}

#[inline]
pub(super) fn cmp_eq_imm(
    frame_raw: *mut super::FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    imm: i16,
    fallback: BinOp,
) -> Result<()> {
    let aval = rk_read(regs, consts, a);
    let imm_i64 = imm as i64;
    let dst_idx = dst as usize;
    match aval {
        Val::Int(x) => {
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(*x == imm_i64));
            Ok(())
        }
        Val::Float(x) => {
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(*x == imm_i64 as f64));
            Ok(())
        }
        _ => {
            let imm_val = Val::Int(imm_i64);
            tracing::debug!(
                target: "lkr::vm::slowpath",
                op = ?fallback,
                lhs = aval.type_name(),
                rhs = "ImmediateInt",
                imm,
                "cmp_eq_imm fallback"
            );
            let res = fallback.cmp(aval, &imm_val)?;
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(res));
            Ok(())
        }
    }
}

#[inline]
pub(super) fn cmp_ne_imm(
    frame_raw: *mut super::FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    imm: i16,
    fallback: BinOp,
) -> Result<()> {
    let aval = rk_read(regs, consts, a);
    let imm_i64 = imm as i64;
    let dst_idx = dst as usize;
    match aval {
        Val::Int(x) => {
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(*x != imm_i64));
            Ok(())
        }
        Val::Float(x) => {
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(*x != imm_i64 as f64));
            Ok(())
        }
        _ => {
            let imm_val = Val::Int(imm_i64);
            tracing::debug!(
                target: "lkr::vm::slowpath",
                op = ?fallback,
                lhs = aval.type_name(),
                rhs = "ImmediateInt",
                imm,
                "cmp_ne_imm fallback"
            );
            let res = fallback.cmp(aval, &imm_val)?;
            assign_reg_slice(frame_raw, regs, dst_idx, Val::Bool(res));
            Ok(())
        }
    }
}
