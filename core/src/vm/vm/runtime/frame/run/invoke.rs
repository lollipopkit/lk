use std::sync::Arc;

use anyhow::Result;

use crate::val::{
    ClosureCapture, NativeArgs, RustFastFunction, RustFastFunctionNamed, RustFunction, RustFunctionNamed, Val,
};
use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{CallIc, CallReturnLayout, ClosureFastCache, FunctionRuntimePlan};
use crate::vm::vm::frame::{CallFrameMeta, FrameInfo, RegisterSpan};
use crate::vm::write_register_value_with_metrics;

use super::raw_boundary::{exec_positional_fast_span_unchecked, with_vm_mut};

#[derive(Clone, Copy)]
pub(super) enum NativeCallable {
    Rust(RustFunction),
    RustFast(RustFastFunction),
    RustNamed(RustFunctionNamed),
    RustFastNamed(RustFastFunctionNamed),
}

impl NativeCallable {
    #[inline]
    pub(super) fn from_val(value: &Val) -> Option<Self> {
        match value {
            Val::RustFunction(func) => Some(Self::Rust(*func)),
            Val::RustFastFunction(func) => Some(Self::RustFast(*func)),
            Val::RustFunctionNamed(func) => Some(Self::RustNamed(*func)),
            Val::RustFastFunctionNamed(func) => Some(Self::RustFastNamed(*func)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ArgWindow<'a> {
    args: &'a [Val],
}

impl<'a> ArgWindow<'a> {
    #[inline]
    pub(super) fn new(args: &'a [Val]) -> Self {
        Self { args }
    }

    #[inline]
    pub(super) fn as_slice(self) -> &'a [Val] {
        self.args
    }
}

#[derive(Clone, Copy)]
pub(super) struct ReturnSlot {
    base: usize,
    retc: u8,
}

impl ReturnSlot {
    #[inline]
    pub(super) fn new(base: usize, retc: u8) -> Self {
        Self { base, retc }
    }

    #[inline]
    pub(super) fn write(self, regs: &mut [Val], value: Val, collect_metrics: bool) {
        if self.retc > 0 {
            write_register_value_with_metrics(regs, self.base, value, collect_metrics);
        }
    }
}

#[inline]
pub(super) fn invoke_native_callable_with_ic(
    ctx: &mut VmContext,
    regs: &mut [Val],
    call_ic_slot: &mut Option<CallIc>,
    callable: NativeCallable,
    argc: u8,
    ret: CallReturnLayout,
    collect_metrics: bool,
) -> Result<bool> {
    let base = ret.base as usize;
    let argc_usize = argc as usize;
    let args = || ArgWindow::new(&regs[base..base + argc_usize]);

    let value = match call_ic_slot.as_ref() {
        Some(CallIc::Rust(fp, cached_argc, cached_ret))
            if argc == *cached_argc
                && cached_ret.matches(ret.base, ret.retc)
                && matches!(callable, NativeCallable::Rust(_)) =>
        {
            invoke_rust_function_fast(ctx, *fp, args())?
        }
        Some(CallIc::RustFast(fp, cached_argc, cached_ret))
            if argc == *cached_argc
                && cached_ret.matches(ret.base, ret.retc)
                && matches!(callable, NativeCallable::RustFast(_)) =>
        {
            invoke_rust_fast_function(ctx, *fp, args())?
        }
        Some(CallIc::RustFastNamed(fp, cached_argc, cached_ret))
            if argc == *cached_argc
                && cached_ret.matches(ret.base, ret.retc)
                && matches!(callable, NativeCallable::RustFastNamed(_)) =>
        {
            invoke_rust_fast_function_named(ctx, *fp, args(), &[])?
        }
        Some(CallIc::RustNamed(fp, cached_argc, cached_ret))
            if argc == *cached_argc
                && cached_ret.matches(ret.base, ret.retc)
                && matches!(callable, NativeCallable::RustNamed(_)) =>
        {
            invoke_rust_function_named_fast(ctx, *fp, args(), &[])?
        }
        _ => match callable {
            NativeCallable::Rust(fptr) => {
                *call_ic_slot = Some(CallIc::Rust(fptr, argc, ret));
                invoke_rust_function_fast(ctx, fptr, args())?
            }
            NativeCallable::RustFast(fptr) => {
                *call_ic_slot = Some(CallIc::RustFast(fptr, argc, ret));
                invoke_rust_fast_function(ctx, fptr, args())?
            }
            NativeCallable::RustFastNamed(fptr) => {
                *call_ic_slot = Some(CallIc::RustFastNamed(fptr, argc, ret));
                invoke_rust_fast_function_named(ctx, fptr, args(), &[])?
            }
            NativeCallable::RustNamed(fptr) => {
                *call_ic_slot = Some(CallIc::RustNamed(fptr, argc, ret));
                invoke_rust_function_named_fast(ctx, fptr, args(), &[])?
            }
        },
    };

    ReturnSlot::new(base, ret.retc).write(regs, value, collect_metrics);
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
#[inline]
pub(super) fn invoke_vm_closure_fast_unchecked(
    self_ptr: *mut Vm,
    fun: &Function,
    runtime: Option<&FunctionRuntimePlan>,
    args: RegisterSpan,
    ctx: &mut VmContext,
    frame_info: Option<&FrameInfo>,
    captures: Option<Arc<ClosureCapture>>,
    capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    cache: &mut ClosureFastCache,
    return_meta: CallFrameMeta,
    collect_metrics: bool,
) -> Result<Val> {
    exec_positional_fast_span_unchecked(
        self_ptr,
        fun,
        runtime,
        args,
        ctx,
        frame_info,
        captures,
        capture_specs,
        cache,
        return_meta,
        collect_metrics,
    )
}

#[inline]
pub(super) fn take_pending_resume_pc(self_ptr: *mut Vm, fallback_pc: usize) -> usize {
    with_vm_mut(self_ptr, |vm| vm.pending_resume_pc.take().unwrap_or(fallback_pc))
}

#[inline]
pub(super) fn clear_pending_resume_pc(self_ptr: *mut Vm) {
    let _ = with_vm_mut(self_ptr, |vm| vm.pending_resume_pc.take());
}

#[inline]
#[allow(dead_code)]
pub(super) fn invoke_rust_function(ctx: &mut VmContext, func: RustFunction, args: &[Val]) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(args, ctx) {
        Ok(val) => Ok(val),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}

#[inline]
pub(super) fn invoke_rust_function_fast(ctx: &mut VmContext, func: RustFunction, args: ArgWindow<'_>) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(args.as_slice(), ctx) {
        Ok(value) => Ok(value),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}

#[inline]
pub(super) fn invoke_rust_fast_function(
    ctx: &mut VmContext,
    func: RustFastFunction,
    args: ArgWindow<'_>,
) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(NativeArgs::new(args.as_slice()), ctx) {
        Ok(value) => Ok(value),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}

#[inline]
pub(super) fn invoke_rust_fast_function_named(
    ctx: &mut VmContext,
    func: RustFastFunctionNamed,
    args: ArgWindow<'_>,
    named: &[(String, Val)],
) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(NativeArgs::new(args.as_slice()), named, ctx) {
        Ok(value) => Ok(value),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}

#[inline]
pub(super) fn invoke_rust_function_named_fast(
    ctx: &mut VmContext,
    func: RustFunctionNamed,
    positional: ArgWindow<'_>,
    named: &[(String, Val)],
) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(positional.as_slice(), named, ctx) {
        Ok(value) => Ok(value),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}

#[inline]
#[allow(dead_code)]
pub(super) fn invoke_rust_function_named(
    ctx: &mut VmContext,
    func: RustFunctionNamed,
    positional: &[Val],
    named: &[(String, Val)],
) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(positional, named, ctx) {
        Ok(val) => Ok(val),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}
