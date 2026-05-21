use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, ClosureValue, Type, Val};
use crate::vm::alloc::RegionAllocator;
use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::copy_call_arg_value_for_register_with_metrics;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    CallIc, CallReturnLayout, CallSitePlan, ClosureFastCache, FunctionRuntimePlan, TinyCallPlan,
};
use crate::vm::vm::frame::{
    CallArgs, CallFrameMeta, CallFrameStackGuard, FrameInfo, FrameState, RegisterSpan, RegisterWindowRef,
};

use super::helpers::{assign_reg_with_metrics, frame_return_common};
use super::invoke::{
    ArgWindow, NativeCallable, ReturnSlot, clear_pending_resume_pc, invoke_native_callable_with_ic,
    invoke_rust_fast_function_named, invoke_rust_function_named_fast, invoke_vm_closure_fast_unchecked,
    take_pending_resume_pc,
};
use super::plan::get_or_build_named_call_site_plan;
use super::raw_boundary::{function_from_ptr, pop_vm_frame, push_vm_frame};

pub(crate) enum CallHotPath {
    Miss,
    Done,
    Return(Val),
}

#[inline]
fn finish_call_entry(
    result: CallHotPath,
    pc_ref: &mut usize,
    self_ptr: *mut Vm,
    resume_pc: usize,
    miss_msg: &'static str,
) -> CallHotPath {
    match result {
        CallHotPath::Done => {
            *pc_ref = take_pending_resume_pc(self_ptr, resume_pc);
            CallHotPath::Done
        }
        CallHotPath::Return(value) => CallHotPath::Return(value),
        CallHotPath::Miss => unreachable!("{}", miss_msg),
    }
}

#[inline]
pub(crate) fn call_hot_path_to_option(result: CallHotPath, miss_msg: &'static str) -> Option<Val> {
    match result {
        CallHotPath::Done => None,
        CallHotPath::Return(value) => Some(value),
        CallHotPath::Miss => unreachable!("{}", miss_msg),
    }
}

