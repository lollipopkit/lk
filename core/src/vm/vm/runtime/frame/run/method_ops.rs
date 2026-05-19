use anyhow::anyhow;

use crate::val::Val;
use crate::vm::bytecode::Function;
use crate::vm::context::VmContext;
use crate::vm::vm::caches::GlobalEntry;
use crate::vm::vm::frame::FrameState;

use super::helpers::assign_reg;

pub(super) fn run_call_method0(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    func: &Function,
    dst: u16,
    receiver: u16,
    method: u16,
) -> anyhow::Result<()> {
    let method_val = func
        .consts
        .get(method as usize)
        .ok_or_else(|| anyhow!("CallMethod0 method constant out of range: {}", method))?;
    let receiver_val = regs[receiver as usize].clone();
    let value = ctx.call_method_zero(receiver_val, method_val)?;
    assign_reg(frame_raw, regs, dst as usize, value);
    Ok(())
}

pub(super) fn run_call_global_method0(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    func: &Function,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    dst: u16,
    receiver: u16,
    method: u16,
) -> anyhow::Result<()> {
    let receiver_val = resolve_global_receiver(func, ctx, global_ic, pc, receiver)?;
    let method_val = func
        .consts
        .get(method as usize)
        .ok_or_else(|| anyhow!("CallGlobalMethod0 method constant out of range: {}", method))?;
    let value = ctx.call_method_zero(receiver_val, method_val)?;
    assign_reg(frame_raw, regs, dst as usize, value);
    Ok(())
}

fn resolve_global_receiver(
    func: &Function,
    ctx: &mut VmContext,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    name_index: u16,
) -> anyhow::Result<Val> {
    let name_val = func
        .consts
        .get(name_index as usize)
        .ok_or_else(|| anyhow!("CallGlobalMethod0 receiver constant out of range: {}", name_index))?;
    let Some(name) = name_val.as_str() else {
        let fallback_name = format!("{name_val}");
        return Ok(ctx.get(fallback_name.as_str()).cloned().unwrap_or(Val::Nil));
    };
    let key_ptr = name.as_ptr() as usize;
    let current_generation = ctx.generation();
    let local_shadowed = ctx.is_local_name(name);
    if !local_shadowed
        && let Some(GlobalEntry(ptr, value, generation)) = &global_ic[pc]
        && *ptr == key_ptr
        && *generation == current_generation
    {
        return Ok(value.clone());
    }
    let mut out = Val::Nil;
    if !local_shadowed && let Some(value) = ctx.get(name) {
        out = value.clone();
    }
    if matches!(out, Val::Nil)
        && let Some(value) = ctx.get_value(name)
    {
        out = value;
    }
    if matches!(out, Val::Nil)
        && let Some(value) = ctx.resolver().get_builtin(name)
    {
        out = value.clone();
    }
    if !local_shadowed {
        global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), current_generation));
    }
    Ok(out)
}
