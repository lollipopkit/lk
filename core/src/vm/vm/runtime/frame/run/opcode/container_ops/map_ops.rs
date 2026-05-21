use anyhow::{Result, anyhow};

use crate::val::Val;
use crate::vm::vm::frame::FrameState;

use super::super::super::helpers::assign_reg;
use super::list_ops::fold_add_values;

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_has(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    map: u16,
    key: u16,
) -> Result<()> {
    let key = regs[key as usize]
        .as_str()
        .ok_or_else(|| anyhow!("has() key must be a string"))?;
    let out = match &regs[map as usize] {
        Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
        _ => return Err(anyhow!("has() first argument must be a map")),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
    Ok(())
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_has_k(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    map: u16,
    key_index: u16,
) -> Result<()> {
    let key = consts[key_index as usize].as_str().unwrap_or("");
    let out = match &regs[map as usize] {
        Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
        _ => return Err(anyhow!("has() first argument must be a map")),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
    Ok(())
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_values_fold_add(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    acc: u16,
    map: u16,
) -> Result<()> {
    let folded = if let Val::Map(values) = &regs[map as usize] {
        Some(fold_add_values(&regs[acc as usize], values.values())?)
    } else {
        None
    };
    if let Some(out) = folded {
        assign_reg(frame_raw, regs, acc as usize, out);
    }
    Ok(())
}