pub(crate) enum CallTarget {
    Closure(Arc<ClosureValue>),
    Native(NativeCallable),
    NotFunction(&'static str),
}

#[inline]
pub(crate) fn call_target_from_register(regs: &[Val], rf: u16) -> CallTarget {
    match &regs[rf as usize] {
        Val::Closure(closure) => CallTarget::Closure(Arc::clone(closure)),
        value => NativeCallable::from_val(value)
            .map_or_else(|| CallTarget::NotFunction(value.type_name()), CallTarget::Native),
    }
}

pub(crate) struct PreparedExactClosureCall {
    plan: CallSitePlan,
}

#[inline]
fn call_runtime_plan(fun: &Function) -> FunctionRuntimePlan {
    FunctionRuntimePlan::from_function(
        fun,
        fun.analysis.as_ref().map(|analysis| Arc::clone(&analysis.region_plan)),
    )
}

pub(crate) fn prepare_exact_closure_call(regs: &[Val], rf: u16, argc: u8) -> Result<PreparedExactClosureCall> {
    let Val::Closure(closure_arc) = &regs[rf as usize] else {
        return Err(anyhow!("{} is not an exact closure", regs[rf as usize].type_name()));
    };
    if !closure_arc.named_params.is_empty() {
        return Err(anyhow!("exact closure call does not accept named fallback"));
    }
    if closure_arc.params.len() != argc as usize {
        return Err(anyhow!(
            "Function expects {} positional arguments, got {}",
            closure_arc.params.len(),
            argc
        ));
    }

    let closure = closure_arc.as_ref();
    let fun = closure.code.get_or_init(|| {
        let c = Compiler::new();
        Arc::new(c.compile_function_with_param_types_and_captures(
            closure.params.as_ref(),
            closure.param_types.as_ref(),
            closure.named_params.as_ref(),
            closure.body.as_ref(),
            closure.capture_specs.as_ref(),
        ))
    });
    let (captures, capture_specs) = closure.frame_captures();
    Ok(PreparedExactClosureCall {
        plan: CallSitePlan::positional(
            Arc::as_ptr(closure_arc) as usize,
            fun.as_ref(),
            call_runtime_plan(fun.as_ref()),
            argc,
            CallReturnLayout::new(0, 0),
            TinyCallPlan::analyze(fun.as_ref()),
            captures,
            capture_specs,
            closure.frame_info(),
        ),
    })
}

fn plan_with_return_layout(mut plan: CallSitePlan, base: u16, retc: u8) -> CallSitePlan {
    plan.ret = CallReturnLayout::new(base, retc);
    plan
}

#[allow(clippy::too_many_arguments)]
fn new_positional_call_site_plan(
    closure_ptr: usize,
    fun: &Function,
    runtime: FunctionRuntimePlan,
    argc: u8,
    ret: CallReturnLayout,
    captures: Option<Arc<ClosureCapture>>,
    capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    frame_info: FrameInfo,
) -> Arc<CallSitePlan> {
    Arc::new(CallSitePlan::positional(
        closure_ptr,
        fun,
        runtime,
        argc,
        ret,
        TinyCallPlan::analyze(fun),
        captures,
        capture_specs,
        frame_info,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_prepared_exact_closure_call(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    base: u16,
    argc: u8,
    retc: u8,
    prepared: PreparedExactClosureCall,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let plan = plan_with_return_layout(prepared.plan, base, retc);
    let fun = function_from_ptr(plan.fun_ptr);
    let start = base as usize;
    let n = argc as usize;

    let mut cache = ClosureFastCache::new();
    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
    match invoke_vm_closure_fast_unchecked(
        self_ptr,
        fun,
        Some(&plan.runtime),
        RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
        ctx,
        Some(&plan.frame_info),
        plan.captures.clone(),
        plan.capture_specs.clone(),
        &mut cache,
        return_meta,
        collect_metrics,
    ) {
        Ok(val) => {
            if retc > 0 {
                assign_reg_with_metrics(regs, base as usize, val, collect_metrics);
            }
            call_ic[pc] = Some(CallIc::ClosurePositional {
                plan: Arc::new(plan),
                cache,
            });
            Ok(CallHotPath::Done)
        }
        Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_run_closure_ic_hot(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let Some(CallIc::ClosurePositional { plan, .. }) = call_ic[pc].as_ref() else {
        return Ok(CallHotPath::Miss);
    };
    if !plan.matches_layout(argc, base, retc) {
        return Ok(CallHotPath::Miss);
    }

    let Val::Closure(arc) = &regs[rf as usize] else {
        return Ok(CallHotPath::Miss);
    };
    let closure_ptr_matches = plan.closure_ptr_matches(arc);
    if !plan.matches_closure(arc) {
        return Ok(CallHotPath::Miss);
    }

    let fun = function_from_ptr(plan.fun_ptr);
    let start = base as usize;
    let n = argc as usize;
    let args_slice_fast = &regs[start..start + n];
    let tiny_captures = if closure_ptr_matches {
        plan.captures.as_deref()
    } else {
        Some(arc.captures.as_ref())
    };
    if let Some(val) = plan
        .tiny
        .as_ref()
        .and_then(|plan| plan.try_eval(args_slice_fast, tiny_captures))
    {
        if retc > 0 {
            assign_reg_with_metrics(regs, base as usize, val, collect_metrics);
        }
        return Ok(CallHotPath::Done);
    }

    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
    let (invoke_captures, invoke_capture_specs) = if closure_ptr_matches {
        (plan.captures.clone(), plan.capture_specs.clone())
    } else {
        arc.frame_captures()
    };
    if let Some(CallIc::ClosurePositional { plan, cache }) = call_ic[pc].as_mut() {
        let val = invoke_vm_closure_fast_unchecked(
            self_ptr,
            fun,
            Some(&plan.runtime),
            RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
            ctx,
            Some(&plan.frame_info),
            invoke_captures,
            invoke_capture_specs,
            cache,
            return_meta,
            collect_metrics,
        );
        match val {
            Ok(val) => {
                if retc > 0 {
                    assign_reg_with_metrics(regs, base as usize, val, collect_metrics);
                }
            }
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        }
        return Ok(CallHotPath::Done);
    }

    Ok(CallHotPath::Miss)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_run_positional_closure_call(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    base: u16,
    argc: u8,
    retc: u8,
    ret_layout: CallReturnLayout,
    closure_arc: &Arc<ClosureValue>,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let closure_ptr = Arc::as_ptr(closure_arc) as usize;
    let mut cached_fast = matches!(call_ic[pc].as_ref(), Some(CallIc::ClosurePositional { plan, .. }) if plan.closure_ptr == closure_ptr && plan.matches_layout(argc, base, retc));
    let supports_fast = cached_fast || closure_arc.supports_vm_positional_fast_path();
    if !supports_fast || !closure_arc.named_params.is_empty() {
        return Ok(CallHotPath::Miss);
    }

    let start = base as usize;
    let n = argc as usize;
    if !cached_fast && n != closure_arc.params.len() {
        return frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                closure_arc.params.len(),
                n
            )),
        )
        .map(CallHotPath::Return);
    }

    let closure = closure_arc.as_ref();
    let mut cached_fun_ptr = None;
    if let Some(CallIc::ClosurePositional { plan, .. }) = call_ic[pc].as_ref()
        && plan.closure_ptr == closure_ptr
        && plan.matches_layout(argc, base, retc)
    {
        cached_fun_ptr = Some(plan.fun_ptr);
    }
    let fun: &Function = if let Some(ptr) = cached_fun_ptr {
        function_from_ptr(ptr)
    } else {
        closure
            .code
            .get_or_init(|| {
                let c = Compiler::new();
                Arc::new(c.compile_function_with_param_types_and_captures(
                    closure.params.as_ref(),
                    closure.param_types.as_ref(),
                    closure.named_params.as_ref(),
                    closure.body.as_ref(),
                    closure.capture_specs.as_ref(),
                ))
            })
            .as_ref()
    };
    if !cached_fast
        && let Some(CallIc::ClosurePositional { plan, .. }) = call_ic[pc].as_ref()
        && plan.matches_layout(argc, base, retc)
        && std::ptr::eq(plan.fun_ptr, fun as *const Function)
    {
        cached_fast = true;
    }

    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
    if let Some(CallIc::ClosurePositional { plan, cache }) = call_ic[pc].as_mut()
        && cached_fast
    {
        match invoke_vm_closure_fast_unchecked(
            self_ptr,
            fun,
            Some(&plan.runtime),
            RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
            ctx,
            Some(&plan.frame_info),
            plan.captures.clone(),
            plan.capture_specs.clone(),
            cache,
            return_meta,
            collect_metrics,
        ) {
            Ok(val) => {
                if retc > 0 {
                    assign_reg_with_metrics(regs, base as usize, val, collect_metrics);
                }
            }
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        }
    } else {
        let (captures, capture_specs) = closure.frame_captures();
        let mut cache = ClosureFastCache::new();
        let frame_info = closure.frame_info_ref();
        let runtime = call_runtime_plan(fun);
        match invoke_vm_closure_fast_unchecked(
            self_ptr,
            fun,
            Some(&runtime),
            RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
            ctx,
            Some(frame_info),
            captures.clone(),
            capture_specs.clone(),
            &mut cache,
            return_meta,
            collect_metrics,
        ) {
            Ok(val) => {
                if retc > 0 {
                    assign_reg_with_metrics(regs, base as usize, val, collect_metrics);
                }
                call_ic[pc] = Some(CallIc::ClosurePositional {
                    plan: new_positional_call_site_plan(
                        closure_ptr,
                        fun,
                        runtime,
                        argc,
                        ret_layout,
                        captures,
                        capture_specs,
                        frame_info.clone(),
                    ),
                    cache,
                });
            }
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        }
    }

    Ok(CallHotPath::Done)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_positional_call_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    allocator: &RegionAllocator,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let ret_layout = CallReturnLayout::new(base, retc);
    match try_run_closure_ic_hot(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        collect_metrics,
    )? {
        CallHotPath::Done => return Ok(CallHotPath::Done),
        CallHotPath::Return(value) => return Ok(CallHotPath::Return(value)),
        CallHotPath::Miss => {}
    }

    match call_target_from_register(regs, rf) {
        CallTarget::Closure(closure_arc) => match try_run_positional_closure_call(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc,
            resume_pc,
            frame_base,
            self_ptr,
            base,
            argc,
            retc,
            ret_layout,
            &closure_arc,
            collect_metrics,
        )? {
            CallHotPath::Done => Ok(CallHotPath::Done),
            CallHotPath::Return(value) => Ok(CallHotPath::Return(value)),
            CallHotPath::Miss => run_closure_slow_call(
                frame_raw,
                regs,
                ctx,
                pc,
                resume_pc,
                frame_base,
                self_ptr,
                base,
                argc,
                retc,
                allocator,
                &closure_arc,
                collect_metrics,
            ),
        },
        CallTarget::Native(callable) => {
            match invoke_native_callable_with_ic(
                ctx,
                regs,
                &mut call_ic[pc],
                callable,
                argc,
                ret_layout,
                collect_metrics,
            ) {
                Ok(handled) => {
                    debug_assert!(handled);
                    Ok(CallHotPath::Done)
                }
                Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
            }
        }
        CallTarget::NotFunction(type_name) => {
            frame_return_common(frame_raw, pc, Err(anyhow!("{} is not a function", type_name))).map(CallHotPath::Return)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_positional_call_entry_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    allocator: &RegionAllocator,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let result = run_positional_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        *pc_ref,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        allocator,
        collect_metrics,
    )?;
    Ok(finish_call_entry(
        result,
        pc_ref,
        self_ptr,
        resume_pc,
        "positional call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_native_fast_call_entry_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc: usize,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let pc = *pc_ref;
    let Some(callable @ (NativeCallable::Rust(_) | NativeCallable::RustFast(_))) =
        NativeCallable::from_val(&regs[rf as usize])
    else {
        return Ok(CallHotPath::Miss);
    };
    let ret_layout = CallReturnLayout::new(base, retc);
    match invoke_native_callable_with_ic(ctx, regs, &mut call_ic[pc], callable, argc, ret_layout, collect_metrics) {
        Ok(handled) => {
            debug_assert!(handled);
            *pc_ref = next_pc;
            Ok(CallHotPath::Done)
        }
        Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_native_fast_or_positional_call_entry_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    allocator: &RegionAllocator,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    match run_native_fast_call_entry_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc_ref,
        resume_pc,
        rf,
        base,
        argc,
        retc,
        collect_metrics,
    )? {
        CallHotPath::Done => Ok(CallHotPath::Done),
        CallHotPath::Return(value) => Ok(CallHotPath::Return(value)),
        CallHotPath::Miss => run_positional_call_entry_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc_ref,
            resume_pc,
            frame_base,
            self_ptr,
            rf,
            base,
            argc,
            retc,
            allocator,
            collect_metrics,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_closure_exact_call_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    match try_run_closure_ic_hot(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        collect_metrics,
    )? {
        CallHotPath::Done => return Ok(CallHotPath::Done),
        CallHotPath::Return(value) => return Ok(CallHotPath::Return(value)),
        CallHotPath::Miss => {}
    }

    let prepared = match prepare_exact_closure_call(regs, rf, argc) {
        Ok(prepared) => prepared,
        Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    };
    run_prepared_exact_closure_call(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        resume_pc,
        frame_base,
        self_ptr,
        base,
        argc,
        retc,
        prepared,
        collect_metrics,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_closure_exact_call_entry_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let result = run_closure_exact_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        *pc_ref,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        collect_metrics,
    )?;
    Ok(finish_call_entry(
        result,
        pc_ref,
        self_ptr,
        resume_pc,
        "closure exact call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_exact_call_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    match try_run_closure_ic_hot(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        collect_metrics,
    )? {
        CallHotPath::Done => return Ok(CallHotPath::Done),
        CallHotPath::Return(value) => return Ok(CallHotPath::Return(value)),
        CallHotPath::Miss => {}
    }

    match &regs[rf as usize] {
        Val::Closure(closure) if closure.named_params.is_empty() && closure.params.len() == argc as usize => {
            let prepared = match prepare_exact_closure_call(regs, rf, argc) {
                Ok(prepared) => prepared,
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
            };
            run_prepared_exact_closure_call(
                frame_raw,
                regs,
                ctx,
                call_ic,
                pc,
                resume_pc,
                frame_base,
                self_ptr,
                base,
                argc,
                retc,
                prepared,
                collect_metrics,
            )
        }
        Val::RustFunction(_) | Val::RustFastFunction(_) => {
            let callable =
                NativeCallable::from_val(&regs[rf as usize]).expect("native function match should produce callable");
            let ret_layout = CallReturnLayout::new(base, retc);
            match invoke_native_callable_with_ic(
                ctx,
                regs,
                &mut call_ic[pc],
                callable,
                argc,
                ret_layout,
                collect_metrics,
            ) {
                Ok(handled) => {
                    debug_assert!(handled);
                    Ok(CallHotPath::Done)
                }
                Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
            }
        }
        Val::Closure(closure) if !closure.named_params.is_empty() => {
            frame_return_common(frame_raw, pc, Err(anyhow!("exact call does not accept named fallback")))
                .map(CallHotPath::Return)
        }
        Val::Closure(closure) => frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                closure.params.len(),
                argc
            )),
        )
        .map(CallHotPath::Return),
        other => frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!("{} is not an exact positional callable", other.type_name())),
        )
        .map(CallHotPath::Return),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_exact_call_entry_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let result = run_exact_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        *pc_ref,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        collect_metrics,
    )?;
    Ok(finish_call_entry(
        result,
        pc_ref,
        self_ptr,
        resume_pc,
        "exact call common cannot miss",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_closure_slow_call(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    base: u16,
    argc: u8,
    retc: u8,
    allocator: &RegionAllocator,
    closure_arc: &Arc<ClosureValue>,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let start = base as usize;
    let n = argc as usize;
    let call_args = CallArgs::registers(RegisterSpan::current(start, n));
    let _frame_guard = CallFrameStackGuard::push(
        self_ptr,
        CallFrameMeta::inline_return(resume_pc, base, retc, frame_base),
    );
    let positional_count = closure_arc.params.len();
    let named_count = closure_arc.named_params.len();
    if call_args.len() < positional_count
        || call_args.len() > positional_count + named_count
        || (named_count == 0 && call_args.len() != positional_count)
    {
        return frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                positional_count,
                call_args.len()
            )),
        )
        .map(CallHotPath::Return);
    }
    let extra_positional_count = call_args.len().saturating_sub(positional_count);
    let positional_call_args = if extra_positional_count == 0 {
        call_args
    } else {
        let span = call_args.span();
        CallArgs::registers(RegisterSpan::new(span.base, positional_count, span.window))
    };
    let closure = closure_arc.as_ref();
    let fun = closure.code.get_or_init(|| {
        let c = Compiler::new();
        Arc::new(c.compile_function_with_param_types_and_captures(
            closure.params.as_ref(),
            closure.param_types.as_ref(),
            closure.named_params.as_ref(),
            closure.body.as_ref(),
            closure.capture_specs.as_ref(),
        ))
    });
    let frame_info = closure.frame_info_ref();
    let captures_arc = Arc::clone(&closure.captures);
    let capture_specs_arc = Arc::clone(&closure.capture_specs);
    let call_result = if closure.named_params.is_empty() {
        Vm::exec_function_with_args(
            fun.as_ref(),
            positional_call_args,
            &mut [],
            Some(Arc::clone(&captures_arc)),
            Some(Arc::clone(&capture_specs_arc)),
            ctx,
            self_ptr,
            Some(frame_info),
            collect_metrics,
        )
    } else {
        let named_params = closure.named_params.as_ref();
        allocator.with_indexed_vals(named_params.len(), |resolved_seed| -> Result<Val> {
            for idx in 0..extra_positional_count {
                let span = call_args.span();
                resolved_seed.push((
                    idx,
                    copy_call_arg_value_for_register_with_metrics(
                        &regs[span.base + positional_count + idx],
                        collect_metrics,
                    ),
                ));
            }
            for (idx, decl) in named_params.iter().enumerate() {
                if idx < extra_positional_count {
                    continue;
                }
                if let Some(default_fun) = closure.default_funcs.get(idx).and_then(|opt| opt.as_ref()) {
                    let default_frame = closure
                        .default_frame_info_ref(idx)
                        .expect("default frame info should exist");
                    let hidden_frame = pop_vm_frame(self_ptr);
                    let default_result = allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                        Vm::map_named_seed(default_fun, resolved_seed.as_slice(), seed_regs, collect_metrics)?;
                        Vm::exec_function_with_args(
                            default_fun,
                            positional_call_args,
                            seed_regs.as_mut_slice(),
                            Some(Arc::clone(&captures_arc)),
                            Some(Arc::clone(&capture_specs_arc)),
                            ctx,
                            self_ptr,
                            Some(default_frame),
                            collect_metrics,
                        )
                    });
                    if let Some(meta) = hidden_frame {
                        push_vm_frame(self_ptr, meta);
                    }
                    let default_val = default_result?;
                    clear_pending_resume_pc(self_ptr);
                    resolved_seed.push((idx, default_val));
                } else if matches!(decl.type_annotation, Some(Type::Optional(_))) {
                    resolved_seed.push((idx, Val::Nil));
                } else {
                    return Err(anyhow!("Missing required named argument: {}", decl.name));
                }
            }
            allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                Vm::map_named_seed_take(fun, resolved_seed.as_mut_slice(), seed_regs)?;
                Vm::exec_function_with_args(
                    fun.as_ref(),
                    positional_call_args,
                    seed_regs.as_mut_slice(),
                    Some(Arc::clone(&captures_arc)),
                    Some(Arc::clone(&capture_specs_arc)),
                    ctx,
                    self_ptr,
                    Some(frame_info),
                    collect_metrics,
                )
            })
        })
    };
    match call_result {
        Ok(val) => {
            if retc > 0 {
                assign_reg_with_metrics(regs, base as usize, val, collect_metrics);
            }
            Ok(CallHotPath::Done)
        }
        Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    }
}

