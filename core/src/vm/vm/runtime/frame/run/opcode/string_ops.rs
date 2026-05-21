use crate::val::Val;
use crate::vm::bytecode::Op;

use super::super::helpers::assign_reg_with_metrics;
use super::super::math::rk_read;

#[inline]
pub(super) fn run_to_str(
    regs: &mut [Val],
    consts: &[Val],
    code: &[Op],
    pc: usize,
    dst: u16,
    src: u16,
    collect_metrics: bool,
) -> bool {
    if let Some(Op::Add(out, lhs, rhs)) = code.get(pc + 1)
        && *rhs == dst
    {
        let fused = {
            let lhs_val = rk_read(regs, consts, *lhs);
            lhs_val
                .as_str()
                .and_then(|lhs_str| Val::concat_str_tostr_rhs(lhs_str, &regs[src as usize]))
        };
        if let Some(value) = fused {
            assign_reg_with_metrics(regs, *out as usize, value, collect_metrics);
            return true;
        }
    }

    let value = Val::to_str_value(&regs[src as usize]);
    assign_reg_with_metrics(regs, dst as usize, value, collect_metrics);
    false
}

#[inline]
pub(super) fn run_starts_with_k(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    src: u16,
    key_index: u16,
    collect_metrics: bool,
) {
    let prefix = consts[key_index as usize].as_str().unwrap_or("");
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Bool(value.as_str().starts_with(prefix)),
        Val::Str(value) => Val::Bool(value.as_str().starts_with(prefix)),
        _ => Val::Bool(false),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline]
pub(super) fn run_contains_k(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    src: u16,
    key_index: u16,
    collect_metrics: bool,
) {
    let needle = consts[key_index as usize].as_str().unwrap_or("");
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Bool(value.as_str().contains(needle)),
        Val::Str(value) => Val::Bool(value.as_str().contains(needle)),
        _ => Val::Bool(false),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}
