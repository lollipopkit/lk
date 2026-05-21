use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, Val};
use crate::vm::bytecode::CaptureSpec;
use crate::vm::context::VmContext;
use crate::vm::copy_value_for_register_with_metrics;
use crate::vm::vm::caches::GlobalEntry;

use super::super::helpers::{assign_reg_with_metrics, load_global_for_register};

#[inline]
pub(super) fn run_load_global(
    regs: &mut [Val],
    consts: &[Val],
    ctx: &mut VmContext,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    dst: u16,
    name_index: u16,
    collect_metrics: bool,
) {
    let name_val = &consts[name_index as usize];
    let out = load_global_for_register(ctx, global_ic, pc, name_val, collect_metrics);
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline]
pub(super) fn run_define_global(
    regs: &[Val],
    consts: &[Val],
    ctx: &mut VmContext,
    name_index: u16,
    src: u16,
    collect_metrics: bool,
) {
    let name_val = &consts[name_index as usize];
    if let Some(name) = name_val.as_str() {
        ctx.set(
            name.to_string(),
            copy_value_for_register_with_metrics(&regs[src as usize], collect_metrics),
        );
    }
}

#[inline]
pub(super) fn run_load_capture(
    regs: &mut [Val],
    ctx: &mut VmContext,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    dst: u16,
    index: u16,
    collect_metrics: bool,
) -> Result<()> {
    let capture_idx = index as usize;
    let mut captured = frame_captures
        .as_ref()
        .and_then(|captures| captures.value_at(capture_idx))
        .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
        .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))?;
    if let Some(specs) = frame_capture_specs
        && let Some(CaptureSpec::Global { name }) = specs.get(capture_idx)
    {
        if let Some(value) = ctx.get(name.as_str()) {
            captured = copy_value_for_register_with_metrics(value, collect_metrics);
        } else {
            captured = Val::Nil;
        }
    }
    assign_reg_with_metrics(regs, dst as usize, captured, collect_metrics);
    Ok(())
}
