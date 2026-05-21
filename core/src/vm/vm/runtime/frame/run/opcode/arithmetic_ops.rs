use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::vm::quickening::{self, QuickeningSite};

use super::super::helpers::assign_reg_with_metrics;
use super::super::math::{float_binop, int_binop, int_binop_imm, rk_read};

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_add(
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if quickening::execute_add_site(quickening, pc, regs, consts, dst, a, b, collect_metrics)? {
        return Ok(());
    }

    let a_val = rk_read(regs, consts, a);
    let b_val = rk_read(regs, consts, b);
    if let Some(a_str) = a_val.as_str()
        && let Some(out) = Val::concat_str_add_rhs(a_str, b_val)
    {
        assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    } else if let Some(b_str) = b_val.as_str()
        && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
    {
        assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    } else if !crate::vm::Vm::arith2_try_numeric(
        regs,
        consts,
        dst,
        a,
        b,
        "add",
        |x, y| x + y,
        |x, y| x + y,
        collect_metrics,
    ) {
        quickening::fallback_add(regs, consts, dst, a, b, collect_metrics)?;
    }
    Ok(())
}

#[inline]
pub(super) fn run_str_concat_known_cap(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    let a_val = rk_read(regs, consts, a);
    let b_val = rk_read(regs, consts, b);
    let out = match (a_val.as_str(), b_val.as_str()) {
        (Some(a_str), Some(b_str)) => Val::concat_strings(a_str, b_str),
        _ => BinOp::Add.eval_vals_with_metrics(a_val, b_val, collect_metrics)?,
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    Ok(())
}

#[inline]
pub(super) fn run_str_concat_to_str(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    src: u16,
    collect_metrics: bool,
) -> Result<()> {
    let lhs_val = rk_read(regs, consts, lhs);
    let out = if let Some(lhs_str) = lhs_val.as_str()
        && let Some(value) = Val::concat_str_tostr_rhs(lhs_str, &regs[src as usize])
    {
        value
    } else {
        let rhs = Val::to_str_value(&regs[src as usize]);
        BinOp::Add.eval_vals_with_metrics(lhs_val, &rhs, collect_metrics)?
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_sub(
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if quickening::execute_sub_site(quickening, pc, regs, consts, dst, a, b, collect_metrics)? {
        return Ok(());
    }
    if !crate::vm::Vm::arith2_try_numeric(
        regs,
        consts,
        dst,
        a,
        b,
        "sub",
        |x, y| x - y,
        |x, y| x - y,
        collect_metrics,
    ) {
        let out =
            BinOp::Sub.eval_vals_with_metrics(rk_read(regs, consts, a), rk_read(regs, consts, b), collect_metrics)?;
        assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mul(
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if quickening::execute_mul_site(quickening, pc, regs, consts, dst, a, b, collect_metrics)? {
        return Ok(());
    }
    if !crate::vm::Vm::arith2_try_numeric(
        regs,
        consts,
        dst,
        a,
        b,
        "mul",
        |x, y| x * y,
        |x, y| x * y,
        collect_metrics,
    ) {
        let out =
            BinOp::Mul.eval_vals_with_metrics(rk_read(regs, consts, a), rk_read(regs, consts, b), collect_metrics)?;
        assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    }
    Ok(())
}

#[inline]
pub(super) fn run_div(regs: &mut [Val], consts: &[Val], dst: u16, a: u16, b: u16, collect_metrics: bool) -> Result<()> {
    let ar = rk_read(regs, consts, a);
    let br = rk_read(regs, consts, b);
    let dst_idx = dst as usize;
    match (ar, br) {
        (Val::Int(x), Val::Int(y)) => {
            let res = *x as f64 / *y as f64;
            if res.fract() == 0.0 {
                assign_reg_with_metrics(regs, dst_idx, Val::Int(res as i64), collect_metrics);
            } else {
                assign_reg_with_metrics(regs, dst_idx, Val::Float(res), collect_metrics);
            }
        }
        (Val::Float(x), Val::Float(y)) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Float(x / y), collect_metrics);
        }
        (Val::Int(x), Val::Float(y)) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Float(*x as f64 / y), collect_metrics);
        }
        (Val::Float(x), Val::Int(y)) => {
            assign_reg_with_metrics(regs, dst_idx, Val::Float(x / *y as f64), collect_metrics);
        }
        _ => {
            let out = BinOp::Div.eval_vals_with_metrics(ar, br, collect_metrics)?;
            assign_reg_with_metrics(regs, dst_idx, out, collect_metrics);
        }
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_mod(
    regs: &mut [Val],
    consts: &[Val],
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if quickening::execute_mod_site(quickening, pc, regs, consts, dst, a, b, collect_metrics)? {
        return Ok(());
    }
    match (rk_read(regs, consts, a), rk_read(regs, consts, b)) {
        (Val::Int(x), Val::Int(y)) => assign_reg_with_metrics(regs, dst as usize, Val::Int(x % y), collect_metrics),
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
            let out = BinOp::Mod.eval_vals_with_metrics(lhs, rhs, collect_metrics)?;
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
        }
    }
    Ok(())
}

#[inline]
pub(super) fn run_add_int(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    let a_val = &regs[a as usize];
    let b_val = &regs[b as usize];
    if let (Val::Str(a_str), Val::Str(b_str)) = (a_val, b_val) {
        assign_reg_with_metrics(regs, dst as usize, Val::concat_strings(a_str, b_str), collect_metrics);
    } else {
        int_binop(regs, consts, dst, a, b, |x, y| x + y, BinOp::Add, collect_metrics)?;
    }
    Ok(())
}

#[inline]
pub(super) fn run_add_float(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    float_binop(regs, consts, dst, a, b, |x, y| x + y, BinOp::Add, collect_metrics)
}

#[inline]
pub(super) fn run_add_int_imm(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    imm: i16,
    collect_metrics: bool,
) -> Result<()> {
    let src_idx = a as usize;
    let dst_idx = dst as usize;
    if let Val::Int(x) = regs[src_idx] {
        assign_reg_with_metrics(regs, dst_idx, Val::Int(x + imm as i64), collect_metrics);
    } else {
        int_binop_imm(regs, consts, dst, a, imm, |x, y| x + y, BinOp::Add, collect_metrics)?;
    }
    Ok(())
}

#[inline]
pub(super) fn run_sub_int(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    int_binop(regs, consts, dst, a, b, |x, y| x - y, BinOp::Sub, collect_metrics)
}

#[inline]
pub(super) fn run_sub_float(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    float_binop(regs, consts, dst, a, b, |x, y| x - y, BinOp::Sub, collect_metrics)
}

#[inline]
pub(super) fn run_mul_int(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    int_binop(regs, consts, dst, a, b, |x, y| x * y, BinOp::Mul, collect_metrics)
}

#[inline]
pub(super) fn run_mul_float(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    float_binop(regs, consts, dst, a, b, |x, y| x * y, BinOp::Mul, collect_metrics)
}

#[inline]
pub(super) fn run_div_float(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    float_binop(regs, consts, dst, a, b, |x, y| x / y, BinOp::Div, collect_metrics)
}

#[inline]
pub(super) fn run_mod_int(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    int_binop(regs, consts, dst, a, b, |x, y| x % y, BinOp::Mod, collect_metrics)
}

#[inline]
pub(super) fn run_mod_float(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    float_binop(regs, consts, dst, a, b, |x, y| x % y, BinOp::Mod, collect_metrics)
}
