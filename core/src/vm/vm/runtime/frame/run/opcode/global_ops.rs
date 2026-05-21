use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, Val};
use crate::vm::bytecode::CaptureSpec;
use crate::vm::context::VmContext;
use crate::vm::vm::caches::GlobalEntry;
use crate::vm::vm::frame::FrameState;

use super::super::helpers::assign_reg;

#[inline]
pub(super) fn run_load_global(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    consts: &[Val],
    ctx: &mut VmContext,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    dst: u16,
    name_index: u16,
) {
    let name_val = &consts[name_index as usize];
    let mut out = Val::Nil;
    if let Some(name) = name_val.as_str() {
        let key_ptr = name.as_ptr() as usize;
        let current_generation = ctx.generation();
        let local_shadowed = ctx.is_local_name(name);
        if !local_shadowed
            && let Some(GlobalEntry(ptr, value, generation)) = &global_ic[pc]
            && *ptr == key_ptr
            && *generation == current_generation
        {
            out = value.clone();
        } else if !local_shadowed && let Some(value) = ctx.get(name) {
            out = value.clone();
            global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), current_generation));
        }
        if matches!(out, Val::Nil)
            && let Some(value) = ctx.get_value(name)
        {
            out = value;
            if !local_shadowed {
                global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), current_generation));
            }
        }
    } else {
        let fallback_name = format!("{name_val}");
        if let Some(value) = ctx.get(fallback_name.as_str()) {
            out = value.clone();
        }
    }
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(super) fn run_define_global(regs: &[Val], consts: &[Val], ctx: &mut VmContext, name_index: u16, src: u16) {
    let name_val = &consts[name_index as usize];
    if let Some(name) = name_val.as_str() {
        ctx.set(name.to_string(), regs[src as usize].clone());
    }
}

#[inline]
pub(super) fn run_load_capture(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    dst: u16,
    index: u16,
) -> Result<()> {
    let capture_idx = index as usize;
    let mut captured = frame_captures
        .as_ref()
        .and_then(|captures| captures.value_at(capture_idx).cloned())
        .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))?;
    if let Some(specs) = frame_capture_specs
        && let Some(CaptureSpec::Global { name }) = specs.get(capture_idx)
    {
        if let Some(value) = ctx.get(name.as_str()).cloned() {
            captured = value;
        } else {
            captured = ctx.get_value(name.as_str()).unwrap_or(Val::Nil);
        }
    }
    assign_reg(frame_raw, regs, dst as usize, captured);
    Ok(())
}
