use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, ClosureInit, ClosureValue, Val};
use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::copy_value_for_register_with_metrics;

use super::super::helpers::{assign_reg_with_metrics, copy_capture_spec_value};

pub(super) fn make_closure_value(
    function: &Function,
    proto: u16,
    ctx: &mut VmContext,
    regs: &[Val],
    _frame_base: usize,
    collect_metrics: bool,
) -> Result<Val> {
    let proto = function
        .protos
        .get(proto as usize)
        .ok_or_else(|| anyhow!("closure proto out of range"))?;
    if proto.self_name.is_none() && proto.captures.is_empty() {
        return Ok(copy_value_for_register_with_metrics(
            proto.empty_closure.get_or_init(|| {
                let closure = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
                    params: Arc::clone(&proto.params),
                    param_types: Arc::clone(&proto.param_types),
                    named_params: Arc::clone(&proto.named_params),
                    body: Arc::clone(&proto.body),
                    env: Arc::clone(&proto.empty_env),
                    upvalues: Arc::clone(&proto.empty_upvalues),
                    captures: Arc::clone(&proto.empty_captures),
                    capture_specs: Arc::clone(&proto.captures),
                    default_funcs: Arc::clone(&proto.default_funcs),
                    code: Arc::clone(&proto.code),
                    debug_name: None,
                    debug_location: None,
                })));
                if proto.func.is_none()
                    && proto.code.get().is_none()
                    && let Val::Closure(closure_arc) = &closure
                {
                    let compiler = Compiler::new();
                    let compiled = compiler.compile_function_with_param_types_and_captures(
                        proto.params.as_ref(),
                        proto.param_types.as_ref(),
                        proto.named_params.as_ref(),
                        proto.body.as_ref(),
                        proto.captures.as_ref(),
                    );
                    let _ = closure_arc.code.set(Arc::new(compiled));
                }
                closure
            }),
            collect_metrics,
        ));
    }
    let captured_env = if proto.self_name.is_some() {
        Arc::new(ctx.snapshot())
    } else {
        Arc::clone(&proto.empty_env)
    };
    let captures = if proto.captures.is_empty() {
        Arc::clone(&proto.empty_captures)
    } else if let [spec] = proto.captures.as_ref().as_slice() {
        let value = copy_capture_spec_value(ctx, regs, &function.consts, spec, collect_metrics);
        ClosureCapture::from_shared_names_one(Arc::clone(&proto.capture_names), value)
    } else {
        let mut values = Vec::with_capacity(proto.captures.len());
        for spec in proto.captures.iter() {
            values.push(copy_capture_spec_value(
                ctx,
                regs,
                &function.consts,
                spec,
                collect_metrics,
            ));
        }
        ClosureCapture::from_shared_names(Arc::clone(&proto.capture_names), values)
    };
    let mut closure = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
        params: Arc::clone(&proto.params),
        param_types: Arc::clone(&proto.param_types),
        named_params: Arc::clone(&proto.named_params),
        body: Arc::clone(&proto.body),
        env: captured_env,
        upvalues: Arc::clone(&proto.empty_upvalues),
        captures,
        capture_specs: Arc::clone(&proto.captures),
        default_funcs: Arc::clone(&proto.default_funcs),
        code: Arc::clone(&proto.code),
        debug_name: proto.self_name.clone(),
        debug_location: None,
    })));
    let self_binding = proto.self_name.as_ref().map(|name| {
        (
            name.clone(),
            copy_value_for_register_with_metrics(&closure, collect_metrics),
        )
    });
    if let (Some((name, clone_for_env)), Val::Closure(closure_arc)) = (self_binding, &mut closure)
        && let Some(closure_value) = Arc::get_mut(closure_arc)
        && let Some(env_mut) = Arc::get_mut(&mut closure_value.env)
    {
        env_mut.define(name, clone_for_env);
    }
    if proto.func.is_none()
        && proto.code.get().is_none()
        && let Val::Closure(closure_arc) = &closure
    {
        let compiler = Compiler::new();
        let compiled = compiler.compile_function_with_param_types_and_captures(
            proto.params.as_ref(),
            proto.param_types.as_ref(),
            proto.named_params.as_ref(),
            proto.body.as_ref(),
            proto.captures.as_ref(),
        );
        let _ = closure_arc.code.set(Arc::new(compiled));
    }
    Ok(closure)
}

#[inline]
pub(super) fn run_load_capture(
    regs: &mut [Val],
    ctx: &VmContext,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    dst: u16,
    idx: u16,
    collect_metrics: bool,
) -> Result<()> {
    let capture_idx = idx as usize;
    if let Some(spec) = frame_capture_specs.as_ref().and_then(|specs| specs.get(capture_idx)) {
        match spec {
            CaptureSpec::Global { name } => {
                let value = ctx
                    .get(name.as_str())
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil);
                assign_reg_with_metrics(regs, dst as usize, value, collect_metrics);
            }
            _ => {
                let captured = capture_value(frame_captures, capture_idx, collect_metrics)?;
                assign_reg_with_metrics(regs, dst as usize, captured, collect_metrics);
            }
        }
    } else {
        let captured = capture_value(frame_captures, capture_idx, collect_metrics)?;
        assign_reg_with_metrics(regs, dst as usize, captured, collect_metrics);
    }
    Ok(())
}

#[inline]
fn capture_value(
    frame_captures: &Option<Arc<ClosureCapture>>,
    capture_idx: usize,
    collect_metrics: bool,
) -> Result<Val> {
    frame_captures
        .as_ref()
        .and_then(|captures| captures.value_at(capture_idx))
        .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
        .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))
}
