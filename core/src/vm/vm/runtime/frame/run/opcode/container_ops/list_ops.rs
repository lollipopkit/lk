use anyhow::Result;

use crate::val::Val;
use crate::vm::copy_container_value_for_register_with_metrics;

use super::super::super::helpers::{assign_reg_with_metrics, fold_add_values_with_metrics};

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_fold_add(
    regs: &mut [Val],
    acc: u16,
    list: u16,
    collect_metrics: bool,
) -> Result<()> {
    let folded = if let Val::List(items) = &regs[list as usize] {
        Some(fold_add_values_with_metrics(
            &regs[acc as usize],
            items.iter(),
            collect_metrics,
        )?)
    } else {
        None
    };
    if let Some(out) = folded {
        assign_reg_with_metrics(regs, acc as usize, out, collect_metrics);
    }
    Ok(())
}

#[inline]
pub(super) fn index_value(list: &[Val], index: i64, collect_metrics: bool) -> Option<Val> {
    let index = if index < 0 {
        list.len().checked_sub(index.unsigned_abs() as usize)?
    } else {
        index as usize
    };
    Some(
        list.get(index)
            .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
            .unwrap_or(Val::Nil),
    )
}

#[inline]
pub(super) fn slice_range_value(list: &[Val], key: &[Val], collect_metrics: bool) -> Option<Val> {
    let (start, end) = range_key_bounds(key, list.len())?;
    let mut out = Vec::with_capacity(end.saturating_sub(start));
    for value in &list[start..end] {
        out.push(copy_container_value_for_register_with_metrics(value, collect_metrics));
    }
    Some(Val::List(out.into()))
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