#[allow(clippy::too_many_arguments)]
fn load_named_pairs(
    regs: &[Val],
    named_start: usize,
    named_len: usize,
    out: &mut Vec<(String, Val)>,
    collect_metrics: bool,
) -> Result<()> {
    for idx in 0..named_len {
        let key_val = &regs[named_start + 2 * idx];
        let value = copy_call_arg_value_for_register_with_metrics(&regs[named_start + 2 * idx + 1], collect_metrics);
        let key = match key_val {
            Val::Str(value) => value.to_string(),
            Val::ShortStr(value) => value.as_str().to_string(),
            Val::Int(value) => value.to_string(),
            Val::Float(value) => value.to_string(),
            Val::Bool(value) => value.to_string(),
            _ => return Err(anyhow!("Named argument key must be primitive, got {:?}", key_val)),
        };
        out.push((key, value));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_named_closure_call_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    self_ptr: *mut Vm,
    base_pos: u16,
    posc: u8,
    base_named: u16,
    namedc: u8,
    retc: u8,
    ret_layout: CallReturnLayout,
    closure_arc: &Arc<ClosureValue>,
    allocator: &RegionAllocator,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let start_pos = base_pos as usize;
    let npos = posc as usize;
    if npos != closure_arc.params.len() {
        return frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                closure_arc.params.len(),
                npos
            )),
        )
        .map(CallHotPath::Return);
    }

    let start_named = base_named as usize;
    let nnamed = namedc as usize;
    let closure = closure_arc.as_ref();
    let named_params = closure.named_params.as_ref();
    let fun = closure.code.get_or_init(|| {
        let c = Compiler::new();
        Arc::new(c.compile_function_with_param_types_and_captures(
            closure.params.as_ref(),
            closure.param_types.as_ref(),
            named_params,
            closure.body.as_ref(),
            closure.capture_specs.as_ref(),
        ))
    });
    let layout = &fun.named_param_layout;
    if layout.len() != named_params.len() {
        return frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Named parameter layout mismatch (layout={}, decls={})",
                layout.len(),
                named_params.len()
            )),
        )
        .map(CallHotPath::Return);
    }

    let positional_span = RegisterSpan::current(start_pos, npos);
    let call_args = CallArgs::registers(positional_span);
    let named_slice = &regs[start_named..start_named + nnamed * 2];
    let closure_ptr = Arc::as_ptr(closure_arc) as usize;
    let site_plan =
        match get_or_build_named_call_site_plan(call_ic, pc, closure_ptr, nnamed, ret_layout, closure, named_slice) {
            Ok(plan) => plan,
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        };
    let plan = &site_plan.named;
    let frame_info = closure.frame_info_ref();
    let (captures, capture_specs) = closure.frame_captures();
    let call_result = allocator.with_indexed_vals(
        plan.provided_indices.len() + plan.defaults_to_eval.len() + plan.optional_nil.len(),
        |seed_pairs| {
            seed_pairs.clear();
            for (arg_idx, param_idx) in plan.provided_indices.iter().enumerate() {
                let val_reg = start_named + 2 * arg_idx + 1;
                seed_pairs.push((
                    *param_idx,
                    copy_call_arg_value_for_register_with_metrics(&regs[val_reg], collect_metrics),
                ));
            }
            for &default_idx in plan.defaults_to_eval.iter() {
                let default_fun = closure
                    .default_funcs
                    .get(default_idx)
                    .and_then(|opt| opt.as_ref())
                    .expect("default function must exist for DefaultThunk");
                let default_frame = closure
                    .default_frame_info_ref(default_idx)
                    .expect("default frame info should exist");
                let default_layout = closure
                    .default_seed_regs(default_idx)
                    .expect("default seed layout should exist for default thunk");
                let hidden_frame = pop_vm_frame(self_ptr);
                let default_result = allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
                    for (seed_idx, seed_val) in seed_pairs.iter() {
                        let reg = default_layout
                            .get(*seed_idx)
                            .copied()
                            .expect("default seed layout must cover parent index");
                        seed_regs.push((
                            reg,
                            copy_call_arg_value_for_register_with_metrics(seed_val, collect_metrics),
                        ));
                    }
                    Vm::exec_function_with_args(
                        default_fun,
                        call_args,
                        seed_regs.as_mut_slice(),
                        captures.clone(),
                        capture_specs.clone(),
                        ctx,
                        self_ptr,
                        Some(default_frame),
                        collect_metrics,
                    )
                });
                if let Some(meta) = hidden_frame {
                    push_vm_frame(self_ptr, meta);
                }
                let default_val = default_result?;
                clear_pending_resume_pc(self_ptr);
                seed_pairs.push((default_idx, default_val));
            }
            for &optional_idx in plan.optional_nil.iter() {
                seed_pairs.push((optional_idx, Val::Nil));
            }

            allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
                for (seed_idx, seed_val) in seed_pairs.iter_mut() {
                    let reg = fun
                        .named_param_regs
                        .get(*seed_idx)
                        .copied()
                        .ok_or_else(|| anyhow!("Named parameter index {} out of range", seed_idx))?;
                    seed_regs.push((reg, std::mem::replace(seed_val, Val::Nil)));
                }
                Vm::exec_function_with_args(
                    fun.as_ref(),
                    call_args,
                    seed_regs.as_mut_slice(),
                    captures.clone(),
                    capture_specs.clone(),
                    ctx,
                    self_ptr,
                    Some(frame_info),
                    collect_metrics,
                )
            })
        },
    );
    match call_result {
        Ok(val) => {
            if retc > 0 {
                assign_reg_with_metrics(regs, base_pos as usize, val, collect_metrics);
            }
            Ok(CallHotPath::Done)
        }
        Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_named_call_common(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base_pos: u16,
    posc: u8,
    base_named: u16,
    namedc: u8,
    retc: u8,
    allocator: &RegionAllocator,
    collect_metrics: bool,
) -> Result<CallHotPath> {
    let pc = *pc_ref;
    let _frame_guard = CallFrameStackGuard::push(
        self_ptr,
        CallFrameMeta::inline_return(resume_pc, base_pos, retc, frame_base),
    );
    let start_pos = base_pos as usize;
    let npos = posc as usize;
    let start_named = base_named as usize;
    let nnamed = namedc as usize;
    let ret_layout = CallReturnLayout::new(base_pos, retc);

    match call_target_from_register(regs, rf) {
        CallTarget::Closure(closure_arc) => match run_named_closure_call_common(
            frame_raw,
            regs,
            ctx,
            call_ic,
            pc,
            self_ptr,
            base_pos,
            posc,
            base_named,
            namedc,
            retc,
            ret_layout,
            &closure_arc,
            allocator,
            collect_metrics,
        )? {
            CallHotPath::Done => {}
            CallHotPath::Return(value) => return Ok(CallHotPath::Return(value)),
            CallHotPath::Miss => unreachable!("named closure call common cannot miss"),
        },
        CallTarget::Native(callable @ (NativeCallable::Rust(_) | NativeCallable::RustFast(_))) => {
            if nnamed > 0 {
                return frame_return_common(
                    frame_raw,
                    pc,
                    Err(anyhow!("Named arguments are not supported for native functions")),
                )
                .map(CallHotPath::Return);
            }
            match invoke_native_callable_with_ic(
                ctx,
                regs,
                &mut call_ic[pc],
                callable,
                posc,
                ret_layout,
                collect_metrics,
            ) {
                Ok(handled) => debug_assert!(handled),
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
            }
        }
        CallTarget::Native(NativeCallable::RustFastNamed(func)) => {
            let call_output = allocator.with_named_pairs(nnamed, |named_vec| {
                load_named_pairs(regs, start_named, nnamed, named_vec, collect_metrics)?;
                invoke_rust_fast_function_named(
                    ctx,
                    func,
                    ArgWindow::new(&regs[start_pos..start_pos + npos]),
                    named_vec.as_slice(),
                )
            });
            match call_output {
                Ok(value) => ReturnSlot::new(base_pos as usize, retc).write(regs, value, collect_metrics),
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
            }
        }
        CallTarget::Native(NativeCallable::RustNamed(func)) => {
            let call_output = allocator.with_named_pairs(nnamed, |named_vec| {
                load_named_pairs(regs, start_named, nnamed, named_vec, collect_metrics)?;
                invoke_rust_function_named_fast(
                    ctx,
                    func,
                    ArgWindow::new(&regs[start_pos..start_pos + npos]),
                    named_vec.as_slice(),
                )
            });
            match call_output {
                Ok(value) => ReturnSlot::new(base_pos as usize, retc).write(regs, value, collect_metrics),
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
            }
        }
        CallTarget::NotFunction(type_name) => {
            return frame_return_common(frame_raw, pc, Err(anyhow!("{} is not a function", type_name)))
                .map(CallHotPath::Return);
        }
    }

    *pc_ref = take_pending_resume_pc(self_ptr, resume_pc);
    Ok(CallHotPath::Done)
}
