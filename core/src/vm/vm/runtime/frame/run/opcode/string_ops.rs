use crate::val::Val;
use crate::vm::bytecode::Op;
use crate::vm::vm::frame::FrameState;

use super::super::helpers::assign_reg;
use super::super::math::rk_read;

#[inline]
pub(super) fn run_to_str(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    consts: &[Val],
    code: &[Op],
    pc: usize,
    dst: u16,
    src: u16,
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
            assign_reg(frame_raw, regs, *out as usize, value);
            return true;
        }
    }

    let value = Val::to_str_value(&regs[src as usize]);
    assign_reg(frame_raw, regs, dst as usize, value);
    false
}

#[inline]
pub(super) fn run_starts_with_k(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    src: u16,
    key_index: u16,
) {
    let prefix = consts[key_index as usize].as_str().unwrap_or("");
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Bool(value.as_str().starts_with(prefix)),
        Val::Str(value) => Val::Bool(value.as_str().starts_with(prefix)),
        _ => Val::Bool(false),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(super) fn run_contains_k(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    src: u16,
    key_index: u16,
) {
    let needle = consts[key_index as usize].as_str().unwrap_or("");
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Bool(value.as_str().contains(needle)),
        Val::Str(value) => Val::Bool(value.as_str().contains(needle)),
        _ => Val::Bool(false),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}
