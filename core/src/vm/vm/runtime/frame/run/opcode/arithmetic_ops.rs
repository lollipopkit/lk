use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::vm::quickening::{self, QuickeningSite};

use super::super::helpers::assign_reg;
use super::super::math::{float_binop, int_binop, int_binop_imm, rk_read};
use crate::vm::vm::frame::FrameState;

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_add(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if quickening::execute_add_site(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }

    let a_val = rk_read(regs, consts, a);
    let b_val = rk_read(regs, consts, b);
    if let Some(a_str) = a_val.as_str()
        && let Some(out) = Val::concat_str_add_rhs(a_str, b_val)
    {
        assign_reg(frame_raw, regs, dst as usize, out);
    } else if let Some(b_str) = b_val.as_str()
        && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
    {
        assign_reg(frame_raw, regs, dst as usize, out);
    } else if !crate::vm::Vm::arith2_try_numeric(frame_raw, regs, consts, dst, a, b, "add", |x, y| x + y, |x, y| x + y)
    {
        quickening::fallback_add(regs, consts, dst, a, b)?;
    }
    Ok(())
}

#[inline]
pub(super) fn run_str_concat_known_cap(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    let a_val = rk_read(regs, consts, a);
    let b_val = rk_read(regs, consts, b);
    let out = match (a_val.as_str(), b_val.as_str()) {
        (Some(a_str), Some(b_str)) => Val::concat_strings(a_str, b_str),
        _ => BinOp::Add.eval_vals(a_val, b_val)?,
    };
    assign_reg(frame_raw, regs, dst as usize, out);
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_sub(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if quickening::execute_sub_site(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }
    if !crate::vm::Vm::arith2_try_numeric(frame_raw, regs, consts, dst, a, b, "sub", |x, y| x - y, |x, y| x - y) {
        let out = BinOp::Sub.eval_vals(rk_read(regs, consts, a), rk_read(regs, consts, b))?;
        assign_reg(frame_raw, regs, dst as usize, out);
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mul(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if quickening::execute_mul_site(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }
    if !crate::vm::Vm::arith2_try_numeric(frame_raw, regs, consts, dst, a, b, "mul", |x, y| x * y, |x, y| x * y) {
        let out = BinOp::Mul.eval_vals(rk_read(regs, consts, a), rk_read(regs, consts, b))?;
        assign_reg(frame_raw, regs, dst as usize, out);
    }
    Ok(())
}

#[inline]
pub(super) fn run_div(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    let ar = rk_read(regs, consts, a);
    let br = rk_read(regs, consts, b);
    let dst_idx = dst as usize;
    match (ar, br) {
        (Val::Int(x), Val::Int(y)) => {
            let res = *x as f64 / *y as f64;
            if res.fract() == 0.0 {
                assign_reg(frame_raw, regs, dst_idx, Val::Int(res as i64));
            } else {
                assign_reg(frame_raw, regs, dst_idx, Val::Float(res));
            }
        }
        (Val::Float(x), Val::Float(y)) => {
            assign_reg(frame_raw, regs, dst_idx, Val::Float(x / y));
        }
        (Val::Int(x), Val::Float(y)) => {
            assign_reg(frame_raw, regs, dst_idx, Val::Float(*x as f64 / y));
        }
        (Val::Float(x), Val::Int(y)) => {
            assign_reg(frame_raw, regs, dst_idx, Val::Float(x / *y as f64));
        }
        _ => {
            let out = BinOp::Div.eval_vals(ar, br)?;
            assign_reg(frame_raw, regs, dst_idx, out);
        }
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mod(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if quickening::execute_mod_site(quickening, pc, regs, consts, dst, a, b)? {
        return Ok(());
    }
    match (rk_read(regs, consts, a), rk_read(regs, consts, b)) {
        (Val::Int(x), Val::Int(y)) => assign_reg(frame_raw, regs, dst as usize, Val::Int(x % y)),
        _ => {
            let lhs = rk_read(regs, consts, a);
            let rhs = rk_read(regs, consts, b);
            tracing::debug!(
                target: "lk::vm::slowpath",
                op = "mod",
                lhs = lhs.type_name(),
                rhs = rhs.type_name(),
                "mod fallback"
            );
            let out = BinOp::Mod.eval_vals(lhs, rhs)?;
            assign_reg(frame_raw, regs, dst as usize, out);
        }
    }
    Ok(())
}

#[inline]
pub(super) fn run_add_int(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    let a_val = &regs[a as usize];
    let b_val = &regs[b as usize];
    if let (Val::Str(a_str), Val::Str(b_str)) = (a_val, b_val) {
        assign_reg(frame_raw, regs, dst as usize, Val::concat_strings(a_str, b_str));
    } else {
        int_binop(frame_raw, regs, consts, dst, a, b, |x, y| x + y, BinOp::Add)?;
    }
    Ok(())
}

#[inline]
pub(super) fn run_add_float(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    float_binop(frame_raw, regs, consts, dst, a, b, |x, y| x + y, BinOp::Add)
}

#[inline]
pub(super) fn run_add_int_imm(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    imm: i16,
) -> Result<()> {
    let src_idx = a as usize;
    let dst_idx = dst as usize;
    if let Val::Int(x) = regs[src_idx] {
        assign_reg(frame_raw, regs, dst_idx, Val::Int(x + imm as i64));
    } else {
        int_binop_imm(frame_raw, regs, consts, dst, a, imm, |x, y| x + y, BinOp::Add)?;
    }
    Ok(())
}

#[inline]
pub(super) fn run_sub_int(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    int_binop(frame_raw, regs, consts, dst, a, b, |x, y| x - y, BinOp::Sub)
}

#[inline]
pub(super) fn run_sub_float(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    float_binop(frame_raw, regs, consts, dst, a, b, |x, y| x - y, BinOp::Sub)
}

#[inline]
pub(super) fn run_mul_int(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    int_binop(frame_raw, regs, consts, dst, a, b, |x, y| x * y, BinOp::Mul)
}

#[inline]
pub(super) fn run_mul_float(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    float_binop(frame_raw, regs, consts, dst, a, b, |x, y| x * y, BinOp::Mul)
}

#[inline]
pub(super) fn run_div_float(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    float_binop(frame_raw, regs, consts, dst, a, b, |x, y| x / y, BinOp::Div)
}

#[inline]
pub(super) fn run_mod_int(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    int_binop(frame_raw, regs, consts, dst, a, b, |x, y| x % y, BinOp::Mod)
}

#[inline]
pub(super) fn run_mod_float(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    float_binop(frame_raw, regs, consts, dst, a, b, |x, y| x % y, BinOp::Mod)
}
