use anyhow::anyhow;

use crate::val::Val;
use crate::vm::bytecode::Function;
use crate::vm::context::VmContext;
use crate::vm::copy_value_for_register_with_metrics;
use crate::vm::vm::caches::GlobalEntry;

use super::helpers::{assign_reg_with_metrics, load_global_for_register};

pub(super) fn run_call_method0(
    regs: &mut [Val],
    ctx: &mut VmContext,
    func: &Function,
    dst: u16,
    receiver: u16,
    method: u16,
    collect_metrics: bool,
) -> anyhow::Result<()> {
    let method_val = func
        .consts
        .get(method as usize)
        .ok_or_else(|| anyhow!("CallMethod0 method constant out of range: {}", method))?;
    let receiver_val = copy_value_for_register_with_metrics(&regs[receiver as usize], collect_metrics);
    let value = ctx.call_method_zero(receiver_val, method_val)?;
    assign_reg_with_metrics(regs, dst as usize, value, collect_metrics);
    Ok(())
}

pub(super) fn run_call_global_method0(
    regs: &mut [Val],
    ctx: &mut VmContext,
    func: &Function,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    dst: u16,
    receiver: u16,
    method: u16,
    collect_metrics: bool,
) -> anyhow::Result<()> {
    let receiver_val = resolve_global_receiver(func, ctx, global_ic, pc, receiver, collect_metrics)?;
    let method_val = func
        .consts
        .get(method as usize)
        .ok_or_else(|| anyhow!("CallGlobalMethod0 method constant out of range: {}", method))?;
    let value = ctx.call_method_zero(receiver_val, method_val)?;
    assign_reg_with_metrics(regs, dst as usize, value, collect_metrics);
    Ok(())
}

fn resolve_global_receiver(
    func: &Function,
    ctx: &mut VmContext,
    global_ic: &mut [Option<GlobalEntry>],
    pc: usize,
    name_index: u16,
    collect_metrics: bool,
) -> anyhow::Result<Val> {
    let name_val = func
        .consts
        .get(name_index as usize)
        .ok_or_else(|| anyhow!("CallGlobalMethod0 receiver constant out of range: {}", name_index))?;
    Ok(load_global_for_register(ctx, global_ic, pc, name_val, collect_metrics))
}
