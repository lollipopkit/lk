use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::vm::frame::FrameState;

use super::super::super::helpers::assign_reg;

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_fold_add(
    frame_raw: *mut FrameState<'_, '_>,
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
    let index = if index < 0 {
        list.len().checked_sub(index.unsigned_abs() as usize)?
    } else {
        index as usize
    };
    Some(list.get(index).cloned().unwrap_or(Val::Nil))
}

#[inline]
pub(super) fn slice_range_value(list: &[Val], key: &[Val]) -> Option<Val> {
    let (start, end) = range_key_bounds(key, list.len())?;
    Some(Val::List(list[start..end].to_vec().into()))
}

#[inline]
pub(crate) fn range_key_bounds(key: &[Val], len: usize) -> Option<(usize, usize)> {
    let Val::Int(first) = key.first()? else {
        return None;
    };
    let mut previous = *first;
    for item in key.iter().skip(1) {
        let Val::Int(current) = item else {
            return None;
        };
        if *current != previous + 1 {
            return None;
        }
        previous = *current;
    }

    let start = normalize_slice_bound(*first, len);
    let end = normalize_slice_bound(previous + 1, len);
    Some((start.min(end), end))
}

#[inline]
fn normalize_slice_bound(index: i64, len: usize) -> usize {
    if index < 0 {
        len.saturating_sub(index.unsigned_abs() as usize)
    } else {
        (index as usize).min(len)
    }
}
