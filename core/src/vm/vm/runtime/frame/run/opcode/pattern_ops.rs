use anyhow::{Result, anyhow};

use crate::val::Val;
use crate::vm::bytecode::Function;
use crate::vm::context::VmContext;
use crate::vm::vm::frame::FrameState;

use super::super::helpers::assign_reg;

#[inline]
pub(super) fn run_pattern_match(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &VmContext,
    function: &Function,
    dst: u16,
    src: u16,
    plan_index: u16,
) -> Result<()> {
    let plan = &function.pattern_plans[plan_index as usize];
    let value = &regs[src as usize];
    match plan.pattern.matches(value, Some(ctx))? {
        Some(bound) => {
            for binding in &plan.bindings {
                if let Some((_, value)) = bound.iter().find(|(name, _)| name == &binding.name) {
                    assign_reg(frame_raw, regs, binding.reg as usize, value.clone());
                } else {
                    assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                }
            }
            assign_reg(frame_raw, regs, dst as usize, Val::Bool(true));
        }
        None => {
            for binding in &plan.bindings {
                assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
            }
            assign_reg(frame_raw, regs, dst as usize, Val::Bool(false));
        }
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_pattern_match_or_fail(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    function: &Function,
    src: u16,
    plan_index: u16,
    err_index: u16,
    is_const: bool,
) -> Result<()> {
    let plan = &function.pattern_plans[plan_index as usize];
    let value = &regs[src as usize];
    let Some(bound) = plan.pattern.matches(value, Some(&*ctx))? else {
        return Err(error_from_const(function, err_index));
    };

    let mut assigned = Vec::with_capacity(plan.bindings.len());
    for binding in &plan.bindings {
        if let Some((_, value)) = bound.iter().find(|(name, _)| name == &binding.name) {
            let cloned = value.clone();
            assign_reg(frame_raw, regs, binding.reg as usize, cloned.clone());
            assigned.push((binding.name.clone(), cloned));
        } else {
            assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
        }
    }
    for (name, value) in assigned {
        if is_const {
            ctx.define_const(name, value);
        } else {
            ctx.set(name, value);
        }
    }
    Ok(())
}

#[inline]
pub(super) fn run_raise(function: &Function, err_index: u16) -> Result<Val> {
    Err(error_from_const(function, err_index))
}

#[inline]
fn error_from_const(function: &Function, err_index: u16) -> anyhow::Error {
    let message_value = &function.consts[err_index as usize];
    let message = match message_value {
        value if value.as_str().is_some() => value.as_str().unwrap().to_string(),
        other => other.to_string(),
    };
    anyhow!(message)
}
