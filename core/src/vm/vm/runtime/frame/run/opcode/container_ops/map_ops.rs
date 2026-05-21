use anyhow::{Result, anyhow};

use crate::val::Val;

use super::super::super::helpers::{assign_reg_with_metrics, fold_add_values_with_metrics};

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_has(
    regs: &mut [Val],
    dst: u16,
    map: u16,
    key: u16,
    collect_metrics: bool,
) -> Result<()> {
    let key = regs[key as usize]
        .as_str()
        .ok_or_else(|| anyhow!("has() key must be a string"))?;
    let out = match &regs[map as usize] {
        Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
        _ => return Err(anyhow!("has() first argument must be a map")),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    Ok(())
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_has_k(
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    map: u16,
    key_index: u16,
    collect_metrics: bool,
) -> Result<()> {
    let key = consts[key_index as usize].as_str().unwrap_or("");
    let out = match &regs[map as usize] {
        Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
        _ => return Err(anyhow!("has() first argument must be a map")),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
    Ok(())
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_values_fold_add(
    regs: &mut [Val],
    acc: u16,
    map: u16,
    collect_metrics: bool,
) -> Result<()> {
    let folded = if let Val::Map(values) = &regs[map as usize] {
        Some(fold_add_values_with_metrics(
            &regs[acc as usize],
            values.values(),
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
