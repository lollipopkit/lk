use anyhow::{Result, anyhow};

use crate::val::Val;
use crate::vm::bytecode::Function;
use crate::vm::context::VmContext;

use super::super::helpers::{
    assign_pattern_bindings_for_context_with_metrics, assign_pattern_bindings_with_metrics, assign_reg_with_metrics,
    clear_pattern_bindings_with_metrics,
};

#[inline]
pub(super) fn run_pattern_match(
    regs: &mut [Val],
    ctx: &VmContext,
    function: &Function,
    dst: u16,
    src: u16,
    plan_index: u16,
    collect_metrics: bool,
) -> Result<()> {
    let plan = &function.pattern_plans[plan_index as usize];
    let value = &regs[src as usize];
    match plan.pattern.matches(value, Some(ctx))? {
        Some(bound) => {
            assign_pattern_bindings_with_metrics(regs, &plan.bindings, &bound, collect_metrics);
            assign_reg_with_metrics(regs, dst as usize, Val::Bool(true), collect_metrics);
        }
        None => {
            clear_pattern_bindings_with_metrics(regs, &plan.bindings, collect_metrics);
            assign_reg_with_metrics(regs, dst as usize, Val::Bool(false), collect_metrics);
        }
    }
    Ok(())
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_pattern_match_or_fail(
    regs: &mut [Val],
    ctx: &mut VmContext,
    function: &Function,
    src: u16,
    plan_index: u16,
    err_index: u16,
    is_const: bool,
    collect_metrics: bool,
) -> Result<()> {
    let plan = &function.pattern_plans[plan_index as usize];
    let value = &regs[src as usize];
    let Some(bound) = plan.pattern.matches(value, Some(&*ctx))? else {
        return Err(error_from_const(function, err_index));
    };

    let mut assigned = Vec::with_capacity(plan.bindings.len());
    assign_pattern_bindings_for_context_with_metrics(regs, &plan.bindings, &bound, &mut assigned, collect_metrics);
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
