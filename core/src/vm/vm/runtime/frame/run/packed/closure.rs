use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, ClosureInit, ClosureValue, Val};
use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::frame::FrameState;

use super::super::helpers::assign_reg;

pub(super) fn make_closure_value(
    function: &Function,
    proto: u16,
    ctx: &mut VmContext,
    regs: &[Val],
    _frame_base: usize,
) -> Result<Val> {
    let proto = function
        .protos
        .get(proto as usize)
        .ok_or_else(|| anyhow!("closure proto out of range"))?;
    if proto.self_name.is_none() && proto.captures.is_empty() {
        return Ok(proto
            .empty_closure
            .get_or_init(|| {
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
            })
            .clone());
    }
    let captured_env = if proto.self_name.is_some() {
        Arc::new(ctx.snapshot())
    } else {
        Arc::clone(&proto.empty_env)
    };
    let captures = if proto.captures.is_empty() {
        Arc::clone(&proto.empty_captures)
    } else if let [spec] = proto.captures.as_ref().as_slice() {
        let value = match spec {
            CaptureSpec::Register { src, .. } => {
                let idx = *src as usize;
                regs.get(idx).cloned().unwrap_or(Val::Nil)
            }
            CaptureSpec::Const { kidx, .. } => function.consts.get(*kidx as usize).cloned().unwrap_or(Val::Nil),
            CaptureSpec::Global { name } => ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil),
        };
        ClosureCapture::from_shared_names_one(Arc::clone(&proto.capture_names), value)
    } else {
        let mut values = Vec::with_capacity(proto.captures.len());
        for spec in proto.captures.iter() {
            match spec {
                CaptureSpec::Register { src, .. } => {
                    let idx = *src as usize;
                    let value = regs.get(idx).cloned().unwrap_or(Val::Nil);
                    values.push(value);
                }
                CaptureSpec::Const { kidx, .. } => {
                    let value = function.consts.get(*kidx as usize).cloned().unwrap_or(Val::Nil);
                    values.push(value);
                }
                CaptureSpec::Global { name } => {
                    let value = ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil);
                    values.push(value);
                }
            }
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
    let self_binding = proto.self_name.as_ref().map(|name| (name.clone(), closure.clone()));
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
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &VmContext,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    dst: u16,
    idx: u16,
) -> Result<()> {
    let capture_idx = idx as usize;
    if let Some(spec) = frame_capture_specs.as_ref().and_then(|specs| specs.get(capture_idx)) {
        match spec {
            CaptureSpec::Global { name } => {
                let value = ctx.get(name.as_str()).cloned().unwrap_or(Val::Nil);
                assign_reg(frame_raw, regs, dst as usize, value);
            }
            _ => {
                let captured = capture_value(frame_captures, capture_idx)?;
                assign_reg(frame_raw, regs, dst as usize, captured);
            }
        }
    } else {
        let captured = capture_value(frame_captures, capture_idx)?;
        assign_reg(frame_raw, regs, dst as usize, captured);
    }
    Ok(())
}

#[inline]
fn capture_value(frame_captures: &Option<Arc<ClosureCapture>>, capture_idx: usize) -> Result<Val> {
    frame_captures
        .as_ref()
        .and_then(|captures| captures.value_at(capture_idx).cloned())
        .ok_or_else(|| anyhow!("Capture index {} out of bounds", capture_idx))
}
