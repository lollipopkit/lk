use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::vm::frame::FrameState;

use super::super::super::helpers::assign_reg;

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_fold_add(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    acc: u16,
    list: u16,
) -> Result<()> {
    let folded = if let Val::List(items) = &regs[list as usize] {
        Some(fold_add_values(&regs[acc as usize], items.iter())?)
    } else {
        None
    };
    if let Some(out) = folded {
        assign_reg(frame_raw, regs, acc as usize, out);
    }
    Ok(())
}

pub(in crate::vm::vm::runtime::frame::run::opcode::container_ops) fn fold_add_values<'a>(
    initial: &Val,
    values: impl Iterator<Item = &'a Val> + Clone,
) -> Result<Val> {
    if let Val::Int(initial_total) = initial {
        let mut total = *initial_total;
        let mut all_int = true;
        for item in values.clone() {
            if let Val::Int(value) = item {
                total = total.wrapping_add(*value);
            } else {
                all_int = false;
                break;
            }
        }
        if all_int {
            return Ok(Val::Int(total));
        }
    }

    let mut out = initial.clone();
    for item in values {
        out = BinOp::Add.eval_vals(&out, item)?;
    }
    Ok(out)
}

#[inline]
pub(super) fn index_value(list: &[Val], index: i64) -> Option<Val> {
    if index < 0 {
        Some(Val::Nil)
    } else {
        Some(list.get(index as usize).cloned().unwrap_or(Val::Nil))
    }
}
